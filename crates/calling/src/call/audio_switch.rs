use super::*;

#[derive(Default)]
pub(super) struct Recording {
    pub(super) samples: Vec<i16>,
    pub(super) current_second: Vec<i16>,
    pub(super) per_second: Vec<(f32, f32)>,
}

pub(super) fn withhold_media(
    muted: &mut bool,
    audio: &AudioSetup,
    software_audio: Option<&SoftwareAudio>,
    remote: &RemoteCall,
    local_video: &mut LocalVideo,
    control: &CallControl,
) {
    *muted = apply_mute(
        MuteCommand::Mute,
        *muted,
        audio,
        software_audio,
        remote,
        control,
    );
    if local_video.camera_on() {
        local_video.stop_camera();
        control.send(CallUpdate::Camera(false));
    }
}

pub(super) fn record_audio_devices(
    timeline: &Timeline,
    lists: &devices::DeviceLists,
    input: Option<&AppliedDevice>,
    output: Option<&AppliedDevice>,
) {
    let name = |devices: &[AudioDevice], applied: Option<&AppliedDevice>| {
        applied
            .and_then(|applied| devices.iter().find(|device| device.index == applied.index))
            .map_or("none", |device| device.name.as_str())
            .to_owned()
    };
    timeline.record(
        "audio devices",
        format!(
            "input={} output={}",
            name(&lists.inputs, input),
            name(&lists.outputs, output)
        ),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn follow_device_change(
    flow: Flow,
    (previous, current): (&[AudioDevice], &[AudioDevice]),
    choice: &DeviceChoice,
    applied: &mut Option<AppliedDevice>,
    touched_ids: &[String],
    select: impl Fn(u16) -> bool,
    timeline: &Timeline,
    control: &CallControl,
) -> bool {
    let system_default = devices::system_default_id(flow);
    let (index, fell_back, recovery) =
        match devices::reselect(applied.as_ref(), current, choice, system_default.as_deref()) {
            Reselect::Keep => {
                // On Windows, a Bluetooth headset profile switch or brief disconnect+reconnect
                // within the debounce window can leave the same endpoint index while the legacy
                // WASAPI render/capture thread has already died on timeout/invalidation.
                let Some(active) = applied.as_ref() else {
                    return false;
                };
                let active_touched = touched_ids
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&active.key));
                if !active_touched {
                    return false;
                }
                let Some(still_present) = current
                    .iter()
                    .find(|device| device.key().eq_ignore_ascii_case(&active.key))
                else {
                    return false;
                };
                (still_present.index, false, true)
            }
            Reselect::Select(index) => (index, false, false),
            Reselect::FallBack(index) => (index, true, false),
        };
    let selected = select(index);
    timeline.record(
        if recovery {
            "device recovery"
        } else {
            "device change"
        },
        format!("{flow:?} {choice:?} -> index {index} fallback={fell_back} applied={selected}"),
    );
    if selected {
        *applied = current
            .iter()
            .find(|device| device.index == index)
            .map(|device| AppliedDevice {
                index,
                key: device.key(),
            });
    }
    if fell_back {
        let fallback = current.iter().find(|device| device.index == index);
        control.send(CallUpdate::Notice(devices::disconnect_notice(
            previous, choice, fallback,
        )));
    }
    selected
}

pub(super) fn apply_mute(
    command: MuteCommand,
    currently_muted: bool,
    audio: &AudioSetup,
    software_audio: Option<&SoftwareAudio>,
    remote: &RemoteCall,
    control: &CallControl,
) -> bool {
    let effect = MuteEffect::for_muted(command.target(currently_muted));
    audio.set_device_tracks_enabled(effect.track_enabled);
    if let Some(software_audio) = software_audio {
        software_audio.set_microphone_muted(effect.muted);
    }
    control.send(CallUpdate::Muted(effect.muted));
    let remote = remote.clone();
    tokio::spawn(async move {
        let _ = remote
            .signaling
            .update_endpoint_state(
                &remote.conversation,
                &remote.participant,
                effect.endpoint_is_muted,
            )
            .await;
    });
    effect.muted
}

pub(super) fn silence_for_server_mute(
    audio: &AudioSetup,
    software_audio: Option<&SoftwareAudio>,
    control: &CallControl,
) {
    let effect = MuteEffect::for_muted(true);
    audio.set_device_tracks_enabled(effect.track_enabled);
    if let Some(software_audio) = software_audio {
        software_audio.set_microphone_muted(true);
    }
    control.send(CallUpdate::Muted(true));
}

pub(super) fn silence_for_hold(
    on_hold: bool,
    muted: bool,
    audio: Option<&AudioSetup>,
    software_audio: Option<&SoftwareAudio>,
) {
    let silent = on_hold || muted;
    if let Some(audio) = audio {
        audio.set_device_tracks_enabled(MuteEffect::for_muted(silent).track_enabled);
    }
    if let Some(software_audio) = software_audio {
        software_audio.set_microphone_muted(silent);
    }
}

pub(super) fn swap_audio_track(peer: &PeerConnection, track: &RtcAudioTrack) -> Result<()> {
    let sender = peer
        .senders()
        .into_iter()
        .find(|sender| matches!(sender.track(), Some(MediaStreamTrack::Audio(_))))
        .ok_or_else(|| Error::Webrtc("no audio sender".into()))?;
    sender
        .set_track(Some(MediaStreamTrack::Audio(track.clone())))
        .map_err(|error| Error::Webrtc(error.to_string()))
}

pub(super) fn outgoing_track<'a>(
    software_audio: Option<&'a SoftwareAudio>,
    device_track: &'a RtcAudioTrack,
) -> &'a RtcAudioTrack {
    pick_track(software_audio.map(|audio| &audio.track), device_track)
}

pub(super) fn pick_track<'a, T>(software: Option<&'a T>, device: &'a T) -> &'a T {
    software.unwrap_or(device)
}

/// What the call wants right now: software capture and the ADM recording state.
/// `audio_disabled` is the single source of truth for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AudioIntent {
    pub(super) wanted: Option<SoftwareAudioPlan>,
    pub(super) adm_recording: bool,
}

impl AudioIntent {
    pub(super) fn disabled() -> Self {
        Self {
            wanted: None,
            adm_recording: false,
        }
    }
}

fn intent_for(
    mic_settings: MicSettings,
    sharing_sound: bool,
    uses_devices: bool,
    audio_disabled: bool,
) -> AudioIntent {
    if audio_disabled {
        return AudioIntent::disabled();
    }
    AudioIntent {
        wanted: SoftwareAudioPlan::wanted(mic_settings, sharing_sound, uses_devices),
        adm_recording: uses_devices,
    }
}

pub(super) fn audio_intent(
    mic_settings: MicSettings,
    share_sound_wanted: bool,
    local_video: &LocalVideo,
    audio: Option<&AudioSetup>,
    audio_disabled: bool,
) -> AudioIntent {
    intent_for(
        mic_settings,
        share_sound_wanted && local_video.sharing(),
        audio.is_some_and(AudioSetup::uses_devices),
        audio_disabled,
    )
}

pub(super) fn restore_device_track(
    factory: &PeerConnectionFactory,
    input: &DeviceChoice,
    peer: &PeerConnection,
    device_track: &RtcAudioTrack,
    timeline: &Timeline,
    adm_recording: bool,
) {
    use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
    let restored = unmount_software_track(
        adm_recording,
        || swap_audio_track(peer, device_track),
        || {
            if adm_recording {
                // The proxy ignores a redundant enable; toggle it so a stopped capture restarts
                // even when its flag still says enabled.
                if factory.adm_recording_enabled() {
                    factory.set_adm_recording_enabled(false);
                }
                factory.set_adm_recording_enabled(true);
                devices::select_input(factory, input);
            }
        },
    );
    if let Err(error) = restored {
        timeline.record("microphone track not restored", error.to_string());
    }
}

// The ADM pushes the microphone straight into the audio send stream whatever track the sender
// holds, so it must not run while the software track feeds that stream too: WebRTC aborts the
// process when two threads send audio at once (`audio_send_stream.cc` race check).

/// Stops the ADM before the software track reaches the sender when it is meant to be recording.
fn mount_software_track(
    adm_recording: bool,
    stop_adm: impl FnOnce(),
    mount: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if adm_recording {
        stop_adm();
    }
    mount()
}

/// Starts the ADM again only once the device track is back on the sender, and only if wanted.
fn unmount_software_track(
    adm_recording: bool,
    mount_device: impl FnOnce() -> Result<()>,
    start_adm: impl FnOnce(),
) -> Result<()> {
    let mounted = mount_device();
    if adm_recording {
        start_adm();
    }
    mounted
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_software_audio(
    factory: &PeerConnectionFactory,
    options: &CallOptions,
    input: &DeviceChoice,
    muted: bool,
    remote_audio: Option<RtcAudioTrack>,
    peer: &PeerConnection,
    device_track: &RtcAudioTrack,
    intent: AudioIntent,
    restart: bool,
    software_audio: &mut Option<SoftwareAudio>,
    mic_settings: MicSettings,
    control: &CallControl,
    timeline: &Timeline,
) {
    let AudioIntent {
        wanted,
        adm_recording,
    } = intent;
    let running = software_audio.as_ref().map(|audio| audio.plan);
    if running == wanted && !restart {
        crate::noise_stage::apply_for_outgoing(mic_settings, software_audio.is_none());
        return;
    }
    if running.is_some() {
        timeline.record("software audio", "stopping");
    }
    *software_audio = None;
    let was_sharing_sound = running.is_some_and(|plan| plan.computer_sound);
    let Some(plan) = wanted else {
        restore_device_track(factory, input, peer, device_track, timeline, adm_recording);
        control.send(CallUpdate::ShareSound(false));
        crate::noise_stage::apply_for_outgoing(mic_settings, true);
        return;
    };
    let started = SoftwareAudio::start(
        factory,
        options.audio,
        options.tone_hz,
        input,
        plan,
        muted,
        remote_audio,
    )
    .and_then(|audio| {
        mount_software_track(
            adm_recording,
            || {
                if options.audio == AudioMode::Platform {
                    use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
                    factory.set_adm_recording_enabled(false);
                }
            },
            || swap_audio_track(peer, &audio.track),
        )
        .map(|()| audio)
    });
    match started {
        Ok(audio) => {
            timeline.record(
                "software audio",
                format!("{:?}, computer sound {:?}", plan.processing, audio.loopback),
            );
            if let Some(source) = &audio.microphone_source {
                timeline.record("mic capture", source.clone());
            }
            if let Some(warning) = &audio.warning {
                timeline.record("computer sound warning", warning.clone());
            }
            control.send(CallUpdate::ShareSound(plan.computer_sound));
            if let Some(notice) = audio.microphone_notice {
                control.send(CallUpdate::Notice(notice.into()));
            }
            *software_audio = Some(audio);
        }
        Err(error) => {
            timeline.record("software audio failed", error.to_string());
            restore_device_track(factory, input, peer, device_track, timeline, adm_recording);
            if was_sharing_sound || plan.computer_sound {
                control.send(CallUpdate::ShareSound(false));
            }
            let notice = if plan.computer_sound {
                "Computer sound could not start, sharing without it"
            } else {
                "Microphone processing could not start, using the standard microphone"
            };
            control.send(CallUpdate::Notice(notice.into()));
        }
    }
    crate::noise_stage::apply_for_outgoing(mic_settings, software_audio.is_none());
}

pub(super) async fn record_track(
    track: RtcAudioTrack,
    recording: Arc<Mutex<Recording>>,
    tone_hz: f32,
) {
    let mut stream = NativeAudioStream::new(track, SAMPLE_RATE as i32, 1);
    while let Some(frame) = stream.next().await {
        let mut recording = recording.lock().await;
        recording.samples.extend_from_slice(&frame.data);
        recording.current_second.extend_from_slice(&frame.data);
        if recording.current_second.len() >= SAMPLE_RATE as usize {
            let second: Vec<i16> = std::mem::take(&mut recording.current_second);
            let summary = (rms(&second), tone_ratio(&second, tone_hz));
            recording.per_second.push(summary);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn the_adm_stops_before_the_software_track_is_mounted() {
        let steps = RefCell::new(Vec::new());
        mount_software_track(
            true,
            || steps.borrow_mut().push("stop adm"),
            || {
                steps.borrow_mut().push("mount software");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*steps.borrow(), ["stop adm", "mount software"]);
    }

    #[test]
    fn the_adm_is_left_alone_when_it_must_stay_disabled() {
        let steps = RefCell::new(Vec::new());
        mount_software_track(
            false,
            || steps.borrow_mut().push("stop adm"),
            || {
                steps.borrow_mut().push("mount software");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*steps.borrow(), ["mount software"]);
    }

    #[test]
    fn the_adm_starts_only_after_the_device_track_is_back() {
        let steps = RefCell::new(Vec::new());
        let unmounted = unmount_software_track(
            true,
            || {
                steps.borrow_mut().push("mount device");
                Err(Error::Webrtc("no audio sender".into()))
            },
            || steps.borrow_mut().push("start adm"),
        );
        assert!(unmounted.is_err());
        assert_eq!(*steps.borrow(), ["mount device", "start adm"]);
    }

    #[test]
    fn the_adm_does_not_restart_when_audio_is_disabled() {
        let steps = RefCell::new(Vec::new());
        let unmounted = unmount_software_track(
            false,
            || {
                steps.borrow_mut().push("mount device");
                Ok(())
            },
            || steps.borrow_mut().push("start adm"),
        );
        assert!(unmounted.is_ok());
        assert_eq!(*steps.borrow(), ["mount device"]);
    }

    #[test]
    fn disabled_audio_never_starts_software_capture() {
        let mic = MicSettings::default();
        assert_eq!(intent_for(mic, true, true, true), AudioIntent::disabled());
        assert_eq!(intent_for(mic, false, true, true), AudioIntent::disabled());
        let enabled = intent_for(mic, true, true, false);
        assert!(enabled.wanted.is_some_and(|plan| plan.computer_sound));
        assert!(enabled.adm_recording);
        let no_devices = intent_for(mic, true, false, false);
        assert!(no_devices.wanted.is_some_and(|plan| plan.computer_sound));
        assert!(!no_devices.adm_recording);
    }

    #[test]
    fn carry_over_without_a_negotiated_sender_uses_the_ordered_restore() {
        use libwebrtc::audio_source::native::NativeAudioSource;
        use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;

        let factory = crate::devices::shared_factory();
        let peer = factory
            .create_peer_connection(RtcConfiguration::default())
            .expect("a peer without an audio sender");
        let source =
            NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE as u32, 1, 0);
        let device_track = factory.create_audio_track("microphone", source);
        let timeline = Timeline::new(false);
        restore_device_track(
            &factory,
            &DeviceChoice::SystemDefault,
            &peer,
            &device_track,
            &timeline,
            false,
        );
        assert!(timeline.entries().iter().any(|entry| {
            entry.label == "microphone track not restored"
                && entry.detail.contains("no audio sender")
        }));
    }
}
