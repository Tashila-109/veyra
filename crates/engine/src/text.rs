use regex::Regex;
use std::sync::LazyLock;

static INVISIBLE_FORMAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{Cf}\p{Default_Ignorable_Code_Point}]").expect("Unicode display properties")
});
pub struct DisplayLine {
    pub text: String,
    pub truncated: bool,
}
pub fn display_line(text: &str, previously_truncated: bool) -> DisplayLine {
    let end = text
        .char_indices()
        .nth(crate::domain::TOOL_OUTPUT_CHARS)
        .map(|(index, _)| index);
    let bounded = &text[..end.unwrap_or(text.len())];
    let text = INVISIBLE_FORMAT
        .replace_all(bounded, "")
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                ch
            }
        })
        .collect();
    DisplayLine {
        text,
        truncated: previously_truncated || end.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_removes_formats_replaces_controls_and_keeps_bound() {
        let line = display_line(
            "a\n\t\r\u{202e}\u{200b}\u{061c}\u{2066}\u{2069}\u{feff}\u{034f}\u{2028}\u{2029}b",
            false,
        );
        assert_eq!(line.text, "a     b");
        assert!(!line.truncated);
        let line = display_line(&"🟣".repeat(10_001), false);
        assert_eq!(line.text.chars().count(), 10_000);
        assert!(line.truncated);
        assert!(display_line("short", true).truncated);
    }
}
