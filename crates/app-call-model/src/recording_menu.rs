use calling::{RecordingKind, StopScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordAction {
    Start(RecordingKind),
    Stop(StopScope),
}

impl RecordAction {
    pub fn label(self) -> String {
        app_base::ui_locale::tr(match self {
            RecordAction::Start(RecordingKind::Video) => "ui.calls.record.start_recording",
            RecordAction::Start(RecordingKind::AudioOnly) => "ui.calls.record.record_audio_only",
            RecordAction::Start(RecordingKind::TranscriptOnly) => {
                "ui.calls.record.start_transcription"
            }
            RecordAction::Stop(StopScope::Both | StopScope::RecordingOnly) => {
                "ui.calls.record.stop_recording"
            }
            RecordAction::Stop(StopScope::TranscriptionOnly) => {
                "ui.calls.record.stop_transcription"
            }
        })
    }

    pub fn confirm_title(self) -> Option<String> {
        let key = match self {
            RecordAction::Start(_) => return None,
            RecordAction::Stop(StopScope::Both) => "ui.calls.record.confirm_both",
            RecordAction::Stop(StopScope::RecordingOnly) => "ui.calls.record.confirm_recording",
            RecordAction::Stop(StopScope::TranscriptionOnly) => {
                "ui.calls.record.confirm_transcription"
            }
        };
        Some(app_base::ui_locale::tr(key))
    }
}

pub fn record_actions(recording: bool, transcribing: bool) -> Vec<(RecordAction, String)> {
    let labelled = |action: RecordAction| (action, action.label());
    match (recording, transcribing) {
        (false, false) => vec![
            labelled(RecordAction::Start(RecordingKind::Video)),
            labelled(RecordAction::Start(RecordingKind::AudioOnly)),
            labelled(RecordAction::Start(RecordingKind::TranscriptOnly)),
        ],
        (true, true) => vec![
            labelled(RecordAction::Stop(StopScope::RecordingOnly)),
            labelled(RecordAction::Stop(StopScope::TranscriptionOnly)),
            (
                RecordAction::Stop(StopScope::Both),
                app_base::ui_locale::tr("ui.calls.record.stop_both"),
            ),
        ],
        (true, false) => vec![labelled(RecordAction::Stop(StopScope::Both))],
        (false, true) => vec![labelled(RecordAction::Stop(StopScope::TranscriptionOnly))],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingChip {
    Recording,
    RecordingAudio,
    Transcribing,
}

impl RecordingChip {
    pub fn label(self) -> String {
        app_base::ui_locale::tr(match self {
            RecordingChip::Recording => "ui.calls.record.chip_recording",
            RecordingChip::RecordingAudio => "ui.calls.record.chip_recording_audio",
            RecordingChip::Transcribing => "ui.calls.record.chip_transcribing",
        })
    }
}

pub fn recording_chip(
    recording: bool,
    audio_only: bool,
    transcribing: bool,
) -> Option<RecordingChip> {
    match (recording, audio_only, transcribing) {
        (true, true, _) => Some(RecordingChip::RecordingAudio),
        (true, false, _) => Some(RecordingChip::Recording),
        (false, _, true) => Some(RecordingChip::Transcribing),
        (false, _, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(recording: bool, transcribing: bool) -> Vec<String> {
        record_actions(recording, transcribing)
            .into_iter()
            .map(|(_, label)| label)
            .collect()
    }

    #[test]
    fn nothing_running_offers_the_three_starts() {
        assert_eq!(
            labels(false, false),
            [
                "Start recording".to_owned(),
                "Record audio only".to_owned(),
                "Start transcription only".to_owned()
            ]
        );
    }

    #[test]
    fn recording_with_transcript_offers_the_three_stop_variants() {
        let actions = record_actions(true, true);
        assert_eq!(actions[0].0, RecordAction::Stop(StopScope::RecordingOnly));
        assert_eq!(
            actions[1].0,
            RecordAction::Stop(StopScope::TranscriptionOnly)
        );
        assert_eq!(actions[2].0, RecordAction::Stop(StopScope::Both));
        assert_eq!(
            labels(true, true),
            [
                "Stop recording".to_owned(),
                "Stop transcription".to_owned(),
                "Stop both".to_owned()
            ]
        );
    }

    #[test]
    fn a_single_running_part_offers_the_one_matching_stop() {
        assert_eq!(
            record_actions(true, false),
            [(
                RecordAction::Stop(StopScope::Both),
                "Stop recording".to_owned()
            )]
        );
        assert_eq!(
            record_actions(false, true),
            [(
                RecordAction::Stop(StopScope::TranscriptionOnly),
                "Stop transcription".to_owned()
            )]
        );
    }

    #[test]
    fn only_stops_ask_for_confirmation() {
        assert_eq!(
            RecordAction::Start(RecordingKind::Video).confirm_title(),
            None
        );
        assert_eq!(
            RecordAction::Stop(StopScope::Both).confirm_title(),
            Some("Stop recording and transcription?".to_owned())
        );
        assert_eq!(
            RecordAction::Stop(StopScope::RecordingOnly).confirm_title(),
            Some("Stop recording?".to_owned())
        );
        assert_eq!(
            RecordAction::Stop(StopScope::TranscriptionOnly).confirm_title(),
            Some("Stop transcription?".to_owned())
        );
    }

    #[test]
    fn the_chip_names_what_is_running() {
        assert_eq!(
            recording_chip(true, false, true),
            Some(RecordingChip::Recording)
        );
        assert_eq!(
            recording_chip(true, true, true),
            Some(RecordingChip::RecordingAudio)
        );
        assert_eq!(
            recording_chip(false, false, true),
            Some(RecordingChip::Transcribing)
        );
        assert_eq!(recording_chip(false, false, false), None);
        assert_eq!(RecordingChip::RecordingAudio.label(), "Recording audio");
    }
}
