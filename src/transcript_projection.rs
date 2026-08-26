use std::collections::{BTreeMap, BTreeSet};

use bmux_tui::prelude::{Color, Line, Modifier, Span, Style};

use crate::model::{Message, ThreadId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadActivityLink {
    pub id: String,
    pub source_line: usize,
    pub target_line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptProjection {
    pub lines: Vec<Line>,
    pub links: Vec<ThreadActivityLink>,
}

#[derive(Debug)]
struct ThreadGroup<'a> {
    root: &'a Message,
    replies: Vec<&'a Message>,
}

pub fn project(
    messages: &[Message],
    sender_label: impl Fn(&crate::model::Sender) -> String,
    colors: TranscriptColors,
) -> TranscriptProjection {
    let groups = thread_groups(messages);
    let reply_ids = groups
        .values()
        .flat_map(|group| group.replies.iter().map(|reply| reply.id.clone()))
        .collect::<BTreeSet<_>>();
    let mut lines = Vec::new();
    let mut activity_links = Vec::new();
    let mut root_lines = BTreeMap::<ThreadId, usize>::new();
    let mut index = 0;

    while index < messages.len() {
        let message = &messages[index];
        if let Some(thread_id) = message.thread_id.as_ref()
            && let Some(group) = groups.get(thread_id)
            && message.id == group.root.id
        {
            root_lines.insert(thread_id.clone(), lines.len());
            append_message(&mut lines, group.root, false, &sender_label, colors);
            if !group.replies.is_empty() {
                lines.push(Line::from_spans([
                    Span::styled("  ┌─ ", Style::new().fg(colors.border)),
                    Span::styled(
                        format!(
                            "{} {}",
                            group.replies.len(),
                            if group.replies.len() == 1 {
                                "REPLY"
                            } else {
                                "REPLIES"
                            }
                        ),
                        Style::new().fg(colors.muted).add_modifier(Modifier::BOLD),
                    ),
                ]));
                for reply in &group.replies {
                    append_message(&mut lines, reply, true, &sender_label, colors);
                }
                lines.push(Line::from_spans([Span::styled(
                    "  └─",
                    Style::new().fg(colors.border),
                )]));
                lines.push(Line::from(""));
            }
            index += 1;
            continue;
        }

        if reply_ids.contains(&message.id) {
            let Some(thread_id) = message.thread_id.as_ref() else {
                index += 1;
                continue;
            };
            let start = index;
            let mut participants = BTreeSet::new();
            while index < messages.len()
                && messages[index].is_thread_reply
                && messages[index].thread_id.as_ref() == Some(thread_id)
            {
                if let Some(sender) = messages[index].sender.as_ref() {
                    participants.insert(sender_label(sender));
                }
                index += 1;
            }
            let count = index.saturating_sub(start);
            let target_line = root_lines.get(thread_id).copied().unwrap_or(0);
            let source_line = lines.len();
            let participant_text = participant_summary(&participants);
            lines.push(Line::from_spans([
                Span::styled("  ↪ ", Style::new().fg(colors.accent)),
                Span::styled(
                    format!(
                        "{participant_text} added {count} {} to a thread",
                        if count == 1 { "reply" } else { "replies" }
                    ),
                    Style::new().fg(colors.muted),
                ),
                Span::styled(
                    "  Jump to thread ↵",
                    Style::new()
                        .fg(colors.accent)
                        .add_modifier(Modifier::UNDERLINE),
                ),
            ]));
            activity_links.push(ThreadActivityLink {
                id: format!("thread-activity-{source_line}"),
                source_line,
                target_line,
            });
            lines.push(Line::from(""));
            continue;
        }

        append_message(&mut lines, message, false, &sender_label, colors);
        index += 1;
    }

    TranscriptProjection {
        lines,
        links: activity_links,
    }
}

fn thread_groups(messages: &[Message]) -> BTreeMap<ThreadId, ThreadGroup<'_>> {
    let mut roots = BTreeMap::<ThreadId, &Message>::new();
    let mut replies = BTreeMap::<ThreadId, Vec<&Message>>::new();
    for message in messages {
        let Some(thread_id) = message.thread_id.as_ref() else {
            continue;
        };
        if message.is_thread_reply {
            replies.entry(thread_id.clone()).or_default().push(message);
        } else {
            roots.insert(thread_id.clone(), message);
        }
    }
    roots
        .into_iter()
        .map(|(thread_id, root)| {
            let replies = replies.remove(&thread_id).unwrap_or_default();
            (thread_id, ThreadGroup { root, replies })
        })
        .collect()
}

fn append_message(
    lines: &mut Vec<Line>,
    message: &Message,
    reply: bool,
    sender_label: &impl Fn(&crate::model::Sender) -> String,
    colors: TranscriptColors,
) {
    let sender = message
        .sender
        .as_ref()
        .map_or_else(|| "Unknown sender".to_string(), sender_label);
    let indent = if reply { "    │ " } else { "" };
    let background = Style::new().bg(colors.message_background);
    lines.push(Line::from_spans([
        Span::styled(
            format!("{indent}{sender}"),
            background.fg(colors.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", message.create_time),
            background.fg(colors.muted),
        ),
    ]));
    for physical_line in normalized_message_lines(&message.text) {
        lines.push(Line::from_spans([Span::styled(
            format!("{indent}{physical_line}"),
            background.fg(colors.text),
        )]));
    }
    if message.unsupported_content {
        lines.push(Line::from_spans([Span::styled(
            format!("{indent}◇ Rich content is not available in the terminal"),
            background.fg(colors.warning),
        )]));
    }
    lines.push(Line::from(""));
}

fn normalized_message_lines(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .split('\n')
        .map(|line| {
            let mut output = String::new();
            let mut column = 0usize;
            for character in line.chars() {
                match character {
                    '\t' => {
                        let spaces = 4usize.saturating_sub(column % 4);
                        output.extend(std::iter::repeat_n(' ', spaces));
                        column = column.saturating_add(spaces);
                    }
                    character if character.is_control() => {}
                    character => {
                        output.push(character);
                        column = column.saturating_add(1);
                    }
                }
            }
            output
        })
        .collect()
}

fn participant_summary(participants: &BTreeSet<String>) -> String {
    match participants.len() {
        0 => "Someone".to_string(),
        1 => participants
            .first()
            .cloned()
            .unwrap_or_else(|| "Someone".to_string()),
        2 => participants
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(" and "),
        count => format!(
            "{} and {} others",
            participants.first().map_or("Someone", String::as_str),
            count - 1
        ),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TranscriptColors {
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub warning: Color,
    pub border: Color,
    pub message_background: Color,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessageId, Sender, SenderKind};

    fn message(id: &str, time: &str, thread: Option<&str>, reply: bool) -> Message {
        Message {
            id: MessageId(id.to_string()),
            thread_id: thread.map(|id| ThreadId(id.to_string())),
            sender: Some(Sender {
                resource_name: "users/example".to_string(),
                display_name: Some("Example User".to_string()),
                kind: SenderKind::Human,
            }),
            text: id.to_string(),
            create_time: time.to_string(),
            is_thread_reply: reply,
            unsupported_content: false,
        }
    }

    fn colors() -> TranscriptColors {
        TranscriptColors {
            text: Color::White,
            muted: Color::BrightBlack,
            accent: Color::Cyan,
            warning: Color::Yellow,
            border: Color::BrightBlack,
            message_background: Color::Black,
        }
    }

    #[test]
    fn message_rows_use_the_message_background_but_spacing_does_not() {
        let projection = project(
            &[message("body", "09:00", None, false)],
            |sender| sender.display_name.clone().unwrap(),
            colors(),
        );

        assert!(
            projection.lines[0]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(Color::Black))
        );
        assert!(
            projection.lines[1]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(Color::Black))
        );
        assert!(projection.lines[2].spans[0].style.bg.is_none());
    }

    #[test]
    fn multiline_code_is_projected_as_real_rows_with_deterministic_tabs() {
        let mut code = message("code", "09:00", None, false);
        code.text = "fn main() {\n\tprintln!(\"hi 👩🏽‍💻\");\n}\n".to_string();
        let projection = project(
            &[code],
            |sender| sender.display_name.clone().unwrap(),
            colors(),
        );
        let rows = projection
            .lines
            .iter()
            .map(Line::plain_text)
            .collect::<Vec<_>>();
        assert!(rows.iter().any(|row| row == "fn main() {"));
        assert!(rows.iter().any(|row| row == "    println!(\"hi 👩🏽‍💻\");"));
        assert!(rows.iter().any(|row| row == "}"));
        assert!(
            rows.iter()
                .all(|row| !row.contains('\n') && !row.contains('\t'))
        );
    }

    #[test]
    fn attaches_replies_to_root_and_groups_consecutive_activity() {
        let messages = vec![
            message("root", "09:00", Some("thread"), false),
            message("ordinary", "09:05", None, false),
            message("reply-one", "09:10", Some("thread"), true),
            message("reply-two", "09:11", Some("thread"), true),
            message("ordinary-two", "09:12", None, false),
            message("reply-three", "09:13", Some("thread"), true),
        ];
        let projection = project(
            &messages,
            |sender| sender.display_name.clone().unwrap(),
            colors(),
        );
        let text = projection
            .lines
            .iter()
            .map(Line::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.find("reply-one").unwrap() < text.find("ordinary").unwrap());
        assert!(text.contains("added 2 replies to a thread"));
        assert!(text.contains("added 1 reply to a thread"));
        assert_eq!(projection.links.len(), 2);
        assert!(projection.links.iter().all(|link| link.target_line == 0));
        assert!(!text.contains("thread-activity-"));
    }

    #[test]
    fn orphan_reply_renders_as_ordinary_message() {
        let messages = vec![message("orphan", "09:10", Some("missing"), true)];
        let projection = project(
            &messages,
            |sender| sender.display_name.clone().unwrap(),
            colors(),
        );
        assert!(projection.links.is_empty());
        assert!(
            projection
                .lines
                .iter()
                .any(|line| line.plain_text().contains("orphan"))
        );
    }
}
