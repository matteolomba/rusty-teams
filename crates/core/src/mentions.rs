use std::ops::Range;

use graph::{MentionTarget, OutgoingMention};
use serde::{Deserialize, Serialize};
use store::MessageRecord;

use crate::draft::{Draft, LineKind, MarkKind};
use crate::error::{Error, Result};
use crate::markdown::escape_html;
use crate::spans::Span;
use crate::stored::MentionTargetKind;

/// `text` is the display name exactly as it appears after the `@` in the message text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MentionInput {
    pub target: MentionTarget,
    pub text: String,
}

impl MentionInput {
    pub fn user(user_id: &str, text: &str) -> Self {
        MentionInput {
            target: MentionTarget::User {
                user_id: user_id.to_owned(),
            },
            text: text.to_owned(),
        }
    }

    pub fn channel(channel_id: &str, text: &str) -> Self {
        MentionInput {
            target: MentionTarget::Channel {
                channel_id: channel_id.to_owned(),
            },
            text: text.to_owned(),
        }
    }

    pub fn team(team_id: &str, text: &str) -> Self {
        MentionInput {
            target: MentionTarget::Team {
                team_id: team_id.to_owned(),
            },
            text: text.to_owned(),
        }
    }

    pub fn everyone(chat_id: &str, text: &str) -> Self {
        MentionInput {
            target: MentionTarget::Everyone {
                chat_id: chat_id.to_owned(),
            },
            text: text.to_owned(),
        }
    }

    pub fn tag(tag_id: &str, text: &str) -> Self {
        MentionInput {
            target: MentionTarget::Tag {
                tag_id: tag_id.to_owned(),
            },
            text: text.to_owned(),
        }
    }

    /// Mention inputs of a saved message, in the order of its `@` spans so
    /// `apply_mentions` gives them the same ids on edit. `classify_group`
    /// resolves legacy group targets from local data, without a network lookup;
    /// returning `None` drops a target that cannot be identified reliably.
    pub fn saved_inputs(
        record: &MessageRecord,
        in_chat: bool,
        classify_group: impl Fn(&str, &str) -> Option<MentionInput>,
    ) -> Vec<Self> {
        let infos = crate::stored::mentions(record);
        let mut inputs = Vec::new();
        collect_saved_mentions(
            &crate::stored::message_spans(record),
            &infos,
            &record.conversation_id,
            in_chat,
            &classify_group,
            &mut inputs,
        );
        inputs
    }
}

fn collect_saved_mentions(
    spans: &[Span],
    infos: &[crate::stored::MentionInfo],
    conversation_id: &str,
    in_chat: bool,
    classify_group: &impl Fn(&str, &str) -> Option<MentionInput>,
    inputs: &mut Vec<MentionInput>,
) {
    for span in spans {
        match span {
            Span::Bold(children)
            | Span::Italic(children)
            | Span::Strike(children)
            | Span::Superscript(children)
            | Span::Subscript(children)
            | Span::Underline(children)
            | Span::BlockQuote(children) => collect_saved_mentions(
                children,
                infos,
                conversation_id,
                in_chat,
                classify_group,
                inputs,
            ),
            Span::Sized(_, children)
            | Span::Colored { children, .. }
            | Span::Heading { children, .. }
            | Span::Link { children, .. }
            | Span::Indent { children, .. } => collect_saved_mentions(
                children,
                infos,
                conversation_id,
                in_chat,
                classify_group,
                inputs,
            ),
            Span::List { items, .. } => items.iter().for_each(|item| {
                collect_saved_mentions(
                    item,
                    infos,
                    conversation_id,
                    in_chat,
                    classify_group,
                    inputs,
                )
            }),
            Span::Table { rows, .. } => rows.iter().flatten().for_each(|cell| {
                collect_saved_mentions(
                    cell,
                    infos,
                    conversation_id,
                    in_chat,
                    classify_group,
                    inputs,
                )
            }),
            // The reply quote is not part of the edited draft.
            Span::Quote { .. } => {}
            Span::Mention { name, id } => {
                let Some(id) = id.as_deref().and_then(|id| id.parse::<i64>().ok()) else {
                    continue;
                };
                let Some(info) = infos.iter().find(|info| info.id == Some(id)) else {
                    continue;
                };
                let Some(target_id) = info.target_id.as_deref().filter(|id| !id.is_empty()) else {
                    continue;
                };
                let target = match info.target_kind {
                    Some(MentionTargetKind::User) => MentionTarget::User {
                        user_id: info
                            .user_id
                            .as_deref()
                            .filter(|id| !id.is_empty())
                            .unwrap_or(target_id)
                            .to_owned(),
                    },
                    Some(MentionTargetKind::Channel) => MentionTarget::Channel {
                        channel_id: target_id.to_owned(),
                    },
                    Some(MentionTargetKind::Team) => MentionTarget::Team {
                        team_id: target_id.to_owned(),
                    },
                    Some(MentionTargetKind::Everyone) => MentionTarget::Everyone {
                        chat_id: target_id.to_owned(),
                    },
                    Some(MentionTargetKind::Tag) => MentionTarget::Tag {
                        tag_id: target_id.to_owned(),
                    },
                    None => match info.user_id.as_deref().filter(|id| !id.is_empty()) {
                        Some(user_id) => MentionTarget::User {
                            user_id: user_id.to_owned(),
                        },
                        // An application has no outgoing mention target here.
                        None if !info.group => continue,
                        None if target_id == conversation_id => {
                            if in_chat {
                                MentionTarget::Everyone {
                                    chat_id: target_id.to_owned(),
                                }
                            } else {
                                MentionTarget::Channel {
                                    channel_id: target_id.to_owned(),
                                }
                            }
                        }
                        None => match classify_group(target_id, name) {
                            Some(input) => input.target,
                            None => continue,
                        },
                    },
                };
                inputs.push(MentionInput {
                    target,
                    text: name.clone(),
                });
            }
            _ => {}
        }
    }
}

fn same_char(expected: char, actual: char, ignore_case: bool) -> bool {
    expected == actual || (ignore_case && expected.to_lowercase().eq(actual.to_lowercase()))
}

fn word_char(character: Option<char>) -> bool {
    character.is_some_and(char::is_alphanumeric)
}

/// An `@` opens a mention only at the start, after whitespace or after opening punctuation: not in `ops@acme.com` or `medium.com/@ada`.
fn opens_mention(before: Option<char>) -> bool {
    before.is_none_or(|character| character.is_whitespace() || "([{\"'<,:;".contains(character))
}

/// Byte length of the text that spells `name`, compared character by character.
fn spelled_len(text: &str, name: &str, ignore_case: bool) -> Option<usize> {
    let mut characters = text.char_indices();
    let mut length = 0;
    for expected in name.chars() {
        let (offset, actual) = characters.next()?;
        if !same_char(expected, actual, ignore_case) {
            return None;
        }
        length = offset + actual.len_utf8();
    }
    (length > 0).then_some(length)
}

fn in_plain_run(draft: &Draft, span: &Range<usize>) -> bool {
    let marks_allow = draft.marks().iter().all(|mark| {
        let touches = mark.range.start < span.end && span.start < mark.range.end;
        !touches
            || (!matches!(mark.kind, MarkKind::Link(_) | MarkKind::Code)
                && mark.range.start <= span.start
                && mark.range.end >= span.end)
    });
    let in_code_block = [span.start, span.end.saturating_sub(1)]
        .into_iter()
        .any(|offset| {
            matches!(
                draft.lines().get(draft.line_at(offset)),
                Some(LineKind::Code(_))
            )
        });
    marks_allow && !in_code_block
}

/// Every `@name` a mention may sit on: whole word on both sides, in plain text, not in a link, code or half-formatted.
pub fn plain_mention_spans(draft: &Draft, name: &str, ignore_case: bool) -> Vec<Range<usize>> {
    let text = draft.text();
    text.match_indices('@')
        .filter_map(|(start, _)| {
            let end = start + 1 + spelled_len(&text[start + 1..], name, ignore_case)?;
            let spans_ok = opens_mention(text[..start].chars().next_back())
                && !word_char(text[end..].chars().next());
            (spans_ok && in_plain_run(draft, &(start..end))).then_some(start..end)
        })
        .collect()
}

struct TextChar {
    character: char,
    start: usize,
    end: usize,
    plain: bool,
}

const NAMED_ENTITIES: [(&str, char); 4] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
];

fn decode_entity(rest: &str) -> Option<(char, usize)> {
    if let Some((entity, character)) = NAMED_ENTITIES
        .iter()
        .find(|(entity, _)| rest.starts_with(entity))
    {
        return Some((*character, entity.len()));
    }
    let numeric = rest.strip_prefix("&#")?;
    let end = numeric.bytes().take(10).position(|byte| byte == b';')?;
    let digits = &numeric[..end];
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) if !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            u32::from_str_radix(hex, 16).ok()?
        }
        None if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            digits.parse().ok()?
        }
        _ => return None,
    };
    Some((char::from_u32(code)?, 2 + end + 1))
}

/// The characters of the text nodes with their HTML offsets; `plain` is false inside a, code and pre.
fn text_characters(html: &str) -> Vec<TextChar> {
    let mut characters = Vec::new();
    let mut depth = 0usize;
    let mut offset = 0;
    while offset < html.len() {
        let rest = &html[offset..];
        if rest.starts_with('<')
            && let Some(close) = rest.find('>')
        {
            let tag = rest[1..close].trim();
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if matches!(name.as_str(), "a" | "code" | "pre") && !tag.ends_with('/') {
                depth = if closing {
                    depth.saturating_sub(1)
                } else {
                    depth + 1
                };
            }
            offset += close + 1;
            if matches!(
                name.as_str(),
                "br" | "p"
                    | "div"
                    | "li"
                    | "ul"
                    | "ol"
                    | "blockquote"
                    | "pre"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "hr"
                    | "table"
                    | "tr"
                    | "td"
                    | "th"
                    | "figure"
            ) {
                characters.push(TextChar {
                    character: ' ',
                    start: offset,
                    end: offset,
                    plain: true,
                });
            }
            continue;
        }
        let (character, length) = decode_entity(rest).unwrap_or_else(|| {
            let character = rest.chars().next().unwrap_or_default();
            (character, character.len_utf8())
        });
        characters.push(TextChar {
            character,
            start: offset,
            end: offset + length,
            plain: depth == 0,
        });
        offset += length;
    }
    characters
}

fn spelled_at(characters: &[TextChar], at: usize, name: &[char]) -> bool {
    let Some(span) = characters.get(at..at + 1 + name.len()) else {
        return false;
    };
    span[0].character == '@'
        && span
            .iter()
            .zip(std::iter::once(&'@').chain(name))
            .all(|(found, expected)| found.character == *expected && found.plain)
        && span.windows(2).all(|pair| pair[0].end == pair[1].start)
        && opens_mention(at.checked_sub(1).map(|before| characters[before].character))
        && !word_char(
            characters
                .get(at + 1 + name.len())
                .map(|after| after.character),
        )
}

/// Mentions are matched in order, each at the first `@text` after the previous one that sits in a plain text run (see `plain_mention_spans`). Unmatched mentions stay plain text.
pub fn apply_mentions(html: &str, inputs: &[MentionInput]) -> (String, Vec<OutgoingMention>) {
    let characters = text_characters(html);
    let mut output = String::with_capacity(html.len());
    let mut outgoing = Vec::new();
    let mut cursor = 0;
    let mut next_character = 0;
    for input in inputs {
        let name: Vec<char> = input.text.chars().collect();
        let Some(at) = (next_character..characters.len())
            .find(|at| !name.is_empty() && spelled_at(&characters, *at, &name))
        else {
            continue;
        };
        let last = &characters[at + name.len()];
        let id = outgoing.len() as u32;
        output.push_str(&html[cursor..characters[at].start]);
        output.push_str(&format!(
            "<at id=\"{id}\">{}</at>",
            escape_html(&input.text)
        ));
        cursor = last.end;
        next_character = at + name.len() + 1;
        outgoing.push(OutgoingMention {
            id,
            text: input.text.clone(),
            target: input.target.clone(),
        });
    }
    output.push_str(&html[cursor..]);
    (output, outgoing)
}

pub(crate) fn ensure_allowed_in_chat(inputs: &[MentionInput]) -> Result<()> {
    if inputs.iter().any(|input| {
        !matches!(
            input.target,
            MentionTarget::User { .. } | MentionTarget::Everyone { .. }
        )
    }) {
        return Err(Error::Unsupported(
            "a channel, team or tag mention in a chat",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mention_right_after_a_heading_or_rule_is_still_a_mention() {
        for html in ["<h2>Plan</h2>@Ada please", "a<hr>@Ada please"] {
            let (_, mentions) = apply_mentions(html, &[MentionInput::user("u1", "Ada")]);
            assert_eq!(mentions.len(), 1, "{html}");
        }
    }

    #[test]
    fn mentions_become_at_tags_with_sequential_ids() {
        let (html, mentions) = apply_mentions(
            "hi @Ada Example and @Bo, see @General",
            &[
                MentionInput::user("u1", "Ada Example"),
                MentionInput::user("u2", "Bo"),
                MentionInput::channel("19:c", "General"),
            ],
        );
        assert_eq!(
            html,
            "hi <at id=\"0\">Ada Example</at> and <at id=\"1\">Bo</at>, see <at id=\"2\">General</at>"
        );
        assert_eq!(
            mentions
                .iter()
                .map(|mention| mention.id)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    fn spans(markdown: &str, name: &str, ignore_case: bool) -> Vec<String> {
        let draft = Draft::from_markdown(markdown);
        plain_mention_spans(&draft, name, ignore_case)
            .into_iter()
            .map(|span| draft.text()[span].to_owned())
            .collect()
    }

    fn placed(markdown: &str, name: &str) -> usize {
        let html = Draft::from_markdown(markdown).to_html();
        apply_mentions(&html, &[MentionInput::user("u1", name)])
            .1
            .len()
    }

    #[test]
    fn a_mention_never_matches_inside_a_longer_name_or_after_a_word() {
        assert_eq!(spans("@Adam and @Ada!", "Ada", false), ["@Ada"]);
        assert!(spans("@Adam", "Ada", false).is_empty());
        assert!(spans("ops@Ada.example", "Ada", false).is_empty());
        assert!(spans("mail ops@acme.com", "acme", false).is_empty());
        assert_eq!(placed("ops@Ada x", "Ada"), 0);
        assert_eq!(placed("@Adam then @Ada", "Ada"), 1);
    }

    #[test]
    fn mentions_stay_out_of_links_code_and_half_formatted_names() {
        for markdown in [
            "[docs](https://example.com/@Ada)",
            "see `@Ada` here",
            "```\n@Ada\n```",
            "read medium.com/@Ada today",
            "@**Ada**",
            "[@Ada](https://example.com)",
        ] {
            assert!(spans(markdown, "Ada", false).is_empty(), "{markdown}");
            assert_eq!(placed(markdown, "Ada"), 0, "{markdown}");
        }
        for markdown in ["**@Ada** hi", "link https://example.com/@Ada and @Ada"] {
            assert_eq!(spans(markdown, "Ada", false), ["@Ada"], "{markdown}");
            assert_eq!(placed(markdown, "Ada"), 1, "{markdown}");
        }
        let html = Draft::from_markdown("https://example.com/@Ada then @Ada").to_html();
        let (tagged, _) = apply_mentions(&html, &[MentionInput::user("u1", "Ada")]);
        assert!(tagged.contains("example.com/@Ada"));
        assert!(tagged.ends_with("then <at id=\"0\">Ada</at>"));
    }

    #[test]
    fn mentions_after_line_breaks_and_block_edges_work_in_both_rules() {
        for (markdown, name, expected) in [
            ("Hi team\n@Ada please", "Ada", 1),
            ("@Ada\nthanks", "Ada", 1),
            ("- @Ada\n- @Bo", "Bo", 1),
            ("- @Ada\n- @Bo", "Ada", 1),
            ("> x\n@Ada", "Ada", 1),
            ("@O'Brien hi", "O'Brien", 1),
            ("say @\"Ada\" now", "\"Ada\"", 1),
        ] {
            assert_eq!(spans(markdown, name, false).len(), expected, "{markdown}");
            assert_eq!(placed(markdown, name), expected, "{markdown}");
        }
    }

    #[test]
    fn numeric_entities_decode_like_named_ones() {
        assert_eq!(decode_entity("&#x27;rest"), Some(('\'', 6)),);
        assert_eq!(decode_entity("&#39;"), Some(('\'', 5)));
        assert_eq!(decode_entity("&#zz;"), None);
        assert_eq!(decode_entity("&#+39;"), None);
        assert_eq!(decode_entity("&#x+27;"), None);
        assert_eq!(decode_entity("&#-1;"), None);
        assert_eq!(decode_entity("&#;"), None);
        assert_eq!(decode_entity("&#1234567890123;"), None);
        assert_eq!(decode_entity("&amp;"), Some(('&', 5)));
    }

    #[test]
    fn case_insensitive_matching_keeps_offsets_after_wide_letters() {
        let draft = Draft::from_markdown("\u{130} @ADA");
        let found = plain_mention_spans(&draft, "ada", true);
        assert_eq!(found.len(), 1);
        assert_eq!(&draft.text()[found[0].clone()], "@ADA");
        assert!(plain_mention_spans(&draft, "ada", false).is_empty());
    }

    #[test]
    fn repeated_names_are_matched_left_to_right() {
        let (html, mentions) = apply_mentions(
            "@Ada then @Ada",
            &[
                MentionInput::user("u1", "Ada"),
                MentionInput::user("u2", "Ada"),
            ],
        );
        assert_eq!(html, "<at id=\"0\">Ada</at> then <at id=\"1\">Ada</at>");
        assert_eq!(mentions.len(), 2);
    }

    #[test]
    fn unmatched_mentions_are_dropped_and_ids_stay_dense() {
        let (html, mentions) = apply_mentions(
            "@Bo only",
            &[
                MentionInput::user("u1", "Ada"),
                MentionInput::user("u2", "Bo"),
            ],
        );
        assert_eq!(html, "<at id=\"0\">Bo</at> only");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].id, 0);
    }

    #[test]
    fn names_are_html_escaped_like_the_body() {
        let (html, mentions) = apply_mentions(
            &crate::markdown_to_html("ping @Ada & Co <x>"),
            &[MentionInput::user("u1", "Ada & Co")],
        );
        assert_eq!(html, "ping <at id=\"0\">Ada &amp; Co</at> &lt;x&gt;");
        assert_eq!(mentions[0].text, "Ada & Co");
    }

    #[test]
    fn mentions_inside_formatted_draft_html_become_at_tags() {
        let (html, mentions) = apply_mentions(
            &crate::Draft::from_markdown("**hi @Ada** and `@Bo`").to_html(),
            &[MentionInput::user("u1", "Ada")],
        );
        assert_eq!(html, "<b>hi <at id=\"0\">Ada</at></b> and <code>@Bo</code>");
        assert_eq!(mentions.len(), 1);
    }

    #[test]
    fn chats_only_take_user_mentions() {
        assert!(ensure_allowed_in_chat(&[MentionInput::user("u", "A")]).is_ok());
        assert!(ensure_allowed_in_chat(&[MentionInput::everyone("19:g", "Everyone")]).is_ok());
        assert!(ensure_allowed_in_chat(&[MentionInput::team("t", "Squad")]).is_err());
        assert!(ensure_allowed_in_chat(&[MentionInput::tag("tag-1", "Design")]).is_err());
    }

    fn saved_record(
        conversation_id: &str,
        body_html: &str,
        mentions: serde_json::Value,
    ) -> MessageRecord {
        MessageRecord {
            conversation_id: conversation_id.to_owned(),
            body_html: body_html.to_owned(),
            mentions_json: mentions.to_string(),
            ..MessageRecord::default()
        }
    }

    fn tagged_mentions(
        record: &MessageRecord,
        in_chat: bool,
        classify_group: impl Fn(&str, &str) -> Option<MentionInput>,
    ) -> (String, Vec<OutgoingMention>) {
        let inputs = MentionInput::saved_inputs(record, in_chat, classify_group);
        let html = Draft::from_spans(&crate::stored::message_spans(record)).to_html();
        apply_mentions(&html, &inputs)
    }

    #[test]
    fn saved_channel_mentions_keep_their_ids_beside_user_mentions() {
        let record = saved_record(
            "19:c@thread.tacv2",
            "<p>hi <at id=\"0\">Ada</at> and <at id=\"1\">General</at></p>",
            serde_json::json!([
                {"user_id": "u1", "name": "Ada", "id": 0, "target_id": "u1"},
                {"name": "General", "id": 1, "target_id": "19:c@thread.tacv2", "group": true},
            ]),
        );
        let (html, outgoing) =
            tagged_mentions(&record, false, |_, _| panic!("the channel names itself"));
        assert_eq!(
            html,
            "hi <at id=\"0\">Ada</at> and <at id=\"1\">General</at>"
        );
        assert_eq!(
            outgoing
                .iter()
                .map(|mention| (mention.id, mention.target.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    0,
                    MentionTarget::User {
                        user_id: "u1".into()
                    }
                ),
                (
                    1,
                    MentionTarget::Channel {
                        channel_id: "19:c@thread.tacv2".into()
                    }
                ),
            ]
        );
    }

    #[test]
    fn a_group_mention_of_the_chat_becomes_everyone_on_edit() {
        let record = saved_record(
            "19:g@thread.v2",
            "<p><at id=\"0\">Everyone</at> please</p>",
            serde_json::json!([
                {"name": "Everyone", "id": 0, "target_id": "19:g@thread.v2", "group": true},
            ]),
        );
        let (html, outgoing) = tagged_mentions(&record, true, |_, _| {
            panic!("a chat mention names the chat")
        });
        assert_eq!(html, "<at id=\"0\">Everyone</at> please");
        assert_eq!(
            outgoing[0].target,
            MentionTarget::Everyone {
                chat_id: "19:g@thread.v2".into()
            }
        );
    }

    #[test]
    fn saved_team_and_tag_mentions_use_the_resolved_target() {
        let record = saved_record(
            "19:c@thread.tacv2",
            "<p><at id=\"0\">Squad</at> and <at id=\"1\">Design</at></p>",
            serde_json::json!([
                {"name": "Squad", "id": 0, "target_id": "team-1", "group": true},
                {"name": "Design", "id": 1, "target_id": "tag-1", "group": true},
            ]),
        );
        let (_, outgoing) = tagged_mentions(&record, false, |target, name| {
            if target == "team-1" {
                Some(MentionInput::team(target, name))
            } else {
                Some(MentionInput::tag(target, name))
            }
        });
        assert_eq!(
            outgoing
                .iter()
                .map(|mention| mention.target.clone())
                .collect::<Vec<_>>(),
            [
                MentionTarget::Team {
                    team_id: "team-1".into()
                },
                MentionTarget::Tag {
                    tag_id: "tag-1".into()
                },
            ]
        );
    }

    #[test]
    fn split_saved_mentions_become_one_input_like_the_draft() {
        let record = saved_record(
            "19:c@thread.tacv2",
            "<p><at id=\"0\">Tip</at> <at id=\"1\">day</at></p>",
            serde_json::json!([
                {"name": "Tip", "id": 0, "target_id": "tag-1", "group": true},
                {"name": "day", "id": 1, "target_id": "tag-1", "group": true},
            ]),
        );
        let inputs = MentionInput::saved_inputs(&record, false, |target, name| {
            Some(MentionInput::tag(target, name))
        });
        assert_eq!(inputs, [MentionInput::tag("tag-1", "Tip day")]);
        let html = Draft::from_spans(&crate::stored::message_spans(&record)).to_html();
        let (tagged, outgoing) = apply_mentions(&html, &inputs);
        assert_eq!(tagged, "<at id=\"0\">Tip day</at>");
        assert_eq!(outgoing[0].text, "Tip day");
    }

    #[test]
    fn a_stored_application_mention_is_not_recreated_as_a_person() {
        let record = saved_record(
            "19:c@thread.tacv2",
            "<p>ping <at id=\"0\">Bot</at></p>",
            serde_json::json!([
                {"name": "Bot", "id": 0, "target_id": "bot-1"},
            ]),
        );
        let inputs = MentionInput::saved_inputs(&record, false, |_, _| {
            panic!("an application is not a group")
        });
        assert!(inputs.is_empty());
    }

    #[test]
    fn explicit_target_kinds_need_no_local_group_lookup() {
        let record = saved_record(
            "19:c@thread.tacv2",
            "<p><at id=\"0\">Squad</at> <at id=\"1\">Design</at> <at id=\"2\">Ada</at></p>",
            serde_json::json!([
                {"name": "Squad", "id": 0, "target_id": "team-1", "group": true, "target_kind": "team"},
                {"name": "Design", "id": 1, "target_id": "tag-1", "group": true, "target_kind": "tag"},
                {"name": "Ada", "id": 2, "target_id": "u1", "user_id": "u1", "target_kind": "user"},
            ]),
        );
        let (_, outgoing) = tagged_mentions(&record, false, |_, _| {
            panic!("explicit kinds need no cache")
        });
        assert_eq!(
            outgoing
                .iter()
                .map(|mention| mention.target.clone())
                .collect::<Vec<_>>(),
            [
                MentionTarget::Team {
                    team_id: "team-1".into()
                },
                MentionTarget::Tag {
                    tag_id: "tag-1".into()
                },
                MentionTarget::User {
                    user_id: "u1".into()
                },
            ]
        );
    }

    #[test]
    fn explicit_channel_and_everyone_kinds_ignore_the_current_view() {
        let channel = saved_record(
            "19:c@thread.tacv2",
            "<p><at id=\"0\">General</at></p>",
            serde_json::json!([
                {"name": "General", "id": 0, "target_id": "19:c@thread.tacv2", "group": true, "target_kind": "channel"},
            ]),
        );
        let (_, outgoing) = tagged_mentions(&channel, true, |_, _| {
            panic!("explicit kinds need no cache")
        });
        assert_eq!(
            outgoing[0].target,
            MentionTarget::Channel {
                channel_id: "19:c@thread.tacv2".into()
            }
        );

        let chat = saved_record(
            "19:g@thread.v2",
            "<p><at id=\"0\">Everyone</at></p>",
            serde_json::json!([
                {"name": "Everyone", "id": 0, "target_id": "19:g@thread.v2", "group": true, "target_kind": "everyone"},
            ]),
        );
        let (_, outgoing) =
            tagged_mentions(&chat, false, |_, _| panic!("explicit kinds need no cache"));
        assert_eq!(
            outgoing[0].target,
            MentionTarget::Everyone {
                chat_id: "19:g@thread.v2".into()
            }
        );
    }

    #[test]
    fn an_unresolved_legacy_group_mention_is_dropped() {
        let record = saved_record(
            "19:c@thread.tacv2",
            "<p>ping <at id=\"0\">Mystery</at></p>",
            serde_json::json!([
                {"name": "Mystery", "id": 0, "target_id": "unknown-1", "group": true},
            ]),
        );
        let inputs = MentionInput::saved_inputs(&record, false, |_, _| None);
        assert!(inputs.is_empty());
    }
}
