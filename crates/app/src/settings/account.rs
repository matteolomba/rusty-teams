use gpui_kit::*;
use teams_core::{
    ActivityAlert, ChatSettingChange, EnterKeyBehavior, MeetingChatNotifications, MessageDensity,
    NotificationPreset, UpArrowBehavior,
};

use super::rows::{Choice, control_row, select};
use crate::app_state::AppState;
use crate::i18n::tr;

/// A catalog key and the Teams value it sets. Resolve the key with [`tr`] where it is shown.
pub struct Labeled<T: 'static> {
    pub key: &'static str,
    pub value: T,
}

const fn option<T>(key: &'static str, value: T) -> Labeled<T> {
    Labeled { key, value }
}

pub const DENSITY: [Labeled<MessageDensity>; 2] = [
    option("ui.settings.account.density_comfy", MessageDensity::Comfy),
    option(
        "ui.settings.account.density_compact",
        MessageDensity::Compact,
    ),
];

pub const ENTER_KEY: [Labeled<EnterKeyBehavior>; 2] = [
    option("ui.settings.account.enter_send", EnterKeyBehavior::Send),
    option(
        "ui.settings.account.enter_newline",
        EnterKeyBehavior::NewLine,
    ),
];

pub const UP_ARROW: [Labeled<UpArrowBehavior>; 3] = [
    option(
        "ui.settings.account.up_navigate",
        UpArrowBehavior::NavigateToMessages,
    ),
    option(
        "ui.settings.account.up_edit_last",
        UpArrowBehavior::EditLastMessage,
    ),
    option(
        "ui.settings.account.up_stay",
        UpArrowBehavior::StayInCompose,
    ),
];

pub const PRESETS: [Labeled<NotificationPreset>; 3] = [
    option(
        "ui.settings.account.preset_all",
        NotificationPreset::AllActivity,
    ),
    option(
        "ui.settings.account.preset_mentions",
        NotificationPreset::MentionsAndReplies,
    ),
    option(
        "ui.settings.account.preset_custom",
        NotificationPreset::Customised,
    ),
];

pub const CHAT_MESSAGES: [Labeled<ActivityAlert>; 2] = [
    option("ui.settings.account.banner_only", ActivityAlert::OnlyBanner),
    option("ui.settings.account.off", ActivityAlert::Off),
];

pub const MENTIONS: [Labeled<ActivityAlert>; 2] = [
    option(
        "ui.settings.account.activity_and_banner",
        ActivityAlert::BannerAndFeed,
    ),
    option("ui.settings.account.activity_only", ActivityAlert::OnlyFeed),
];

pub const ACTIVITY_OR_OFF: [Labeled<ActivityAlert>; 3] = [
    option(
        "ui.settings.account.activity_and_banner",
        ActivityAlert::BannerAndFeed,
    ),
    option("ui.settings.account.activity_only", ActivityAlert::OnlyFeed),
    option("ui.settings.account.off", ActivityAlert::Off),
];

pub const MEETING_CHATS: [Labeled<MeetingChatNotifications>; 3] = [
    option(
        "ui.settings.account.meeting_mute",
        MeetingChatNotifications::Mute,
    ),
    option(
        "ui.settings.account.meeting_unmute",
        MeetingChatNotifications::Unmute,
    ),
    option(
        "ui.settings.account.meeting_mute_until_join",
        MeetingChatNotifications::MuteUntilJoin,
    ),
];

/// Catalog key for the short description under the preset selector.
pub fn preset_detail(preset: NotificationPreset) -> &'static str {
    match preset {
        NotificationPreset::AllActivity => "ui.settings.account.preset_all_detail",
        NotificationPreset::MentionsAndReplies => "ui.settings.account.preset_mentions_detail",
        NotificationPreset::Customised => "ui.settings.account.preset_custom_detail",
    }
}

/// Catalog key for the short description under the meeting chat selector.
pub fn meeting_chats_detail(setting: MeetingChatNotifications) -> &'static str {
    match setting {
        MeetingChatNotifications::Mute => "ui.settings.account.meeting_mute_detail",
        MeetingChatNotifications::Unmute => "ui.settings.account.meeting_unmute_detail",
        MeetingChatNotifications::MuteUntilJoin => {
            "ui.settings.account.meeting_mute_until_join_detail"
        }
    }
}

/// The catalog key of the selected option; empty when the value has no option.
pub fn label_of<T: PartialEq + Copy>(options: &[Labeled<T>], current: T) -> &'static str {
    options
        .iter()
        .find(|option| option.value == current)
        .map(|option| option.key)
        .unwrap_or_default()
}

pub fn account_select<T: PartialEq + Copy + 'static>(
    app: &Entity<AppState>,
    (id, title, detail): (
        &'static str,
        impl Into<SharedString>,
        impl Into<SharedString>,
    ),
    options: &'static [Labeled<T>],
    current: T,
    change: fn(T) -> ChatSettingChange,
) -> Div {
    let choices = options
        .iter()
        .map(|option| {
            let (app, value) = (app.clone(), option.value);
            Choice::new(tr(option.key), value == current, move |cx| {
                app.update(cx, |state, cx| state.save_chat_setting(change(value), cx))
            })
        })
        .collect();
    let current_key = label_of(options, current);
    let current_label: SharedString = if current_key.is_empty() {
        "".into()
    } else {
        tr(current_key).into()
    };
    control_row(title, detail, select(id, current_label, false, choices))
}

#[cfg(test)]
mod tests {
    use teams_core::{ActivityAlert, MeetingChatNotifications, MessageDensity, UpArrowBehavior};

    use super::{CHAT_MESSAGES, DENSITY, MEETING_CHATS, MENTIONS, UP_ARROW, label_of};

    #[test]
    fn every_value_has_a_catalog_key() {
        assert_eq!(
            label_of(&DENSITY, MessageDensity::Compact),
            "ui.settings.account.density_compact"
        );
        assert_eq!(
            label_of(&UP_ARROW, UpArrowBehavior::default()),
            "ui.settings.account.up_edit_last"
        );
        assert_eq!(
            label_of(&MEETING_CHATS, MeetingChatNotifications::default()),
            "ui.settings.account.meeting_mute_until_join"
        );
        assert_eq!(
            label_of(&CHAT_MESSAGES, ActivityAlert::OnlyBanner),
            "ui.settings.account.banner_only"
        );
        assert_eq!(label_of(&MENTIONS, ActivityAlert::Off), "");
    }
}
