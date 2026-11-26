use gpui_kit::component::v_flex;
use gpui_kit::*;

use crate::data::Directory;
use crate::i18n::tr;
use crate::rows::StartInfo;
use crate::theme;
use crate::views::avatar::person_avatar;

const NOTES_SPACE_TITLE: &str = "ui.chat.notes.title";
const NOTES_SPACE_TEXT: &str = "ui.chat.notes.text";
const NOTES_SPACE_AVATAR_SIZE: f32 = 72.;
const NOTES_SPACE_TEXT_WIDTH: f32 = 360.;

pub(super) fn render_notes_space(directory: &Directory, start: &StartInfo) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .px(px(16.))
        .child(person_avatar(
            directory,
            start.user_id.as_deref(),
            &start.avatar_name,
            NOTES_SPACE_AVATAR_SIZE,
        ))
        .child(
            v_flex()
                .items_center()
                .gap(px(4.))
                .max_w(px(NOTES_SPACE_TEXT_WIDTH))
                .child(
                    div()
                        .text_size(theme::text_heading())
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text())
                        .child(tr(NOTES_SPACE_TITLE)),
                )
                .child(
                    div()
                        .text_center()
                        .text_size(theme::text_label())
                        .text_color(theme::text_muted())
                        .child(tr(NOTES_SPACE_TEXT)),
                ),
        )
        .into_any_element()
}
