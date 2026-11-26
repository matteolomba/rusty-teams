//! User-facing labels for enums owned by crates that cannot depend on the UI locale layer.

use calling::NoiseSuppression;
use chatsvc::sounds::{AlertSound, Ringtone, RingtoneCallType};

use crate::i18n::tr;
use crate::notify::rules::Corner;

pub(crate) fn noise_suppression_label(level: NoiseSuppression) -> String {
    tr(match level {
        NoiseSuppression::Auto => "ui.settings.devices.noise_auto",
        NoiseSuppression::Low => "ui.settings.devices.noise_low",
        NoiseSuppression::Off => "ui.settings.devices.noise_off",
    })
}

pub(crate) fn ringtone_label(ringtone: Ringtone) -> String {
    match ringtone {
        Ringtone::Ring => tr("ui.settings.calls.ringtone_default"),
        Ringtone::BubblesLoud => tr("ui.settings.calls.ringtone_bubbles_loud"),
        _ => ringtone.label().to_owned(),
    }
}

pub(crate) fn ringtone_call_type_label(call_type: RingtoneCallType) -> String {
    tr(match call_type {
        RingtoneCallType::Regular => "ui.settings.calls.call_type_regular",
        RingtoneCallType::CallGroup => "ui.settings.calls.call_type_group",
        RingtoneCallType::CallQueue => "ui.settings.calls.call_type_queue",
        RingtoneCallType::Delegate => "ui.settings.calls.call_type_delegate",
    })
}

pub(crate) fn alert_sound_label(sound: AlertSound) -> String {
    match sound {
        AlertSound::Default | AlertSound::DefaultUrgent => {
            tr("ui.settings.notifications.sound_default")
        }
        _ => sound.name().to_owned(),
    }
}

pub(crate) fn corner_label(corner: Corner) -> String {
    tr(match corner {
        Corner::TopLeft => "ui.settings.notifications.corner_top_left",
        Corner::TopRight => "ui.settings.notifications.corner_top_right",
        Corner::BottomLeft => "ui.settings.notifications.corner_bottom_left",
        Corner::BottomRight => "ui.settings.notifications.corner_bottom_right",
    })
}
