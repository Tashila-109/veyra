use super::theme::Theme;
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
use veyra_engine::{domain::AsyncQuestion, text::display_line};

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum QuoteKind {
    Reason,
    WorkingFolder,
    Tool,
    File,
    Question,
}
#[derive(serde::Serialize)]
pub(super) struct QuotedText {
    pub kind: QuoteKind,
    pub label: String,
    pub text: String,
}
pub(super) fn quoted(value: &str, truncated: bool) -> String {
    let line = display_line(value, truncated);
    format!(
        "“{}”{}",
        line.text,
        if line.truncated { " [truncated]" } else { "" }
    )
}
impl QuotedText {
    pub fn new(kind: QuoteKind, label: String, value: &str, truncated: bool) -> Self {
        Self::from_quoted(kind, label, quoted(value, truncated))
    }
    pub fn from_quoted(kind: QuoteKind, label: String, text: String) -> Self {
        Self { kind, label, text }
    }
    pub fn render(self, selector: &'static str, cx: &App) -> AnyElement {
        let t = cx.global::<Theme>();
        div()
            .debug_selector(move || selector.into())
            .v_flex()
            .gap(t.space.0[1])
            .text_size(t.text.label.size)
            .text_color(t.colors.text_muted)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.label))
            .child(
                div()
                    .pl(t.space.0[3])
                    .border_l(t.layout.border)
                    .border_color(t.colors.border)
                    .child(self.text),
            )
            .into_any_element()
    }
}
#[derive(serde::Serialize)]
pub(super) struct QuestionDisplay {
    pub title: String,
    pub options: Vec<String>,
}
pub(super) fn question_display(question: &AsyncQuestion) -> QuestionDisplay {
    QuestionDisplay {
        title: quoted(&question.title, false),
        options: question
            .options
            .iter()
            .map(|option| quoted(option, false))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_have_the_provider_text_cap_and_collapsed_spaces() {
        assert_eq!(super::quoted("one   \n\t two", false), "“one two”");
        let text = super::quoted(&"a".repeat(1000), false);
        assert!(text.starts_with('“'));
        assert!(text.ends_with("…” [truncated]"));
        assert_eq!(text, format!("“{}…” [truncated]", "a".repeat(400)));
    }
}
