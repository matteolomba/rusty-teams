// Copyright 2025 LiveKit, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// Runs a Rust callback invoked from native code without letting a panic cross the FFI boundary.
/// The event is dropped and the failure logged; native code keeps running.
pub fn guard_callback(name: &'static str, callback: impl FnOnce()) {
    let _ = guard_callback_with(name, (), move || callback());
}

/// Like [`guard_callback`], but returns `fallback` when the callback panics.
pub fn guard_callback_with<T>(name: &'static str, fallback: T, callback: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)) {
        Ok(value) => value,
        Err(_) => {
            log::error!("WebRTC callback `{name}` panicked; the event was dropped");
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_callback_does_not_unwind_into_native_code() {
        let reached = std::sync::atomic::AtomicBool::new(false);
        let reached = &reached;
        let ran = std::panic::catch_unwind(|| {
            guard_callback("test::on_event", || {
                reached.store(true, std::sync::atomic::Ordering::SeqCst);
                panic!("simulated callback failure");
            });
        });
        assert!(ran.is_ok());
        assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn a_panicking_callback_returns_the_fallback_value() {
        let value = guard_callback_with("test::get_value", -1, || 7);
        assert_eq!(value, 7);
        let value = guard_callback_with("test::get_value", -1, || -> i32 {
            panic!("simulated callback failure");
        });
        assert_eq!(value, -1);
    }
}

#[cxx::bridge(namespace = "livekit_ffi")]
pub mod ffi {
    // Wrapper to opaque C++ objects
    // https://github.com/dtolnay/cxx/issues/741
    // Used to allow SharedPtr/UniquePtr type inside a rust::Vec
    pub struct MediaStreamPtr {
        pub ptr: SharedPtr<MediaStream>,
    }

    pub struct CandidatePtr {
        pub ptr: SharedPtr<Candidate>,
    }

    pub struct AudioTrackPtr {
        pub ptr: SharedPtr<AudioTrack>,
    }

    pub struct VideoTrackPtr {
        pub ptr: SharedPtr<VideoTrack>,
    }

    pub struct RtpSenderPtr {
        pub ptr: SharedPtr<RtpSender>,
    }

    pub struct RtpReceiverPtr {
        pub ptr: SharedPtr<RtpReceiver>,
    }

    pub struct RtpTransceiverPtr {
        pub ptr: SharedPtr<RtpTransceiver>,
    }

    unsafe extern "C++" {
        include!("livekit/helper.h");

        type MediaStream = crate::media_stream::ffi::MediaStream;
        type AudioTrack = crate::media_stream::ffi::AudioTrack;
        type VideoTrack = crate::media_stream::ffi::VideoTrack;
        type Candidate = crate::candidate::ffi::Candidate;
        type RtpSender = crate::rtp_sender::ffi::RtpSender;
        type RtpReceiver = crate::rtp_receiver::ffi::RtpReceiver;
        type RtpTransceiver = crate::rtp_transceiver::ffi::RtpTransceiver;

        fn _vec_media_stream_ptr() -> Vec<MediaStreamPtr>;
        fn _vec_candidate_ptr() -> Vec<CandidatePtr>;
        fn _vec_audio_track_ptr() -> Vec<AudioTrackPtr>;
        fn _vec_video_track_ptr() -> Vec<VideoTrackPtr>;
        fn _vec_rtp_sender_ptr() -> Vec<RtpSenderPtr>;
        fn _vec_rtp_receiver_ptr() -> Vec<RtpReceiverPtr>;
        fn _vec_rtp_transceiver_ptr() -> Vec<RtpTransceiverPtr>;
    }
}
