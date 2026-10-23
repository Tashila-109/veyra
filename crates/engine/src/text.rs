use regex::Regex;
use std::sync::LazyLock;

static INVISIBLE_FORMAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{Cf}\p{Default_Ignorable_Code_Point}]").expect("Unicode display properties")
});
static EXCESS_MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{M}{4,}").unwrap());
static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{M}").unwrap());
pub fn display_text(text: &str) -> String {
    let visible = INVISIBLE_FORMAT.replace_all(text, "");
    EXCESS_MARKS
        .replace_all(&visible, |marks: &regex::Captures<'_>| {
            marks[0].chars().take(3).collect::<String>()
        })
        .into_owned()
}
pub const PROVIDER_TEXT_CHARS: usize = 400;
pub struct DisplayLine {
    pub text: String,
    pub truncated: bool,
}
pub fn display_line(text: &str, previously_truncated: bool) -> DisplayLine {
    let end = text
        .char_indices()
        .nth(PROVIDER_TEXT_CHARS)
        .map(|(index, _)| index);
    let bounded = &text[..end.unwrap_or(text.len())];
    let projected: String = display_text(bounded)
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let mut text = projected.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated = previously_truncated || end.is_some();
    if truncated {
        text.push('…');
    }
    DisplayLine { text, truncated }
}
pub fn consent_multiline(text: &str) -> String {
    consent_projection(text, |ch| ch == '\n')
}
pub fn consent_line(text: &str) -> String {
    consent_projection(text, |_| false)
}
fn consent_projection(text: &str, preserve: impl Fn(char) -> bool) -> String {
    let mut projected = String::new();
    let mut marks = 0;
    for ch in text.chars() {
        marks = if MARK.is_match(ch.encode_utf8(&mut [0; 4])) {
            marks + 1
        } else {
            0
        };
        if preserve(ch) {
            projected.push(ch);
        } else if marks > 3
            || ch.is_control()
            || matches!(ch, '\u{2028}' | '\u{2029}')
            || INVISIBLE_FORMAT.is_match(ch.encode_utf8(&mut [0; 4]))
        {
            use std::fmt::Write;
            write!(projected, "⟨U+{:04X}⟩", ch as u32).unwrap();
        } else {
            projected.push(ch);
        }
    }
    projected
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_draws_at_most_three_combining_marks_per_base() {
        let raw = format!("a{}b{}", "\u{0301}".repeat(7), "\u{0323}".repeat(5));
        let expected = format!("a{}b{}", "\u{0301}".repeat(3), "\u{0323}".repeat(3));
        assert_eq!(display_text(&raw), expected);
        assert_eq!(display_line(&raw, false).text, expected);
        let consent = format!(
            "a{}{}b{}{}",
            "\u{0301}".repeat(3),
            "⟨U+0301⟩".repeat(4),
            "\u{0323}".repeat(3),
            "⟨U+0323⟩".repeat(2)
        );
        assert_eq!(consent_multiline(&raw), consent);
        assert_eq!(consent_line(&raw), consent);
        assert_eq!(display_text("Vio\u{202e}let\nnext"), "Violet\nnext");
        assert_eq!(raw.chars().count(), 14);
    }
    #[test]
    fn consent_seven_marks_keeps_original_request_and_sent_bytes() {
        use crate::{
            domain::*,
            provider::{ProviderEvent, TurnEvent, claude::Codec},
        };
        let raw = format!("printf e{}", "\u{0301}".repeat(7));
        assert_eq!(
            consent_multiline(&raw),
            format!("printf e{}{}", "\u{0301}".repeat(3), "⟨U+0301⟩".repeat(4))
        );
        let mut codec = Codec::new(
            NativeSessionId("fixture".into()),
            1,
            std::path::Path::new("/project"),
        );
        let sent = codec.start(TurnId::new(), raw.clone()).unwrap();
        assert_eq!(
            sent.controls[0].value()["message"]["content"]
                .as_str()
                .unwrap()
                .as_bytes(),
            raw.as_bytes()
        );
        let decoded = codec
            .value(
                serde_json::json!({"type":"control_request", "request_id":"marks",
            "request":{"subtype":"can_use_tool", "tool_name":"Bash", "input":{"command":raw}}}),
            )
            .unwrap();
        let command = decoded
            .events
            .iter()
            .find_map(|event| match event {
                ProviderEvent::Turn {
                    event:
                        TurnEvent::RequestOpened {
                            kind: RequestKind::Approval { action },
                            ..
                        },
                    ..
                } => {
                    if let ApprovalTarget::Command { command, .. } = &action.target {
                        Some(command)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(command.as_bytes(), raw.as_bytes());
        let reply = codec
            .answer(NativeRequestRef("marks".into()), Decision::AllowOnce)
            .unwrap();
        assert_eq!(
            reply["response"]["response"],
            serde_json::json!({"behavior":"allow"})
        );
        codec.value(serde_json::json!({"type":"control_request", "request_id":"answer",
            "request":{"subtype":"can_use_tool", "tool_name":"AskUserQuestion", "input":{
                "questions":[{"question":"Which command?", "options":[{"label":raw},{"label":"Skip"}]}]}}})).unwrap();
        let reply = codec
            .answer(
                NativeRequestRef("answer".into()),
                Decision::Answers(vec![Answer { text: raw.clone() }]),
            )
            .unwrap();
        assert_eq!(
            reply["response"]["response"]["updatedInput"]["answers"]["Which command?"]
                .as_str()
                .unwrap()
                .as_bytes(),
            raw.as_bytes()
        );
    }
    #[test]
    fn provider_text_collapses_whitespace_and_has_its_own_cap() {
        assert_eq!(display_line("a  \n\t b", false).text, "a b");
        let line = display_line(&"🟣".repeat(1000), false);
        assert!(line.text.chars().count() <= 401);
        assert!(line.text.ends_with('…'));
        assert!(line.truncated);
    }
    #[test]
    fn display_removes_formats_replaces_controls_and_keeps_bound() {
        let line = display_line(
            "a\n\t\r\u{202e}\u{200b}\u{061c}\u{2066}\u{2069}\u{feff}\u{034f}\u{2028}\u{2029}b",
            false,
        );
        assert_eq!(line.text, "a b");
        assert!(!line.truncated);
        let line = display_line(&"🟣".repeat(10_001), false);
        assert_eq!(line.text.chars().count(), PROVIDER_TEXT_CHARS + 1);
        assert!(line.truncated);
        assert!(display_line("short", true).truncated);
    }
}
