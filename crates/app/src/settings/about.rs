use gpui_kit::component::v_flex;
use gpui_kit::*;

use super::rows::{caption, control_row, text_button};
use crate::{format, i18n::tr, updater};

pub fn render() -> Div {
    let build_id = updater::running_version();
    let copied = build_id.to_owned();
    v_flex()
        .w_full()
        .child(caption("RUSTY TEAMS"))
        .child(control_row(
            tr("ui.settings.about.version"),
            format::short_version(build_id, env!("CARGO_PKG_VERSION")),
            div(),
        ))
        .child(control_row(
            tr("ui.settings.about.build"),
            build_id.to_owned(),
            text_button("settings-copy-build", tr("ui.settings.common.copy")).on_click(
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone())),
            ),
        ))
}
