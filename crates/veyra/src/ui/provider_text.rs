use super::theme::Theme;
use gpui_kit::component::{
    StyledExt,
    scroll::{Scrollbar, ScrollbarMode},
};
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
impl QuoteKind {
    pub fn selector(&self) -> &'static str {
        match self {
            Self::Reason => "approval-provider-reason",
            Self::WorkingFolder => "approval-working-folder",
            Self::Tool => "approval-provider-tool",
            Self::File => "approval-provider-file",
            Self::Question => "approval-provider-text",
        }
    }
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
        self.render_with_scroll(selector, None, cx)
    }
    pub fn render_folder(self, handle: &ScrollHandle, cx: &App) -> AnyElement {
        let selector = self.kind.selector();
        self.render_with_scroll(selector, Some(handle), cx)
    }
    fn render_with_scroll(
        self,
        selector: &'static str,
        folder: Option<&ScrollHandle>,
        cx: &App,
    ) -> AnyElement {
        let t = cx.global::<Theme>();
        let value = div()
            .pl(t.space.0[3])
            .border_l(t.layout.border)
            .border_color(t.colors.border)
            .child(self.text);
        let value = if let Some(handle) = folder {
            bounded_scroll(
                "working-folder",
                "approval-working-folder-scroll",
                handle,
                div()
                    .line_height(t.text.label.line_height)
                    .child(value.debug_selector(|| "approval-working-folder-text".into())),
                cx,
            )
            .min_h(t.text.label.line_height)
            .max_h(t.text.label.line_height * 3.)
            .into_any_element()
        } else {
            value.into_any_element()
        };
        div()
            .debug_selector(move || selector.into())
            .v_flex()
            .min_h_0()
            .when(folder.is_some(), |value| value.min_h_auto())
            .gap(t.space.0[1])
            .text_size(t.text.label.size)
            .line_height(t.text.label.line_height)
            .text_color(t.colors.text_muted)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.label),
            )
            .child(value)
            .into_any_element()
    }
}
pub(super) fn bounded_scroll(
    id: &'static str,
    selector: &'static str,
    handle: &ScrollHandle,
    content: Div,
    cx: &App,
) -> Div {
    let t = cx.global::<Theme>();
    div()
        .relative()
        .v_flex()
        .w_full()
        .min_h_0()
        .child(
            div()
                .id(id)
                .debug_selector(move || selector.into())
                .w_full()
                .min_w_0()
                .min_h_0()
                .v_flex()
                .track_scroll(handle)
                .overflow_y_scroll()
                .pr(t.space.0[4])
                .child(content.flex_none()),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(handle).mode(ScrollbarMode::Always)),
        )
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
