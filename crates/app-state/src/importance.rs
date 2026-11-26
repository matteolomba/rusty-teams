use app_base::ui_locale::{tr, tr_with};
use store::MessageImportance;

/// Teams strings "Important!" / "Urgent!", shown in capitals (`text-transform: uppercase`).
pub const IMPORTANT_LABEL: &str = "ui.chat.importance.important";
pub const URGENT_LABEL: &str = "ui.chat.importance.urgent";
pub const URGENT_TOAST_SUFFIX: &str = "ui.chat.importance.urgent_toast_suffix";

pub fn importance_label(importance: MessageImportance) -> Option<String> {
    match importance {
        MessageImportance::Normal => None,
        MessageImportance::High => Some(tr(IMPORTANT_LABEL)),
        MessageImportance::Urgent => Some(tr(URGENT_LABEL)),
    }
}

pub fn urgent_toast_title(sender: &str) -> String {
    tr_with(URGENT_TOAST_SUFFIX, &[("sender", sender)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_the_teams_strings_in_capitals() {
        app_base::ui_locale::apply_locale("en");
        assert_eq!(importance_label(MessageImportance::Normal), None);
        assert_eq!(
            importance_label(MessageImportance::High).as_deref(),
            Some("IMPORTANT!")
        );
        assert_eq!(
            importance_label(MessageImportance::Urgent).as_deref(),
            Some("URGENT!")
        );
    }

    #[test]
    fn the_urgent_toast_title_names_the_sender() {
        app_base::ui_locale::apply_locale("en");
        assert_eq!(
            urgent_toast_title("Lea Schneider"),
            "Lea Schneider sent an urgent message"
        );
    }
}
