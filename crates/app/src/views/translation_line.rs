use std::rc::Rc;

use gpui_kit::component::h_flex;
use gpui_kit::*;

use crate::i18n::tr;
use crate::rows::MessageRow;
use crate::theme;
use crate::translation::TranslationLine;
use app_base::ui_locale::tr_with;

const LINE_SIZE: f32 = 11.;
const LINE_GAP: f32 = 10.;

pub type LineAction = Rc<dyn Fn(&mut App)>;

#[derive(Clone)]
pub struct TranslationActions {
    pub translate: LineAction,
    pub never: Option<LineAction>,
}

fn line_link(id: ElementId, label: String, color: Hsla, action: LineAction) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .text_color(color)
        .hover(|link| link.underline())
        .child(label)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            action(cx)
        })
}

pub fn translation_line(
    row: &MessageRow,
    index: usize,
    actions: Option<&TranslationActions>,
) -> Option<AnyElement> {
    let line = row.translation.as_ref()?;
    let link_id = |name: &str| ElementId::Name(format!("translation-{name}-{index}").into());
    let base = h_flex()
        .gap(px(LINE_GAP))
        .text_size(px(LINE_SIZE))
        .text_color(theme::text_muted());
    let element = match line {
        TranslationLine::Offer { language, .. } => base
            .children(actions.map(|actions| {
                line_link(
                    link_id("translate"),
                    tr("ui.chat.translation.translate"),
                    theme::accent_text(),
                    actions.translate.clone(),
                )
            }))
            .children(actions.and_then(|actions| {
                actions.never.clone().map(|never| {
                    line_link(
                        link_id("never"),
                        tr_with("ui.chat.translation.never", &[("language", language)]),
                        theme::text_muted(),
                        never,
                    )
                })
            })),
        TranslationLine::Working => base.child(tr("ui.chat.translation.working")),
        TranslationLine::Failed => {
            base.child(tr("ui.chat.translation.failed"))
                .children(actions.map(|actions| {
                    line_link(
                        link_id("retry"),
                        tr("ui.chat.message.retry"),
                        theme::accent_text(),
                        actions.translate.clone(),
                    )
                }))
        }
        TranslationLine::Translated {
            language,
            showing_original,
        } => {
            let summary = match (language, showing_original) {
                (_, true) => tr("ui.chat.translation.showing_original"),
                (Some(language), false) => tr_with(
                    "ui.chat.translation.translated_from",
                    &[("language", language)],
                ),
                (None, false) => tr("ui.chat.translation.translated"),
            };
            let toggle = if *showing_original {
                tr("ui.chat.conversation.see_translation")
            } else {
                tr("ui.chat.conversation.see_original")
            };
            base.child(
                h_flex()
                    .gap(px(4.))
                    .child(summary)
                    .child("\u{b7}")
                    .children(actions.map(|actions| {
                        line_link(
                            link_id("toggle"),
                            toggle.clone(),
                            theme::accent_text(),
                            actions.translate.clone(),
                        )
                    })),
            )
        }
    };
    Some(element.into_any_element())
}
