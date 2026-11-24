use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesRef, BytesText, Event};

/// Upper bound for an entire CalDAV XML response body accepted by the parser.
const MAX_XML_BYTES: usize = 32 * 1024 * 1024;
/// CalDAV responses are shallow; bound the parser's own element stack.
const MAX_XML_DEPTH: usize = 64;
/// Upper bound for a single collected text field, for example `calendar-data`.
const MAX_FIELD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct PropfindResponse {
    pub href: String,
    pub displayname: Option<String>,
    pub calendar_color: Option<String>,
    pub resource_type_calendar: bool,
    pub supports_vevent: bool,
    pub writable: bool,
    pub current_user_principal: Option<String>,
    pub calendar_home_set: Option<String>,
    pub calendar_user_addresses: Vec<String>,
    pub schedule_outbox: Option<String>,
    pub getctag: Option<String>,
}

#[derive(Clone, Copy)]
enum MultistatusField {
    Href,
    Principal,
    CalendarHomeSet,
    CalendarUserAddress,
    ScheduleOutbox,
    DisplayName,
    CalendarColor,
    CTag,
}

#[derive(Clone, Copy)]
enum QueryField {
    Href,
    ETag,
    CalendarData,
}

/// quick-xml 0.41 emits `GeneralRef` events instead of embedding `&...;` in
/// text events, so references must be resolved separately to keep the parsed
/// CalDAV values unchanged.
fn reference_fragment(reference: &BytesRef<'_>) -> Option<String> {
    let name = reference.decode().ok()?;
    let escaped = format!("&{name};");
    unescape(&escaped).ok().map(|value| value.into_owned())
}

fn push_fragment(field: &mut String, fragment: &str) -> bool {
    if field.len().saturating_add(fragment.len()) > MAX_FIELD_BYTES {
        return false;
    }
    field.push_str(fragment);
    true
}

fn text_fragment(text: &BytesText<'_>) -> String {
    // 0.41 emits references as separate events, so text events are already unescaped.
    text.decode().unwrap_or_default().into_owned()
}

/// Applies the old `trim_text(true)` behavior once the whole field is collected.
/// Trimming each fragment would corrupt values such as `Bob &amp; Sue`, whose
/// text is split into multiple events by quick-xml 0.41.
fn trimmed(value: String) -> String {
    let trimmed = value.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'));
    if trimmed.len() == value.len() {
        value
    } else {
        trimmed.to_owned()
    }
}

fn multistatus_field(path: &[String]) -> Option<MultistatusField> {
    match path.last().map(|name| name.as_str()) {
        Some("href") => Some(if path.iter().any(|p| p == "current-user-principal") {
            MultistatusField::Principal
        } else if path.iter().any(|p| p == "calendar-home-set") {
            MultistatusField::CalendarHomeSet
        } else if path.iter().any(|p| p == "calendar-user-address-set") {
            MultistatusField::CalendarUserAddress
        } else if path.iter().any(|p| p == "schedule-outbox-URL") {
            MultistatusField::ScheduleOutbox
        } else {
            MultistatusField::Href
        }),
        Some("displayname") => Some(MultistatusField::DisplayName),
        Some("calendar-color" | "color") => Some(MultistatusField::CalendarColor),
        Some("getctag" | "getetag") => Some(MultistatusField::CTag),
        _ => None,
    }
}

fn apply_multistatus_field(
    response: &mut PropfindResponse,
    field: MultistatusField,
    value: String,
) {
    let value = trimmed(value);
    match field {
        MultistatusField::Href => {
            if response.href.is_empty() {
                response.href = value;
            }
        }
        MultistatusField::Principal => response.current_user_principal = Some(value),
        MultistatusField::CalendarHomeSet => response.calendar_home_set = Some(value),
        MultistatusField::CalendarUserAddress => response.calendar_user_addresses.push(value),
        MultistatusField::ScheduleOutbox => response.schedule_outbox = Some(value),
        MultistatusField::DisplayName => response.displayname = Some(value),
        MultistatusField::CalendarColor => response.calendar_color = Some(value),
        MultistatusField::CTag => response.getctag = Some(value.trim_matches('"').to_owned()),
    }
}

fn query_field(path: &[String]) -> Option<QueryField> {
    match path.last().map(|name| name.as_str()) {
        Some("href") => Some(QueryField::Href),
        Some("getetag") => Some(QueryField::ETag),
        Some("calendar-data") => Some(QueryField::CalendarData),
        _ => None,
    }
}

fn apply_query_field(
    href: &mut String,
    etag: &mut Option<String>,
    data: &mut String,
    field: QueryField,
    value: String,
) {
    let value = trimmed(value);
    match field {
        QueryField::Href => {
            if href.is_empty() {
                *href = value;
            }
        }
        QueryField::ETag => *etag = Some(value.trim_matches('"').to_owned()),
        QueryField::CalendarData => data.push_str(&value),
    }
}

pub fn parse_multistatus(xml: &str) -> Vec<PropfindResponse> {
    // Reject oversized bodies before allocating an event buffer.
    if xml.len() > MAX_XML_BYTES {
        return Vec::new();
    }

    let mut reader = Reader::from_str(xml);
    let mut responses = Vec::new();
    let mut current = PropfindResponse::default();
    let mut in_response = false;
    let mut buf = Vec::new();
    let mut path: Vec<String> = Vec::new();
    // Collected text of the innermost scalar field: (field, depth, value).
    let mut pending: Option<(MultistatusField, usize, String)> = None;

    'events: loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().as_ref());
                if path.len() >= MAX_XML_DEPTH {
                    break 'events;
                }
                path.push(local.clone());
                if local == "response" {
                    in_response = true;
                    current = PropfindResponse::default();
                    pending = None;
                }
                if local == "calendar" && path.iter().any(|p| p == "resourcetype") {
                    current.resource_type_calendar = true;
                }
                if local == "comp" {
                    for attr in e.attributes().flatten() {
                        if local_name(attr.key.as_ref()) == "name"
                            && attr
                                .normalized_value(XmlVersion::Implicit1_0)
                                .map(|v| v.eq_ignore_ascii_case("VEVENT"))
                                .unwrap_or(false)
                        {
                            current.supports_vevent = true;
                        }
                    }
                }
                if in_response && let Some(field) = multistatus_field(&path) {
                    pending = Some((field, path.len(), String::new()));
                }
            }
            Ok(Event::Empty(e)) => {
                let local = local_name(e.name().as_ref());
                if local == "calendar" && path.iter().any(|p| p == "resourcetype") {
                    current.resource_type_calendar = true;
                }
                if local == "comp" {
                    for attr in e.attributes().flatten() {
                        if local_name(attr.key.as_ref()) == "name"
                            && attr
                                .normalized_value(XmlVersion::Implicit1_0)
                                .map(|v| v.eq_ignore_ascii_case("VEVENT"))
                                .unwrap_or(false)
                        {
                            current.supports_vevent = true;
                        }
                    }
                }
                if (local == "write" || local == "write-content" || local == "all")
                    && path.iter().any(|p| p == "current-user-privilege-set")
                {
                    current.writable = true;
                }
            }
            Ok(Event::Text(t)) => {
                if let Some((_, _, value)) = pending.as_mut()
                    && !push_fragment(value, &text_fragment(&t))
                {
                    break 'events;
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some((_, _, value)) = pending.as_mut()
                    && let Some(fragment) = reference_fragment(&r)
                    && !push_fragment(value, &fragment)
                {
                    break 'events;
                }
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().as_ref());
                let closes_pending = pending
                    .as_ref()
                    .is_some_and(|(_, depth, _)| *depth == path.len());
                if closes_pending && let Some((field, _, value)) = pending.take() {
                    apply_multistatus_field(&mut current, field, value);
                }
                if local == "response" && in_response {
                    responses.push(std::mem::take(&mut current));
                    in_response = false;
                    pending = None;
                }
                path.pop();
            }
            Ok(Event::Eof) => break,
            // Malformed XML: keep the responses already completed.
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    responses
}

pub fn parse_calendar_query(xml: &str) -> Vec<(String, Option<String>, String)> {
    // href, etag, calendar-data
    // Reject oversized bodies before allocating an event buffer.
    if xml.len() > MAX_XML_BYTES {
        return Vec::new();
    }

    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut href = String::new();
    let mut etag = None;
    let mut data = String::new();
    let mut in_response = false;
    let mut path: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    let mut pending: Option<(QueryField, usize, String)> = None;

    'events: loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().as_ref());
                if path.len() >= MAX_XML_DEPTH {
                    break 'events;
                }
                path.push(local.clone());
                if local == "response" {
                    in_response = true;
                    href.clear();
                    etag = None;
                    data.clear();
                    pending = None;
                }
                if in_response && let Some(field) = query_field(&path) {
                    pending = Some((field, path.len(), String::new()));
                }
            }
            Ok(Event::Text(t)) => {
                if let Some((_, _, value)) = pending.as_mut()
                    && !push_fragment(value, &text_fragment(&t))
                {
                    break 'events;
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some((_, _, value)) = pending.as_mut()
                    && let Some(fragment) = reference_fragment(&r)
                    && !push_fragment(value, &fragment)
                {
                    break 'events;
                }
            }
            Ok(Event::CData(t)) => {
                if let Some((_, _, value)) = pending.as_mut() {
                    let fragment = String::from_utf8_lossy(&t);
                    if !push_fragment(value, &fragment) {
                        break 'events;
                    }
                }
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().as_ref());
                let closes_pending = pending
                    .as_ref()
                    .is_some_and(|(_, depth, _)| *depth == path.len());
                if closes_pending && let Some((field, _, value)) = pending.take() {
                    apply_query_field(&mut href, &mut etag, &mut data, field, value);
                }
                if local == "response" && in_response {
                    if !data.is_empty() {
                        out.push((href.clone(), etag.clone(), data.clone()));
                    }
                    in_response = false;
                }
                path.pop();
            }
            Ok(Event::Eof) => break,
            // Malformed XML: keep the responses already completed.
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn local_name(name: &[u8]) -> String {
    let full = String::from_utf8_lossy(name);
    let after_ns = full.rsplit('}').next().unwrap_or(&full);
    after_ns
        .rsplit(':')
        .next()
        .unwrap_or(after_ns)
        .to_ascii_lowercase()
}

pub fn propfind_body(props: &[&str]) -> String {
    let mut body = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:x="http://apple.com/ns/ical/">
  <d:prop>
"#,
    );
    for prop in props {
        body.push_str("    ");
        body.push_str(prop);
        body.push('\n');
    }
    body.push_str("  </d:prop>\n</d:propfind>");
    body
}

pub fn calendar_query_body(start: &str, end: &str, expand: bool) -> String {
    let expand_xml = if expand {
        format!(
            r#"      <c:expand start="{start}" end="{end}"/>
"#
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data>
{expand_xml}    </c:calendar-data>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{start}" end="{end}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_calendar_query, parse_multistatus};

    #[test]
    fn resolves_references_in_text_fields() {
        let xml = r#"<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/joe/a&amp;b/</d:href>
    <d:propstat><d:prop>
      <d:displayname>A &amp; B &#x2F; C</d:displayname>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let responses = parse_multistatus(xml);
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].href, "/calendars/joe/a&b/");
        assert_eq!(responses[0].displayname.as_deref(), Some("A & B / C"));
    }

    #[test]
    fn stops_on_truncated_input() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/a</d:href>"#;
        assert!(parse_multistatus(xml).is_empty());
        assert!(parse_calendar_query(xml).is_empty());
    }

    #[test]
    fn bounds_element_nesting() {
        let mut xml = String::from(r#"<d:multistatus xmlns:d="DAV:"><d:response>"#);
        for _ in 0..200 {
            xml.push_str("<d:prop>");
        }
        xml.push_str("<d:href>/deep/</d:href>");
        for _ in 0..200 {
            xml.push_str("</d:prop>");
        }
        xml.push_str("</d:response></d:multistatus>");

        assert!(parse_multistatus(&xml).is_empty());
    }

    #[test]
    fn collects_calendar_data_from_text_and_cdata() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
<d:response>
  <d:href>/a.ics</d:href>
  <d:propstat><d:prop>
    <d:getetag>"e&amp;1"</d:getetag>
    <c:calendar-data><![CDATA[BEGIN:VCALENDAR
UID:1
]]>&amp;END:VCALENDAR</c:calendar-data>
  </d:prop></d:propstat>
</d:response>
</d:multistatus>"#;
        let items = parse_calendar_query(xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "/a.ics");
        assert_eq!(items[0].1.as_deref(), Some("e&1"));
        assert!(items[0].2.contains("UID:1"));
        assert!(items[0].2.contains("&END:VCALENDAR"));
    }
}
