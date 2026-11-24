//! Read-only calendars of people in the organization, from their free/busy schedule.

use app_base::ui_locale::tr;
use chrono::{DateTime, Utc};
use store::{CalendarAccounts, PersonCalendar};

use chatsvc::cards::CardTransport;
use chatsvc::{Calendar, CalendarEvent, CalendarSource, PersonSchedule, ShowAs};

/// Graph's getSchedule limit per request.
const SCHEDULES_PER_REQUEST: usize = 20;

/// Events plus per-person failure messages.
pub async fn fetch_people_events<T: CardTransport>(
    calendar: &Calendar<T>,
    accounts: &CalendarAccounts,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<(Vec<CalendarEvent>, Vec<String>), String> {
    let people: Vec<&PersonCalendar> = accounts
        .people_calendars
        .iter()
        .filter(|person| person.selected)
        .collect();
    let mut events = Vec::new();
    let mut failures = Vec::new();
    for chunk in people.chunks(SCHEDULES_PER_REQUEST) {
        let mails: Vec<String> = chunk.iter().map(|person| person.mail.clone()).collect();
        let schedules = match calendar.schedules(&mails, start, end).await {
            Ok(schedules) => schedules,
            Err(error) => {
                let names = chunk
                    .iter()
                    .map(|person| person.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                failures.push(format!("{names}: {error}"));
                continue;
            }
        };
        for person in chunk {
            let Some(schedule) = schedules
                .iter()
                .find(|schedule| schedule.email.eq_ignore_ascii_case(&person.mail))
            else {
                continue;
            };
            match &schedule.error {
                Some(error) => failures.push(format!("{}: {error}", person.name)),
                None => events.extend(person_events(person, schedule)),
            }
        }
    }
    Ok((events, failures))
}

pub fn person_events(person: &PersonCalendar, schedule: &PersonSchedule) -> Vec<CalendarEvent> {
    schedule
        .busy
        .iter()
        .enumerate()
        .map(|(index, slot)| CalendarEvent {
            object_id: format!("person:{}:{}:{index}", person.mail, slot.start.timestamp()),
            subject: slot
                .subject
                .clone()
                .unwrap_or_else(|| status_text(slot.show_as())),
            start: slot.start,
            end: slot.end,
            show_as: slot.show_as(),
            location: slot.location.clone(),
            source: CalendarSource::Person,
            sources: vec![CalendarSource::Person],
            account_id: Some(person.mail.clone()),
            calendar_name: Some(person.name.clone()),
            ..Default::default()
        })
        .collect()
}

fn status_text(show_as: ShowAs) -> String {
    tr(match show_as {
        ShowAs::Tentative => "calendar.view.person_tentative",
        ShowAs::OutOfOffice => "calendar.view.person_away",
        ShowAs::WorkingElsewhere => "calendar.view.person_elsewhere",
        _ => "calendar.view.person_busy",
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chatsvc::Result as ServiceResult;
    use chrono::TimeZone;
    use serde_json::{Value, json};
    use session::{ApiResponse, Request, Scope};

    use super::*;

    fn slot(subject: Option<&str>, status: &str) -> chatsvc::BusyTime {
        let start = Utc.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap();
        chatsvc::BusyTime {
            start,
            end: start + chrono::Duration::hours(1),
            status: status.to_owned(),
            subject: subject.map(str::to_owned),
            location: None,
        }
    }

    #[test]
    fn slots_become_read_only_person_events() {
        let person = PersonCalendar {
            mail: "mara@example.com".into(),
            name: "Mara".into(),
            selected: true,
            color: None,
        };
        let schedule = PersonSchedule {
            email: "mara@example.com".into(),
            busy: vec![slot(Some("Planning"), "busy"), slot(None, "oof")],
            error: None,
        };
        let events = person_events(&person, &schedule);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].subject, "Planning");
        assert_eq!(events[0].source, CalendarSource::Person);
        assert_eq!(events[0].sources, vec![CalendarSource::Person]);
        assert_eq!(events[0].calendar_name.as_deref(), Some("Mara"));
        assert_eq!(events[0].account_id.as_deref(), Some("mara@example.com"));
        assert!(events[0].join_url.is_none());
        assert_eq!(events[1].show_as, ShowAs::OutOfOffice);
        assert!(!events[1].subject.is_empty());
        assert_ne!(events[0].object_id, events[1].object_id);
    }

    struct Recording {
        requests: Arc<Mutex<Vec<Value>>>,
        fail_on_request: Option<usize>,
    }

    impl CardTransport for Recording {
        async fn send(&self, request: Request, _scope: Scope) -> ServiceResult<ApiResponse> {
            let body = request.body.unwrap_or(json!({}));
            let entries = body["schedules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|mail| {
                    json!({
                        "scheduleId": mail,
                        "scheduleItems": [{
                            "status": "busy",
                            "subject": format!("Busy with {mail}"),
                            "start": {"dateTime": "2026-11-24T09:00:00.0000000", "timeZone": "UTC"},
                            "end": {"dateTime": "2026-11-24T10:00:00.0000000", "timeZone": "UTC"}
                        }],
                        "error": if mail == "p22@example.com" {
                            json!({"message": "not found"})
                        } else {
                            Value::Null
                        }
                    })
                })
                .collect::<Vec<_>>();
            let mut requests = self.requests.lock().unwrap();
            let request_index = requests.len();
            requests.push(body);
            drop(requests);
            if self.fail_on_request == Some(request_index) {
                return Err(chatsvc::Error::UnexpectedAnswer(
                    "transient failure".to_owned(),
                ));
            }
            Ok(ApiResponse {
                status: 200,
                body: json!({"value": entries}),
                retry_after: None,
            })
        }
    }

    #[tokio::test]
    async fn schedules_are_fetched_in_chunks_and_failures_are_reported() {
        let mut accounts = CalendarAccounts::default();
        for index in 0..25 {
            accounts
                .add_person_calendar(&format!("p{index}@example.com"), &format!("Person {index}"));
        }
        accounts.people_calendars[3].selected = false;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorder = Recording {
            requests: requests.clone(),
            fail_on_request: None,
        };
        let calendar = Calendar::with_transport(recorder, "emea");
        let start = Utc.with_ymd_and_hms(2026, 11, 24, 0, 0, 0).unwrap();
        let (events, failures) = fetch_people_events(&calendar, &accounts, start, start)
            .await
            .expect("schedules");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["schedules"].as_array().map(Vec::len), Some(20));
        assert_eq!(requests[1]["schedules"].as_array().map(Vec::len), Some(4));
        assert_eq!(events.len(), 23);
        assert_eq!(failures, vec!["Person 22: not found".to_owned()]);
    }

    #[tokio::test]
    async fn a_failed_chunk_keeps_events_from_the_others() {
        let mut accounts = CalendarAccounts::default();
        for index in 0..25 {
            accounts
                .add_person_calendar(&format!("p{index}@example.com"), &format!("Person {index}"));
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorder = Recording {
            requests: requests.clone(),
            fail_on_request: Some(1),
        };
        let calendar = Calendar::with_transport(recorder, "emea");
        let start = Utc.with_ymd_and_hms(2026, 11, 24, 0, 0, 0).unwrap();
        let (events, failures) = fetch_people_events(&calendar, &accounts, start, start)
            .await
            .expect("schedules");
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(events.len(), 20);
        assert_eq!(
            failures,
            vec![
                "Person 20, Person 21, Person 22, Person 23, Person 24: unexpected service answer: transient failure".to_owned()
            ]
        );
    }
}
