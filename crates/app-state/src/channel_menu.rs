use std::future::Future;

use gpui_kit::*;

use crate::app_state::AppState;
use crate::sidebar_model::channel_shown;
use crate::team_admin::ChannelAdmin;
use app_base::ui_locale::{tr, tr_with};

pub const COPIED_TO_CLIPBOARD: &str = "ui.chat.channel_menu.copied";
pub const PINNED_CHANNEL_CANNOT_HIDE: &str = "ui.chat.channel_menu.pinned_cannot_hide";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMenuItem {
    Notifications,
    Pin,
    Unpin,
    Show,
    Hide,
    EditChannel,
    ManageChannel,
    CopyLink,
    DeleteChannel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamMenuItem {
    AddChannel,
    Show,
    Hide,
    ManageTeam,
    CopyLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEntry<Item> {
    Item(Item),
    Separator,
}

impl ChannelMenuItem {
    pub fn label(self) -> &'static str {
        match self {
            ChannelMenuItem::Notifications => "ui.chat.channel_menu.notifications",
            ChannelMenuItem::Pin => "ui.chat.channel_menu.pin",
            ChannelMenuItem::Unpin => "ui.chat.channel_menu.unpin",
            ChannelMenuItem::Show => "ui.chat.channel_menu.show",
            ChannelMenuItem::Hide => "ui.chat.channel_menu.hide",
            ChannelMenuItem::EditChannel => "ui.chat.channel_menu.edit",
            ChannelMenuItem::ManageChannel => "ui.chat.channel_menu.manage",
            ChannelMenuItem::CopyLink => "ui.chat.channel_menu.copy_link",
            ChannelMenuItem::DeleteChannel => "ui.chat.channel_menu.delete",
        }
    }
}

impl TeamMenuItem {
    pub fn label(self) -> &'static str {
        match self {
            TeamMenuItem::AddChannel => "ui.chat.channel_menu.add_channel",
            TeamMenuItem::Show => "ui.chat.channel_menu.show",
            TeamMenuItem::Hide => "ui.chat.channel_menu.hide",
            TeamMenuItem::ManageTeam => "ui.chat.channel_menu.manage_team",
            TeamMenuItem::CopyLink => "ui.chat.channel_menu.copy_link",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkUse {
    Copy,
    OpenInBrowser,
}

impl LinkUse {
    fn failure(self, reason: &str) -> String {
        match self {
            LinkUse::Copy => tr_with("ui.chat.channel_menu.copy_failed", &[("reason", reason)]),
            LinkUse::OpenInBrowser => {
                tr_with("ui.chat.channel_menu.open_failed", &[("reason", reason)])
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelMenuState {
    pub pinned: bool,
    pub shown: bool,
    pub admin: ChannelAdmin,
}

impl ChannelMenuState {
    pub fn disables(self, item: ChannelMenuItem) -> bool {
        item == ChannelMenuItem::Hide && self.pinned
    }
}

pub fn channel_menu_entries(state: ChannelMenuState) -> Vec<MenuEntry<ChannelMenuItem>> {
    let mut entries = vec![
        MenuEntry::Item(ChannelMenuItem::Notifications),
        MenuEntry::Item(if state.pinned {
            ChannelMenuItem::Unpin
        } else {
            ChannelMenuItem::Pin
        }),
        MenuEntry::Item(if state.shown {
            ChannelMenuItem::Hide
        } else {
            ChannelMenuItem::Show
        }),
        MenuEntry::Separator,
    ];
    if state.admin.can_edit {
        entries.push(MenuEntry::Item(ChannelMenuItem::EditChannel));
    }
    entries.extend([
        MenuEntry::Item(ChannelMenuItem::ManageChannel),
        MenuEntry::Item(ChannelMenuItem::CopyLink),
    ]);
    if state.admin.can_delete {
        entries.extend([
            MenuEntry::Separator,
            MenuEntry::Item(ChannelMenuItem::DeleteChannel),
        ]);
    }
    entries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TeamMenuState {
    pub hidden: bool,
    pub can_add_channel: bool,
}

pub fn team_menu_entries(state: TeamMenuState) -> Vec<MenuEntry<TeamMenuItem>> {
    let mut entries = Vec::new();
    if state.can_add_channel {
        entries.push(MenuEntry::Item(TeamMenuItem::AddChannel));
    }
    entries.extend([
        MenuEntry::Item(if state.hidden {
            TeamMenuItem::Show
        } else {
            TeamMenuItem::Hide
        }),
        MenuEntry::Separator,
        MenuEntry::Item(TeamMenuItem::ManageTeam),
        MenuEntry::Item(TeamMenuItem::CopyLink),
    ]);
    entries
}

pub fn pin_failure(pinned: bool) -> &'static str {
    if pinned {
        "ui.chat.channel_menu.pin_channel_failed"
    } else {
        "ui.chat.channel_menu.unpin_channel_failed"
    }
}

pub fn channel_visibility_failure(shown: bool) -> &'static str {
    if shown {
        "ui.chat.channel_menu.show_channel_failed"
    } else {
        "ui.chat.channel_menu.hide_channel_failed"
    }
}

pub fn team_visibility_failure(shown: bool) -> &'static str {
    if shown {
        "ui.chat.channel_menu.show_team_failed"
    } else {
        "ui.chat.channel_menu.hide_team_failed"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostPinDialog {
    pub title: &'static str,
    pub body: &'static str,
    pub confirm: &'static str,
}

pub fn post_pin_dialog(pin: bool) -> PostPinDialog {
    if pin {
        PostPinDialog {
            title: "ui.chat.channel_menu.pin_dialog_title",
            body: "ui.chat.channel_menu.pin_dialog_body",
            confirm: "ui.chat.channel_menu.pin",
        }
    } else {
        PostPinDialog {
            title: "ui.chat.channel_menu.unpin_dialog_title",
            body: "ui.chat.channel_menu.unpin_dialog_body",
            confirm: "ui.chat.channel_menu.unpin",
        }
    }
}

pub fn post_pin_notice(pin: bool) -> &'static str {
    if pin {
        "ui.chat.channel_menu.post_pinned"
    } else {
        "ui.chat.channel_menu.post_unpinned"
    }
}

pub fn post_pin_failure(pin: bool) -> &'static str {
    if pin {
        "ui.chat.channel_menu.pin_post_failed"
    } else {
        "ui.chat.channel_menu.unpin_post_failed"
    }
}

impl AppState {
    pub fn channel_menu_state(&self, channel_id: &str) -> ChannelMenuState {
        ChannelMenuState {
            pinned: self.channel_pinned(channel_id),
            shown: self
                .sidebar
                .teams
                .iter()
                .find(|team| team.channels.iter().any(|channel| channel.id == channel_id))
                .is_none_or(|team| channel_shown(team, channel_id)),
            admin: self.channel_admin(channel_id),
        }
    }

    pub fn channel_pinned(&self, channel_id: &str) -> bool {
        self.directory
            .pinned_channels
            .iter()
            .any(|id| id == channel_id)
    }

    pub fn set_channel_pinned(&mut self, channel_id: &str, pinned: bool, cx: &mut Context<Self>) {
        if self.channel_pinned(channel_id) == pinned {
            return;
        }
        let Some(engine_or_demo) = self.chat_action_engine(cx) else {
            return;
        };
        let previous = self.directory.pinned_channels.clone();
        let mut changed = previous.clone();
        changed.retain(|id| id != channel_id);
        if pinned {
            changed.push(channel_id.to_owned());
        }
        self.apply_pinned_channels(changed, cx);
        let Some(engine) = engine_or_demo else {
            return;
        };
        let owned_id = channel_id.to_owned();
        self.run_channel_action(
            pin_failure(pinned),
            async move { engine.set_channel_pinned(&owned_id, pinned).await },
            move |state, cx| state.apply_pinned_channels(previous, cx),
            None,
            cx,
        );
    }

    fn apply_pinned_channels(&mut self, channel_ids: Vec<String>, cx: &mut Context<Self>) {
        let _ = self.store.replace_pinned_channels(&channel_ids);
        self.directory.pinned_channels = channel_ids;
        cx.emit(crate::app_state::AppEvent::Sidebar);
        cx.notify();
    }

    pub fn set_channel_shown(&mut self, channel_id: &str, shown: bool, cx: &mut Context<Self>) {
        if !shown && self.channel_pinned(channel_id) {
            self.raise_notice(tr(PINNED_CHANNEL_CANNOT_HIDE), None, cx);
            return;
        }
        let Some(engine_or_demo) = self.chat_action_engine(cx) else {
            return;
        };
        let _ = self.store.set_channel_hidden(channel_id, !shown);
        self.reload_sidebar(cx);
        let Some(engine) = engine_or_demo else {
            return;
        };
        let owned_id = channel_id.to_owned();
        let revert_id = owned_id.clone();
        self.run_channel_action(
            channel_visibility_failure(shown),
            async move { engine.set_channel_shown(&owned_id, shown).await },
            move |state, cx| {
                let _ = state.store.set_channel_hidden(&revert_id, shown);
                state.reload_sidebar(cx);
            },
            None,
            cx,
        );
    }

    pub fn set_team_shown(&mut self, team_id: &str, shown: bool, cx: &mut Context<Self>) {
        let Some(engine_or_demo) = self.chat_action_engine(cx) else {
            return;
        };
        let _ = self.store.set_team_hidden(team_id, !shown);
        self.reload_sidebar(cx);
        let Some(engine) = engine_or_demo else {
            return;
        };
        let owned_id = team_id.to_owned();
        let revert_id = owned_id.clone();
        self.run_channel_action(
            team_visibility_failure(shown),
            async move { engine.set_team_shown(&owned_id, shown).await },
            move |state, cx| {
                let _ = state.store.set_team_hidden(&revert_id, shown);
                state.reload_sidebar(cx);
            },
            None,
            cx,
        );
    }

    pub fn copy_channel_link(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        self.with_channel_link(channel_id, LinkUse::Copy, cx);
    }

    pub fn manage_channel_in_browser(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        self.with_channel_link(channel_id, LinkUse::OpenInBrowser, cx);
    }

    fn with_channel_link(&mut self, channel_id: &str, link_use: LinkUse, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            if self.mode.demo {
                let link = teams_core::channel_link(&teams_core::ChannelAddress {
                    tenant_id: crate::demo::DEMO_TENANT_ID.to_owned(),
                    group_id: self.channel_team_id(channel_id).unwrap_or_default(),
                    channel_id: channel_id.to_owned(),
                    channel_name: self.channel_name(channel_id).unwrap_or_default(),
                    membership_type: None,
                });
                self.use_link(link, link_use, cx);
            }
            return;
        };
        let owned_id = channel_id.to_owned();
        self.link_from(
            async move { engine.channel_link(&owned_id).await },
            link_use,
            cx,
        );
    }

    pub fn copy_team_link(&mut self, team_id: &str, cx: &mut Context<Self>) {
        self.with_team_link(team_id, LinkUse::Copy, cx);
    }

    pub fn manage_team_in_browser(&mut self, team_id: &str, cx: &mut Context<Self>) {
        self.with_team_link(team_id, LinkUse::OpenInBrowser, cx);
    }

    fn with_team_link(&mut self, team_id: &str, link_use: LinkUse, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            if self.mode.demo {
                let team_thread_id = self.store.team_thread_id(team_id).ok().flatten();
                let link = teams_core::team_link(
                    team_thread_id.as_deref().unwrap_or(team_id),
                    team_id,
                    crate::demo::DEMO_TENANT_ID,
                );
                self.use_link(link, link_use, cx);
            }
            return;
        };
        let owned_id = team_id.to_owned();
        self.link_from(
            async move { engine.team_link(&owned_id).await },
            link_use,
            cx,
        );
    }

    pub fn channel_team_id(&self, channel_id: &str) -> Option<String> {
        self.sidebar.teams.iter().find_map(|team| {
            team.channels
                .iter()
                .any(|channel| channel.id == channel_id)
                .then(|| team.team.id.clone())
        })
    }

    pub fn channel_name(&self, channel_id: &str) -> Option<String> {
        self.sidebar
            .teams
            .iter()
            .flat_map(|team| &team.channels)
            .find(|channel| channel.id == channel_id)
            .map(|channel| channel.name.clone())
    }

    fn use_link(&mut self, link: String, link_use: LinkUse, cx: &mut Context<Self>) {
        match link_use {
            LinkUse::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(link));
                self.raise_notice(tr(COPIED_TO_CLIPBOARD), None, cx);
            }
            LinkUse::OpenInBrowser => cx.open_url(&link),
        }
    }

    fn link_from(
        &mut self,
        call: impl Future<Output = teams_core::Result<String>> + Send + 'static,
        link_use: LinkUse,
        cx: &mut Context<Self>,
    ) {
        let receiver = crate::runtime::spawn(call);
        cx.spawn(async move |this, cx| {
            let outcome = match receiver.await {
                Ok(Ok(link)) => Ok(link),
                Ok(Err(error)) => Err(crate::notice::short_error(&error)),
                Err(_) => Err("cancelled".to_owned()),
            };
            this.update(cx, |state, cx| match outcome {
                Ok(link) => state.use_link(link, link_use, cx),
                Err(reason) => state.raise_notice(link_use.failure(&reason), None, cx),
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn run_channel_action(
        &mut self,
        failure: &'static str,
        call: impl Future<Output = teams_core::Result<()>> + Send + 'static,
        revert: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        success: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        let receiver = crate::runtime::spawn(call);
        cx.spawn(async move |this, cx| {
            let failed = !matches!(receiver.await, Ok(Ok(())));
            this.update(cx, |state, cx| {
                if failed {
                    revert(state, cx);
                    state.raise_notice(app_base::ui_locale::tr(failure), None, cx);
                } else if let Some(text) = success {
                    state.raise_notice(app_base::ui_locale::tr(text), None, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use crate::team_admin::ChannelAdmin;

    use super::{
        ChannelMenuItem, ChannelMenuState, MenuEntry, TeamMenuItem, TeamMenuState,
        channel_menu_entries, channel_visibility_failure, pin_failure, post_pin_dialog,
        post_pin_failure, post_pin_notice, team_menu_entries, team_visibility_failure,
    };

    fn labels<Item: Copy>(
        entries: &[MenuEntry<Item>],
        label: impl Fn(Item) -> &'static str,
    ) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(item) => app_base::ui_locale::tr(label(*item)),
                MenuEntry::Separator => "-".to_owned(),
            })
            .collect()
    }

    #[test]
    fn channel_menu_follows_teams_order() {
        app_base::ui_locale::apply_locale("en");
        let shown = channel_menu_entries(ChannelMenuState {
            pinned: false,
            shown: true,
            ..ChannelMenuState::default()
        });
        assert_eq!(
            labels(&shown, ChannelMenuItem::label),
            [
                "Channel notifications",
                "Pin",
                "Hide",
                "-",
                "Manage channel",
                "Copy link"
            ]
        );
        let hidden_pinned = channel_menu_entries(ChannelMenuState {
            pinned: true,
            shown: false,
            ..ChannelMenuState::default()
        });
        assert_eq!(
            labels(&hidden_pinned, ChannelMenuItem::label),
            [
                "Channel notifications",
                "Unpin",
                "Show",
                "-",
                "Manage channel",
                "Copy link"
            ]
        );
    }

    #[test]
    fn pinned_channels_cannot_be_hidden() {
        let pinned = ChannelMenuState {
            pinned: true,
            shown: true,
            ..ChannelMenuState::default()
        };
        assert!(pinned.disables(ChannelMenuItem::Hide));
        assert!(!pinned.disables(ChannelMenuItem::Unpin));
        let unpinned = ChannelMenuState {
            pinned: false,
            shown: true,
            ..ChannelMenuState::default()
        };
        assert!(!unpinned.disables(ChannelMenuItem::Hide));
    }

    #[test]
    fn edit_and_delete_appear_only_with_the_rights() {
        app_base::ui_locale::apply_locale("en");
        let admin = ChannelMenuState {
            pinned: false,
            shown: true,
            admin: ChannelAdmin {
                can_edit: true,
                can_delete: true,
            },
        };
        assert_eq!(
            labels(&channel_menu_entries(admin), ChannelMenuItem::label),
            [
                "Channel notifications",
                "Pin",
                "Hide",
                "-",
                "Edit channel",
                "Manage channel",
                "Copy link",
                "-",
                "Delete channel"
            ]
        );
    }

    #[test]
    fn team_menu_switches_between_show_and_hide() {
        app_base::ui_locale::apply_locale("en");
        let shown = TeamMenuState::default();
        assert_eq!(
            labels(&team_menu_entries(shown), TeamMenuItem::label),
            ["Hide", "-", "Manage team", "Copy link"]
        );
        let hidden = TeamMenuState {
            hidden: true,
            ..shown
        };
        assert_eq!(
            labels(&team_menu_entries(hidden), TeamMenuItem::label),
            ["Show", "-", "Manage team", "Copy link"]
        );
    }

    #[test]
    fn add_channel_leads_the_team_menu_when_the_team_allows_it() {
        app_base::ui_locale::apply_locale("en");
        let state = TeamMenuState {
            hidden: false,
            can_add_channel: true,
        };
        assert_eq!(
            labels(&team_menu_entries(state), TeamMenuItem::label),
            ["Add channel", "Hide", "-", "Manage team", "Copy link"]
        );
    }

    #[test]
    fn post_pin_dialogs_and_notices_use_teams_strings() {
        use app_base::ui_locale::tr;
        app_base::ui_locale::apply_locale("en");
        let unpin = post_pin_dialog(false);
        assert_eq!(tr(unpin.title), "Unpin this message?");
        assert_eq!(
            tr(unpin.body),
            "The message will be unpinned for everyone who views the channel."
        );
        assert_eq!(tr(unpin.confirm), "Unpin");
        let pin = post_pin_dialog(true);
        assert_eq!(tr(pin.title), "Pin in channel");
        assert_eq!(tr(pin.confirm), "Pin");
        assert_eq!(tr(post_pin_notice(true)), "Post pinned");
        assert_eq!(tr(post_pin_notice(false)), "Post unpinned");
        assert_eq!(tr(post_pin_failure(true)), "We couldn't pin the post.");
        assert_eq!(tr(post_pin_failure(false)), "We couldn't unpin the post.");
    }

    #[test]
    fn failures_use_teams_strings() {
        use app_base::ui_locale::tr;
        app_base::ui_locale::apply_locale("en");
        assert_eq!(tr(pin_failure(true)), "We couldn't pin the channel.");
        assert_eq!(tr(pin_failure(false)), "We couldn't unpin the channel.");
        assert_eq!(
            tr(channel_visibility_failure(false)),
            "We couldn't hide the channel."
        );
        assert_eq!(
            tr(team_visibility_failure(true)),
            "We couldn't show the team."
        );
    }
}
