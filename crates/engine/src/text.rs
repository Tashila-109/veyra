use regex::Regex;
use std::sync::LazyLock;

static INVISIBLE_FORMAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{Cf}\p{Default_Ignorable_Code_Point}]").expect("Unicode display properties")
});
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
    let projected: String = INVISIBLE_FORMAT
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
    let mut text = projected.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated = previously_truncated || end.is_some();
    if truncated {
        text.push('…');
    }
    DisplayLine { text, truncated }
}
pub fn consent_text(text: &str, keep_newlines: bool) -> String {
    let mut projected = String::new();
    for ch in text.chars() {
        if keep_newlines && ch == '\n' {
            projected.push(ch);
        } else if ch.is_control()
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
