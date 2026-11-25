use std::sync::Arc;

use chatsvc::{CreatedEvent, GoogleEventDraft, NewEvent};
use store::{CalendarAccounts, CalendarProvider, CalendarRef, Store};

use super::launcher::CallLauncher;
use super::schedule::MeetingDraft;

/// Whether create must mint a Teams join URL before per-target inserts.
pub fn needs_join_first(accounts: &CalendarAccounts) -> bool {
    accounts
        .resolved_write_targets()
        .iter()
        .any(|target| target.provider != CalendarProvider::Outlook)
}

/// A created meeting plus the user-facing notice when only some calendars accepted it.
#[derive(Debug)]
pub struct CreateOutcome {
    pub event: CreatedEvent,
    pub notice: Option<String>,
}

/// Write targets used for New meeting (home + extras, de-duplicated).
pub fn create_targets(accounts: &CalendarAccounts) -> Vec<CalendarRef> {
    accounts.resolved_write_targets()
}

/// Format a partial-failure notice when the join link exists but a target failed.
pub fn partial_create_notice(join_url: &str, failed: &[String]) -> String {
    format!(
        "Meeting link ready, but calendar write failed for {}: {join_url}",
        failed.join(", ")
    )
}

pub async fn create_across_targets(
    launcher: Arc<CallLauncher>,
    draft: MeetingDraft,
    mut accounts: CalendarAccounts,
    store: Arc<Store>,
) -> Result<CreateOutcome, String> {
    if !needs_join_first(&accounts) {
        return launcher
            .create_meeting(draft)
            .await
            .map(|event| CreateOutcome {
                event,
                notice: None,
            });
    }
    let event = launcher.resolve_new_event(draft).await?;
    let join_url = launcher.create_meet_now(&event.subject).await?;
    let total_targets = create_targets(&accounts).len();
    let targets = create_targets(&accounts);
    let outlook_invites = targets
        .iter()
        .any(|target| target.provider == CalendarProvider::Outlook);
    let mut failed = Vec::new();
    let mut first_id = String::new();
    for target in targets {
        match write_target(
            launcher.as_ref(),
            &mut accounts,
            &target,
            &event,
            &join_url,
            outlook_invites,
        )
        .await
        {
            Ok(id) => {
                if first_id.is_empty() {
                    first_id = id;
                }
            }
            Err(error) => failed.push(format!("{} ({error})", target.key())),
        }
    }
    let _ = store::save_calendar_accounts(&store, &accounts);
    finish_creation(join_url, first_id, failed, total_targets)
}

fn finish_creation(
    join_url: String,
    first_id: String,
    failed: Vec<String>,
    total_targets: usize,
) -> Result<CreateOutcome, String> {
    if !failed.is_empty() && failed.len() == total_targets {
        let notice = format!(
            "Nessun calendario ha salvato la riunione ({join_url}): {}",
            failed.join(", ")
        );
        crate::diagnostics::record(notice.clone());
        return Err(notice);
    }
    let notice = (!failed.is_empty()).then(|| partial_create_notice(&join_url, &failed));
    if let Some(notice) = &notice {
        crate::diagnostics::record(notice.clone());
    }
    Ok(CreateOutcome {
        event: CreatedEvent {
            id: if first_id.is_empty() {
                join_url.clone()
            } else {
                first_id
            },
            join_url: Some(join_url),
        },
        notice,
    })
}

async fn write_target(
    launcher: &CallLauncher,
    accounts: &mut CalendarAccounts,
    target: &CalendarRef,
    event: &NewEvent,
    join_url: &str,
    outlook_invites: bool,
) -> Result<String, String> {
    match target.provider {
        CalendarProvider::Outlook => launcher
            .create_meeting_with_join(event.clone(), join_url)
            .await
            .map(|created| created.id),
        CalendarProvider::Google => {
            let calendar_id = if target.calendar_id.is_empty() {
                "primary"
            } else {
                target.calendar_id.as_str()
            };
            let mut draft = GoogleEventDraft::from(event);
            draft.join_url = Some(join_url.to_owned());
            crate::google_calendar_sync::insert_google_event(
                accounts,
                &target.account_id,
                calendar_id,
                &draft,
            )
            .await?;
            Ok(format!("google:{}:{}", target.account_id, calendar_id))
        }
        CalendarProvider::CalDav => {
            let attendees: Vec<String> = event
                .attendees
                .iter()
                .map(|attendee| attendee.address.clone())
                .collect();
            let draft = crate::caldav_sync::draft_from_meeting(
                &event.subject,
                event.start,
                event.end,
                Some(join_url),
                &attendees,
                !outlook_invites,
                event.repeat,
            );
            crate::caldav_sync::insert_caldav_event(
                accounts,
                &target.account_id,
                &target.calendar_id,
                &draft,
            )
            .await?;
            Ok(format!(
                "caldav:{}:{}",
                target.account_id, target.calendar_id
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use store::{CalendarRef, GoogleAccount, GoogleCalendarInfo, GoogleTokenSet};

    use super::*;

    fn google_home() -> CalendarAccounts {
        let mut accounts = CalendarAccounts::default();
        accounts.home = CalendarRef::google("g1", "primary");
        accounts.google_accounts.push(GoogleAccount {
            id: "g1".into(),
            email: "joe@example.com".into(),
            calendars: vec![GoogleCalendarInfo {
                id: "primary".into(),
                summary: "Joe".into(),
                selected: true,
                primary: true,
            }],
            tokens: GoogleTokenSet::default(),
        });
        accounts
    }

    #[test]
    fn outlook_only_home_keeps_one_step() {
        let accounts = CalendarAccounts::default();
        assert!(!needs_join_first(&accounts));
        assert_eq!(create_targets(&accounts).len(), 1);
        assert_eq!(
            create_targets(&accounts)[0].provider,
            CalendarProvider::Outlook
        );
    }

    #[test]
    fn google_home_needs_join_first() {
        assert!(needs_join_first(&google_home()));
    }

    #[test]
    fn caldav_only_home_needs_join_first() {
        let mut accounts = CalendarAccounts::default();
        accounts.home = CalendarRef::caldav("c1", "/cal/");
        assert!(needs_join_first(&accounts));
    }

    #[test]
    fn outlook_home_with_google_extra_needs_join_first() {
        let mut accounts = CalendarAccounts::default();
        accounts
            .write_targets
            .push(CalendarRef::google("g1", "primary"));
        assert!(needs_join_first(&accounts));
        assert_eq!(create_targets(&accounts).len(), 2);
    }

    #[test]
    fn partial_notice_keeps_join_url() {
        let notice = partial_create_notice(
            "https://teams.microsoft.com/l/meetup-join/x",
            &["Google:primary".into()],
        );
        assert!(notice.contains("teams.microsoft.com"));
        assert!(notice.contains("Google:primary"));
    }

    #[test]
    fn one_calendar_wins_and_the_failures_become_a_notice() {
        let outcome = finish_creation(
            "https://teams.microsoft.com/l/meetup-join/x".into(),
            "outlook-id".into(),
            vec!["Google:primary (offline)".into(), "CalDAV:c1 (403)".into()],
            3,
        )
        .expect("Outlook created the event");
        assert_eq!(outcome.event.id, "outlook-id");
        assert_eq!(
            outcome.event.join_url.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/x")
        );
        let notice = outcome.notice.expect("partial failures are reported");
        assert!(notice.contains("Google:primary"));
        assert!(notice.contains("CalDAV:c1"));
    }

    #[test]
    fn no_calendar_wins_and_no_fake_event_is_returned() {
        let error = finish_creation(
            "https://teams.microsoft.com/l/meetup-join/x".into(),
            String::new(),
            vec!["Google:primary (offline)".into(), "CalDAV:c1 (403)".into()],
            2,
        )
        .expect_err("every calendar failed");
        assert!(error.contains("Google:primary"));
        assert!(error.contains("https://teams.microsoft.com/l/meetup-join/x"));
    }
}
