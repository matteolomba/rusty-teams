use app_base::updater::UpdateMode;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::Button,
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use teams_core::{ChatSettingChange, TranslationBehavior};

use super::account::{UP_ARROW, account_select};
use super::rows::{Choice, caption, control_row, select, text_button, toggle_row};
use crate::app_state::AppState;
use crate::i18n::{tr, tr_args};
use crate::notify::NotificationCenter;
use crate::notify::rules::Settings;
use crate::theme;
use crate::views::widgets::icon;

const CHIP_HEIGHT: f32 = 24.;
const DISABLED_OPACITY: f32 = 0.4;
const LANGUAGE_MENU_HEIGHT: f32 = 320.;

fn startup_rows(center: &Entity<NotificationCenter>, cx: &App) -> [Stateful<Div>; 2] {
    let startup = center.read(cx).startup();
    let (auto_start_center, background_center) = (center.clone(), center.clone());
    [
        toggle_row(
            "settings-auto-start",
            tr("settings.general.auto_start"),
            "",
            theme::text_muted(),
            startup.auto_start,
        )
        .on_click(move |_, _, cx| {
            auto_start_center.update(cx, |center, cx| {
                center.set_auto_start(!startup.auto_start, cx)
            })
        }),
        toggle_row(
            "settings-open-in-background",
            tr("settings.general.open_in_background"),
            "",
            theme::text_muted(),
            startup.open_in_background,
        )
        .when(!startup.auto_start, |row| row.opacity(DISABLED_OPACITY))
        .when(startup.auto_start, |row| {
            row.on_click(move |_, _, cx| {
                background_center.update(cx, |center, cx| {
                    center.set_open_in_background(!startup.open_in_background, cx)
                })
            })
        }),
    ]
}

pub fn render(center: &Entity<NotificationCenter>, app: &Entity<AppState>, cx: &App) -> Div {
    let settings = center.read(cx).settings();
    let agents_app = app.clone();
    let agents_enabled = app.read(cx).agents_mcp_enabled;
    let agents_row = toggle_row(
        "settings-agents-mcp",
        tr("settings.general.agents_mcp"),
        tr("settings.general.agents_mcp_detail"),
        theme::text_muted(),
        agents_enabled,
    )
    .on_click(move |_, _, cx| {
        agents_app.update(cx, |state, cx| {
            state.set_agents_mcp_enabled(!agents_enabled, cx)
        })
    });
    v_flex()
        .w_full()
        .child(caption(tr("settings.caption.system")))
        .children(startup_rows(center, cx))
        .child(agents_row)
        .child(settings_toggle(
            center,
            (
                "settings-show-in-tray",
                tr("settings.general.show_in_tray"),
                tr("settings.general.show_in_tray_detail"),
            ),
            settings.show_in_tray,
            |settings| settings.show_in_tray = !settings.show_in_tray,
        ))
        .when(settings.show_in_tray, |rows| {
            rows.child(settings_toggle(
                center,
                (
                    "settings-close-to-tray",
                    tr("settings.general.close_to_tray"),
                    tr("settings.general.close_to_tray_detail"),
                ),
                settings.close_to_tray,
                |settings| settings.close_to_tray = !settings.close_to_tray,
            ))
        })
        .children(external_browser_rows(app, cx))
        .children(update_rows(app, cx))
        .child(caption(tr("settings.caption.compose_box")))
        .child(account_select(
            app,
            (
                "settings-up-arrow",
                tr("settings.general.up_arrow"),
                tr("settings.general.up_arrow_detail"),
            ),
            &UP_ARROW,
            app.read(cx).account_chat_settings().up_arrow,
            ChatSettingChange::UpArrow,
        ))
        .child(caption(tr("settings.caption.translation")))
        .children(translation_rows(app, cx))
}

fn settings_toggle(
    center: &Entity<NotificationCenter>,
    (id, title, detail): (&'static str, String, String),
    enabled: bool,
    change: fn(&mut Settings),
) -> Stateful<Div> {
    let center = center.clone();
    toggle_row(id, title, detail, theme::text_muted(), enabled).on_click(move |_, _, cx| {
        center.update(cx, |center, cx| center.update_settings(cx, change))
    })
}

/// Linux only: the application menu install and the install question.
fn install_rows(cx: &App) -> Vec<AnyElement> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cx;
        Vec::new()
    }
    #[cfg(target_os = "linux")]
    {
        let folder = linux_install::install_folder();
        let installation = linux_install::installation();
        let detail = match installation {
            Installation::Running => tr_with("install.installed_detail", "folder", folder),
            Installation::OtherCopy => tr_with("install.other_copy_detail", "folder", folder),
            Installation::NotInstalled => tr("install.not_installed_detail"),
        };
        let mut buttons = h_flex().gap(px(8.)).flex_none();
        if installation != Installation::Running {
            let label = match installation {
                Installation::OtherCopy => tr("install.replace"),
                _ => tr("install.move"),
            };
            buttons = buttons.child(
                text_button("settings-install", label)
                    .on_click(|_, _, cx| linux_install::install_now(cx)),
            );
        }
        if installation != Installation::NotInstalled {
            buttons = buttons.child(
                text_button("settings-install-remove", tr("install.remove"))
                    .on_click(|_, _, cx| linux_install::remove(cx)),
            );
        }
        let mut rows =
            vec![control_row(tr("install.menu_title"), detail, buttons).into_any_element()];
        if installation != Installation::Running {
            let asks = linux_install::asks_on_start(cx);
            rows.push(
                toggle_row(
                    "settings-install-ask",
                    tr("install.ask_on_start"),
                    "",
                    theme::text_muted(),
                    asks,
                )
                .on_click(move |_, _, cx| linux_install::set_asks_on_start(!asks, cx))
                .into_any_element(),
            );
        }
        rows
    }
}

fn tr_with(key: &str, name: &str, value: impl Into<String>) -> String {
    let mut args = std::collections::HashMap::new();
    args.insert(name.to_owned(), value.into());
    tr_args(key, &args)
}

#[cfg(any(windows, target_os = "linux"))]
fn update_mode_label(mode: UpdateMode) -> String {
    match mode {
        UpdateMode::Off => tr("install.update_mode_off"),
        UpdateMode::Manual => tr("install.update_mode_manual"),
        UpdateMode::Automatic => tr("install.update_mode_automatic"),
        UpdateMode::ReleasePage => tr("install.update_mode_release_page"),
    }
}

/// Updates section shown on Windows and Linux. Only `off` disables the check button;
/// `release-page` keeps it active but it opens the fork instead of downloading.
#[cfg(any(windows, target_os = "linux"))]
fn update_rows(app: &Entity<AppState>, cx: &App) -> Vec<AnyElement> {
    let mode = crate::updater::load_update_mode(&app.read(cx).store);
    let choices = UpdateMode::ALL
        .into_iter()
        .map(|choice| {
            let choice_app = app.clone();
            Choice::new(update_mode_label(choice), choice == mode, move |cx| {
                if let Err(error) =
                    crate::updater::save_update_mode(&choice_app.read(cx).store, choice)
                {
                    app_base::diagnostics::record(format!("update mode not saved: {error}"));
                }
                cx.refresh_windows();
            })
        })
        .collect();
    #[cfg(target_os = "linux")]
    let version = crate::linux_install::running_release().to_owned();
    #[cfg(windows)]
    let version = app_base::version::running_release_tag().to_owned();
    #[cfg(target_os = "linux")]
    let detail = if crate::linux_install::updatable_release().is_some() {
        tr_with("install.updates_detail", "version", version)
    } else {
        tr_with("install.updates_detail_not_installed", "version", version)
    };
    #[cfg(windows)]
    let detail = tr_with("install.updates_detail", "version", version);
    let button_label = if mode == UpdateMode::ReleasePage {
        tr("install.open_releases")
    } else {
        tr("install.check_now")
    };
    let check = text_button("settings-check-updates", button_label)
        .when(mode == UpdateMode::Off, |button| {
            button.opacity(DISABLED_OPACITY)
        })
        .when(mode != UpdateMode::Off, |button| {
            let check_app = app.clone();
            button.on_click(move |_, _, cx| check_now(&check_app, cx))
        });
    vec![
        caption(tr("settings.caption.updates")).into_any_element(),
        control_row(
            tr("settings.general.update_mode"),
            tr("settings.general.update_mode_detail"),
            select(
                "settings-update-mode",
                update_mode_label(mode),
                false,
                choices,
            ),
        )
        .into_any_element(),
        control_row(tr("install.updates_title"), detail, check).into_any_element(),
    ]
}

#[cfg(not(any(windows, target_os = "linux")))]
fn update_rows(app: &Entity<AppState>, cx: &App) -> Vec<AnyElement> {
    let _ = (app, cx);
    Vec::new()
}

#[cfg(any(windows, target_os = "linux"))]
fn check_now(app: &Entity<AppState>, cx: &mut App) {
    let mode = crate::updater::load_update_mode(&app.read(cx).store);
    if mode == UpdateMode::ReleasePage {
        open_release_page(cx);
        return;
    }
    if !mode.downloads() {
        return;
    }
    #[cfg(target_os = "linux")]
    crate::linux_update::check(true, cx);
    #[cfg(windows)]
    {
        let Some(running_tag) = app_base::version::release_tag() else {
            notice(tr("install.updates_release_only"), cx);
            return;
        };
        let Some(paths) = crate::updater::current_paths() else {
            return;
        };
        let fetch = app_base::release_feed::fetch_update(running_tag, paths, cx);
        cx.spawn(async move |cx| match fetch.await {
            Ok(Some(tag)) => {
                log::info!("release {tag} scaricata per il prossimo riavvio");
                cx.update(|cx| notice(tr("install.update_downloaded"), cx));
            }
            Ok(None) => cx.update(|cx| notice(tr("install.up_to_date"), cx)),
            Err(error) => {
                cx.update(|cx| notice(tr_with("install.update_failed", "detail", error), cx))
            }
        })
        .detach();
    }
}

/// `release-page` mode only looks up the newest tag and opens its page: no download.
#[cfg(any(windows, target_os = "linux"))]
fn open_release_page(cx: &mut App) {
    let running_tag = app_base::version::running_release_tag();
    let find = app_base::release_feed::find_update(running_tag, cx);
    cx.spawn(async move |cx| match find.await {
        Ok(update) => {
            let url = release_page_for(update.as_ref().map(|update| update.tag.as_str()));
            cx.update(|cx| cx.open_url(&url));
        }
        Err(error) => cx.update(|cx| notice(tr_with("install.update_failed", "detail", error), cx)),
    })
    .detach();
}

#[cfg(any(windows, target_os = "linux"))]
fn release_page_for(tag: Option<&str>) -> String {
    match tag {
        Some(tag) => app_base::release_feed::release_page_url(tag),
        None => app_base::release_feed::RELEASES_PAGE_URL.to_owned(),
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn notice(text: String, cx: &mut App) {
    if let Some(handle) = cx.try_global::<crate::app_state::AppHandle>() {
        handle
            .0
            .clone()
            .update(cx, |state, cx| state.raise_notice(text, None, cx));
    }
}

fn external_browser_rows(app: &Entity<AppState>, cx: &App) -> Vec<AnyElement> {
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (app, cx);
        return Vec::new();
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use crate::external_browser::{self, ExternalBrowser};

        let selected = external_browser::load(&app.read(cx).store);
        let current_label = external_browser::available_choices()
            .iter()
            .find(|choice| choice.id == selected.id())
            .map(|choice| {
                if external_browser::is_system_id(&choice.id) {
                    tr("settings.general.external_browser_system")
                } else {
                    choice.label.clone()
                }
            })
            .unwrap_or_else(|| tr("settings.general.external_browser_system"));
        let choices: Vec<Choice> = external_browser::available_choices()
            .iter()
            .map(|choice| {
                let label = if external_browser::is_system_id(&choice.id) {
                    tr("settings.general.external_browser_system")
                } else {
                    choice.label.clone()
                };
                let is_selected = selected.id() == choice.id;
                let (app, id) = (app.clone(), choice.id.clone());
                Choice::new(label, is_selected, move |cx| {
                    app.update(cx, |state, cx| {
                        let browser = if external_browser::is_system_id(&id) {
                            ExternalBrowser::System
                        } else {
                            ExternalBrowser::Program(id.clone())
                        };
                        external_browser::save(&state.store, &browser);
                        cx.notify();
                    });
                })
            })
            .collect();
        vec![
            caption(tr("settings.caption.browser")).into_any_element(),
            sign_in_browser_row(),
            control_row(
                tr("settings.general.external_browser"),
                tr("settings.general.external_browser_detail"),
                select("settings-external-browser", current_label, false, choices),
            )
            .into_any_element(),
        ]
    }
}

/// The browser the Teams session runs in. Changing it restarts the app, which stops the old
/// browser on the way out and starts the new one.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sign_in_browser_row() -> AnyElement {
    use std::sync::LazyLock;

    use browser::{InstalledBrowser, SignInBrowser};

    static INSTALLED: LazyLock<Vec<InstalledBrowser>> = LazyLock::new(browser::installed_browsers);
    static AUTOMATIC: LazyLock<Option<String>> = LazyLock::new(|| {
        browser::automatic_browser(&INSTALLED).map(|browser| browser.name.clone())
    });

    let saved = SignInBrowser::load();
    let automatic_label = match AUTOMATIC.as_deref() {
        Some(name) => format!(
            "{} ({name})",
            tr("settings.general.sign_in_browser_automatic")
        ),
        None => tr("settings.general.sign_in_browser_automatic"),
    };
    let options = std::iter::once((SignInBrowser::Automatic, automatic_label)).chain(
        INSTALLED.iter().map(|browser| {
            (
                SignInBrowser::Program(browser.path.clone()),
                browser.name.clone(),
            )
        }),
    );
    let mut current_label = None;
    let choices: Vec<Choice> = options
        .map(|(option, label)| {
            let is_selected = option == saved;
            if is_selected {
                current_label = Some(label.clone());
            }
            Choice::new(label, is_selected, move |cx| {
                if is_selected {
                    return;
                }
                match option.save() {
                    Ok(()) => restart_app(cx),
                    Err(error) => {
                        app_base::diagnostics::record(format!("sign-in browser not saved: {error}"))
                    }
                }
            })
        })
        .collect();
    let current_label =
        current_label.unwrap_or_else(|| tr("settings.general.sign_in_browser_automatic"));
    control_row(
        tr("settings.general.sign_in_browser"),
        tr("settings.general.sign_in_browser_detail"),
        select("settings-sign-in-browser", current_label, false, choices),
    )
    .into_any_element()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn restart_app(cx: &mut App) {
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(
                app_base::updater::RELAUNCH_ENV,
                std::process::id().to_string(),
            )
            .spawn()
    });
    match spawned {
        Ok(_) => cx.quit(),
        Err(error) => app_base::diagnostics::record(format!(
            "restart for the sign-in browser failed: {error}"
        )),
    }
}

fn translation_rows(app: &Entity<AppState>, cx: &App) -> Vec<AnyElement> {
    let state = app.read(cx);
    if state.translation.settings.is_none() {
        return vec![
            control_row(
                tr("settings.general.translation_title"),
                tr("settings.general.translation_not_loaded"),
                div(),
            )
            .into_any_element(),
        ];
    }
    let translation = &state.translation;
    let target = translation.target();
    let behavior = translation.behavior();
    let targets = translation
        .languages
        .iter()
        .map(|language| {
            let (app, code) = (app.clone(), language.code.clone());
            Choice::new(language.name.clone(), language.code == target, move |cx| {
                app.update(cx, |state, cx| state.set_translation_target(&code, cx))
            })
        })
        .collect();
    let behavior_options = [
        (
            tr("settings.general.behavior_ask"),
            TranslationBehavior::Ask,
        ),
        (
            tr("settings.general.behavior_auto"),
            TranslationBehavior::Auto,
        ),
        (
            tr("settings.general.behavior_never"),
            TranslationBehavior::Never,
        ),
    ];
    let behavior_label = behavior_options
        .iter()
        .find(|(_, choice)| *choice == behavior)
        .map_or(String::new(), |(label, _)| label.clone());
    let behaviors = behavior_options
        .into_iter()
        .map(|(label, choice)| {
            let app = app.clone();
            Choice::new(label, behavior == choice, move |cx| {
                app.update(cx, |state, cx| state.set_translation_behavior(choice, cx))
            })
        })
        .collect();
    vec![
        control_row(
            tr("settings.general.translate_into"),
            "Read from your Teams account",
            select(
                "settings-translation-target",
                translation.language_name(&target),
                false,
                targets,
            ),
        )
        .into_any_element(),
        control_row(
            tr("settings.general.behavior_title"),
            "",
            select(
                "settings-translation-behavior",
                behavior_label,
                false,
                behaviors,
            ),
        )
        .into_any_element(),
        control_row(
            tr("settings.general.never_translate"),
            "",
            never_chips(app, &target, cx),
        )
        .into_any_element(),
    ]
}

fn never_chips(app: &Entity<AppState>, target: &str, cx: &App) -> Div {
    let translation = &app.read(cx).translation;
    let never: Vec<String> = translation
        .display_codes()
        .into_iter()
        .filter(|code| code != target)
        .collect();
    let chips = never.iter().map(|code| {
        let (remove_app, remove_code) = (app.clone(), code.clone());
        h_flex()
            .h(px(CHIP_HEIGHT))
            .pl(px(8.))
            .gap(px(4.))
            .items_center()
            .rounded(px(6.))
            .bg(theme::surface_raised())
            .text_size(theme::text_small())
            .text_color(theme::text_strong())
            .child(translation.language_name(code))
            .child(
                div()
                    .id(SharedString::from(format!("settings-never-remove-{code}")))
                    .w(px(16.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(icon(IconName::X, 10., theme::text_faint()))
                    .on_click(move |_, _, cx| {
                        remove_app.update(cx, |state, cx| {
                            state.allow_translating_language(&remove_code, cx)
                        })
                    }),
            )
    });
    let addable = translation
        .languages
        .iter()
        .filter(|language| language.code != target && !never.contains(&language.code))
        .map(|language| {
            let (app, code) = (app.clone(), language.code.clone());
            Choice::new(language.name.clone(), false, move |cx| {
                app.update(cx, |state, cx| state.never_translate_language(&code, cx))
            })
        })
        .collect();
    h_flex()
        .flex_wrap()
        .justify_end()
        .gap(px(6.))
        .items_center()
        .children(chips)
        .child(add_language_button(addable))
}

fn add_language_button(choices: Vec<Choice>) -> impl IntoElement {
    Button::new("settings-never-add")
        .outline()
        .h(px(CHIP_HEIGHT))
        .px(px(8.))
        .rounded(px(6.))
        .text_size(theme::text_small())
        .text_color(theme::accent_text())
        .label(format!("+ {}", tr("common.add")))
        .dropdown_menu(move |menu, _, _| {
            choices.iter().fold(
                menu.max_h(px(LANGUAGE_MENU_HEIGHT)).scrollable(true),
                |menu, choice| {
                    let choose = choice.choose.clone();
                    menu.item(
                        PopupMenuItem::new(choice.label.clone())
                            .on_click(move |_, _, cx| choose(cx)),
                    )
                },
            )
        })
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::release_page_for;

    #[test]
    fn release_page_uses_the_found_tag() {
        assert_eq!(
            release_page_for(Some("v0.7.0-rc.1")),
            "https://github.com/matteolomba/rusty-teams/releases/tag/v0.7.0-rc.1"
        );
        assert_eq!(
            release_page_for(None),
            "https://github.com/matteolomba/rusty-teams/releases"
        );
    }

    #[test]
    fn update_mode_labels_are_localized() {
        use app_base::ui_locale::apply_locale;
        use app_base::updater::UpdateMode;

        use super::update_mode_label;
        use crate::i18n::locale_test_lock;

        let _guard = locale_test_lock();
        apply_locale("it");
        assert!(update_mode_label(UpdateMode::Manual).contains("Manual"));
        assert!(update_mode_label(UpdateMode::ReleasePage).contains("nessun download"));
        apply_locale("de");
        assert!(update_mode_label(UpdateMode::Automatic).contains("Automatisch"));
        apply_locale("en");
        assert!(update_mode_label(UpdateMode::Off).contains("never check"));
        apply_locale("en");
    }
}
