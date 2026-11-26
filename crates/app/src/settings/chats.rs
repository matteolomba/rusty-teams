use gpui_kit::component::v_flex;
use gpui_kit::*;
use teams_core::{ChatSection, ChatSettingChange};

use super::account::{DENSITY, ENTER_KEY, account_select};
use super::rows::{caption, toggle_row};
use crate::app_state::AppState;
use crate::i18n::tr;
use crate::theme;

pub fn render(app: &Entity<AppState>, cx: &App) -> Div {
    let state = app.read(cx);
    let muted_section = state.directory.section_settings.enabled(ChatSection::Muted);
    let settings = state.account_chat_settings();
    let markdown_mode = state.markdown_mode;
    let actions_on_right_click = state.actions_on_right_click;
    let (section_app, preview_app, markdown_app, receipts_app, actions_app) = (
        app.clone(),
        app.clone(),
        app.clone(),
        app.clone(),
        app.clone(),
    );
    v_flex()
        .w_full()
        .child(caption(tr("ui.settings.chats.list_caption")))
        .child(
            toggle_row(
                "settings-muted-section",
                tr("settings.chats.muted_section"),
                tr("ui.settings.chats.account_detail"),
                theme::text_muted(),
                muted_section,
            )
            .on_click(move |_, _, cx| {
                section_app.update(cx, |state, cx| {
                    state.set_chat_section(ChatSection::Muted, !muted_section, cx)
                })
            }),
        )
        .child(
            toggle_row(
                "settings-show-message-preview",
                tr("settings.chats.message_preview"),
                tr("settings.chats.message_preview_detail"),
                theme::text_muted(),
                settings.show_message_preview,
            )
            .on_click(move |_, _, cx| {
                preview_app.update(cx, |state, cx| {
                    state.save_chat_setting(
                        ChatSettingChange::ShowMessagePreview(!settings.show_message_preview),
                        cx,
                    )
                })
            }),
        )
        .child(caption(tr("ui.settings.chats.caption")))
        .child(account_select(
            app,
            (
                "settings-message-density",
                tr("settings.chats.density"),
                tr("ui.settings.chats.density_detail"),
            ),
            &DENSITY,
            settings.message_density,
            ChatSettingChange::MessageDensity,
        ))
        .child(account_select(
            app,
            (
                "settings-enter-key",
                tr("settings.chats.enter_to"),
                tr("ui.settings.chats.enter_detail"),
            ),
            &ENTER_KEY,
            settings.enter_key,
            ChatSettingChange::EnterKey,
        ))
        .child(
            toggle_row(
                "settings-markdown",
                tr("settings.chats.markdown"),
                tr("settings.chats.markdown_detail"),
                theme::text_muted(),
                markdown_mode,
            )
            .on_click(move |_, _, cx| {
                markdown_app.update(cx, |state, cx| state.set_markdown_mode(!markdown_mode, cx))
            }),
        )
        .child(
            toggle_row(
                "settings-actions-on-right-click",
                tr("settings.chats.message_actions"),
                tr("ui.settings.chats.actions_detail"),
                theme::text_muted(),
                actions_on_right_click,
            )
            .on_click(move |_, _, cx| {
                actions_app.update(cx, |state, cx| {
                    state.set_actions_on_right_click(!actions_on_right_click, cx)
                })
            }),
        )
        .child(
            toggle_row(
                "settings-read-receipts",
                tr("settings.chats.read_receipts"),
                tr("ui.settings.chats.receipts_detail"),
                theme::text_muted(),
                settings.read_receipts,
            )
            .on_click(move |_, _, cx| {
                receipts_app.update(cx, |state, cx| {
                    state.save_chat_setting(
                        ChatSettingChange::ReadReceipts(!settings.read_receipts),
                        cx,
                    )
                })
            }),
        )
}
