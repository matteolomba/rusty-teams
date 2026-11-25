use app_base::orgs::{OPEN_CONVERSATION_FLAG, Org, Orgs};
use gpui_kit::Context;

use crate::app_state::AppState;

impl AppState {
    pub(crate) fn take_org_list(&mut self, list: Vec<Org>) {
        if self.orgs.list == list {
            return;
        }
        self.orgs.replace_list(list);
        self.save_orgs(&self.orgs);
    }

    /// A guest org has no calendar for the account, like in Teams.
    pub fn calendar_available(&self) -> bool {
        self.orgs.active_guest().is_none()
    }

    /// Orgs to offer in the account menu; a single org needs no switcher, like Teams.
    pub fn switchable_orgs(&self) -> &[Org] {
        if self.orgs.list.len() > 1 {
            &self.orgs.list
        } else {
            &[]
        }
    }

    /// Chats with new messages in `org` since this window opened; zero for the open org.
    pub fn org_unread_count(&self, org: &Org) -> usize {
        self.org_unread.get(&org.id).map_or(0, |chats| chats.len())
    }

    /// Restarts the app on `org`: its own cache opens at once and the backend signs in to its tenant.
    /// `open_conversation` opens that chat once the org has loaded.
    pub fn switch_org(
        &mut self,
        org: &Org,
        open_conversation: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if self.orgs.is_active(org) {
            return;
        }
        let mut next = self.orgs.clone();
        next.activate(org);
        self.restart_on(next, open_conversation, cx);
    }

    /// A guest MFA challenge cannot complete the home tenant's sign-in; leaving guest needs no new token.
    pub fn can_return_home(&self) -> bool {
        self.orgs.active_guest().is_some()
    }

    /// Restarts on the home tenant after a guest org asked for interactive MFA.
    /// Returns `false` when no guest org is open, so callers never restart in a loop.
    pub fn return_to_home_org(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(next) = return_home_orgs(&self.orgs) else {
            return false;
        };
        self.restart_on(next, None, cx);
        true
    }

    fn restart_on(&mut self, next: Orgs, open_conversation: Option<&str>, cx: &mut Context<Self>) {
        if !self.save_orgs(&next) {
            return;
        }
        self.orgs = next;
        match std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .args(restart_arguments(
                    std::env::args_os().skip(1),
                    open_conversation,
                ))
                .env(
                    app_base::updater::RELAUNCH_ENV,
                    std::process::id().to_string(),
                )
                .spawn()
        }) {
            Ok(_) => cx.quit(),
            Err(error) => {
                crate::diagnostics::record(format!("orgs: restart for the switch failed: {error}"))
            }
        }
    }

    fn save_orgs(&self, orgs: &Orgs) -> bool {
        if self.mode.demo {
            return false;
        }
        let saved = Orgs::path().map(|path| orgs.save(&path));
        if let Some(Err(error)) = &saved {
            crate::diagnostics::record(format!(
                "orgs: {} not saved: {error}",
                app_base::orgs::ORGS_FILE
            ));
        }
        matches!(saved, Some(Ok(())))
    }
}

/// The home tenant needs no row in the org list: clearing `active` leaves the guest cache.
fn return_home_orgs(orgs: &Orgs) -> Option<Orgs> {
    orgs.active_guest()?;
    let mut next = orgs.clone();
    next.activate_home();
    Some(next)
}

/// The process's own flags without an earlier open target, plus the chat to open now.
fn restart_arguments(
    current: impl Iterator<Item = std::ffi::OsString>,
    open_conversation: Option<&str>,
) -> Vec<std::ffi::OsString> {
    let mut arguments = Vec::new();
    let mut current = current;
    while let Some(argument) = current.next() {
        if argument == OPEN_CONVERSATION_FLAG {
            current.next();
            continue;
        }
        arguments.push(argument);
    }
    if let Some(conversation_id) = open_conversation {
        arguments.push(OPEN_CONVERSATION_FLAG.into());
        arguments.push(conversation_id.into());
    }
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_keeps_the_flags_and_replaces_the_open_target() {
        let current = ["--endpoint", "x", OPEN_CONVERSATION_FLAG, "19:old"].map(Into::into);
        assert_eq!(
            restart_arguments(current.into_iter(), Some("19:new")),
            ["--endpoint", "x", OPEN_CONVERSATION_FLAG, "19:new"].map(std::ffi::OsString::from)
        );
    }

    #[test]
    fn leaving_a_guest_org_reopens_the_home_cache() {
        let mut orgs = Orgs {
            active: None,
            list: vec![
                Org {
                    id: "aaaa-01".into(),
                    name: "Home".into(),
                    guest: false,
                },
                Org {
                    id: "bbbb-02".into(),
                    name: "Guest".into(),
                    guest: true,
                },
            ],
        };
        orgs.activate(&orgs.list[1].clone());
        let next = return_home_orgs(&orgs).expect("an open guest org can return home");
        assert_eq!(next.active, None);
        assert_eq!(next.database_file(), "cache.sqlite3");
        assert!(next.is_active(&next.list[0]));
        assert!(return_home_orgs(&next).is_none());
    }
}
