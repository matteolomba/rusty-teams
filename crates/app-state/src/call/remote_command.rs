use calling::{CallCommand, MuteCommand, ShareKind};
use gpui_kit::*;

use super::join::JoinMode;
use crate::app_state::AppState;

pub const JOIN_FLAG: &str = "--join";
pub const HANG_UP_FLAG: &str = "--hang-up";
pub const ADMIT_ALL_FLAG: &str = "--admit-all";
pub const UNMUTE_FLAG: &str = "--unmute";
pub const CALL_FLAG: &str = "--call";
const JOIN_COMMAND: &str = "join ";
const HANG_UP_COMMAND: &str = "hang-up";
const ADMIT_ALL_COMMAND: &str = "admit-all";
const UNMUTE_COMMAND: &str = "unmute";
const MUTE_COMMAND: &str = "mute";
const CAMERA_ON_COMMAND: &str = "camera on";
const CAMERA_OFF_COMMAND: &str = "camera off";
const SHARE_SOURCES_COMMAND: &str = "share-sources";
const SHARE_COMMAND: &str = "share";
const STOP_SHARE_COMMAND: &str = "stop-share";
const ECHO_COMMAND: &str = "echo";
const HOLD_COMMAND: &str = "hold";
const RESUME_COMMAND: &str = "resume";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCommand {
    Join(String),
    HangUp,
    AdmitAll,
    Unmute,
    Mute,
    Camera(bool),
    RefreshShareSources,
    ShareScreen,
    StopShare,
    EchoCall,
    Hold(bool),
}

impl RemoteCommand {
    pub fn encode(&self) -> String {
        match self {
            RemoteCommand::Join(link) => format!("{JOIN_COMMAND}{link}"),
            RemoteCommand::HangUp => HANG_UP_COMMAND.to_owned(),
            RemoteCommand::AdmitAll => ADMIT_ALL_COMMAND.to_owned(),
            RemoteCommand::Unmute => UNMUTE_COMMAND.to_owned(),
            RemoteCommand::Mute => MUTE_COMMAND.to_owned(),
            RemoteCommand::Camera(true) => CAMERA_ON_COMMAND.to_owned(),
            RemoteCommand::Camera(false) => CAMERA_OFF_COMMAND.to_owned(),
            RemoteCommand::RefreshShareSources => SHARE_SOURCES_COMMAND.to_owned(),
            RemoteCommand::ShareScreen => SHARE_COMMAND.to_owned(),
            RemoteCommand::StopShare => STOP_SHARE_COMMAND.to_owned(),
            RemoteCommand::EchoCall => ECHO_COMMAND.to_owned(),
            RemoteCommand::Hold(true) => HOLD_COMMAND.to_owned(),
            RemoteCommand::Hold(false) => RESUME_COMMAND.to_owned(),
        }
    }

    pub fn decode(text: &str) -> Option<RemoteCommand> {
        match text {
            HANG_UP_COMMAND => return Some(RemoteCommand::HangUp),
            ADMIT_ALL_COMMAND => return Some(RemoteCommand::AdmitAll),
            UNMUTE_COMMAND => return Some(RemoteCommand::Unmute),
            MUTE_COMMAND => return Some(RemoteCommand::Mute),
            CAMERA_ON_COMMAND => return Some(RemoteCommand::Camera(true)),
            CAMERA_OFF_COMMAND => return Some(RemoteCommand::Camera(false)),
            SHARE_SOURCES_COMMAND => return Some(RemoteCommand::RefreshShareSources),
            SHARE_COMMAND => return Some(RemoteCommand::ShareScreen),
            STOP_SHARE_COMMAND => return Some(RemoteCommand::StopShare),
            ECHO_COMMAND => return Some(RemoteCommand::EchoCall),
            HOLD_COMMAND => return Some(RemoteCommand::Hold(true)),
            RESUME_COMMAND => return Some(RemoteCommand::Hold(false)),
            _ => {}
        }
        let link = text.strip_prefix(JOIN_COMMAND)?.trim();
        (!link.is_empty()).then(|| RemoteCommand::Join(link.to_owned()))
    }
}

impl AppState {
    /// Commands a second `teams` process forwards, used by the call end-to-end test.
    pub fn run_remote_command(&mut self, text: &str, cx: &mut Context<Self>) {
        match RemoteCommand::decode(text) {
            Some(RemoteCommand::Join(link)) => {
                self.join_link(&link, JoinMode::Direct, cx);
            }
            Some(RemoteCommand::HangUp) => self.leave_call(cx),
            Some(RemoteCommand::AdmitAll) => self.admit_all_call_guests(cx),
            Some(RemoteCommand::Unmute) => {
                self.send_call_command(CallCommand::Mute(MuteCommand::Unmute))
            }
            Some(RemoteCommand::Mute) => {
                self.send_call_command(CallCommand::Mute(MuteCommand::Mute))
            }
            Some(RemoteCommand::Camera(on)) => self.send_call_command(CallCommand::SetCamera(on)),
            Some(RemoteCommand::RefreshShareSources) => {
                self.send_call_command(CallCommand::RefreshShareSources)
            }
            Some(RemoteCommand::ShareScreen) => self.share_first_screen(),
            Some(RemoteCommand::StopShare) => self.send_call_command(CallCommand::StopShare),
            Some(RemoteCommand::EchoCall) => self.start_test_call(cx),
            Some(RemoteCommand::Hold(hold)) => self.send_call_command(CallCommand::Hold(hold)),
            None => {}
        }
    }

    fn share_first_screen(&self) {
        let screen = self.call.as_ref().and_then(|call| {
            call.model
                .share_sources
                .iter()
                .find(|source| source.kind == ShareKind::Screen)
                .cloned()
        });
        if let Some(screen) = screen {
            self.send_call_command(CallCommand::StartShare(screen));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteCommand;

    #[test]
    fn commands_survive_the_trip_through_the_pipe() {
        for command in [
            RemoteCommand::Join("https://teams.microsoft.com/l/meetup-join/x".to_owned()),
            RemoteCommand::HangUp,
            RemoteCommand::AdmitAll,
            RemoteCommand::Unmute,
            RemoteCommand::Mute,
            RemoteCommand::Camera(true),
            RemoteCommand::Camera(false),
            RemoteCommand::RefreshShareSources,
            RemoteCommand::ShareScreen,
            RemoteCommand::StopShare,
            RemoteCommand::EchoCall,
            RemoteCommand::Hold(true),
            RemoteCommand::Hold(false),
        ] {
            assert_eq!(RemoteCommand::decode(&command.encode()), Some(command));
        }
        assert_eq!(RemoteCommand::decode("join  "), None);
        assert_eq!(RemoteCommand::decode("reboot"), None);
    }

    #[test]
    fn test_only_commands_are_not_part_of_the_user_allowlist() {
        assert_eq!(RemoteCommand::decode("stats C:\\temp\\stats.json"), None);
        assert_eq!(RemoteCommand::decode("stats "), None);
        assert_eq!(RemoteCommand::decode("input cable output"), None);
        assert_eq!(RemoteCommand::decode("input "), None);
    }
}
