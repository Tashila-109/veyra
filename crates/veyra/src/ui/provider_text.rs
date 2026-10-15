use super::theme::Theme;
use gpui_kit::component::{
    StyledExt,
    scroll::{Scrollbar, ScrollbarMode},
};
use gpui_kit::{prelude::*, *};
use veyra_engine::{
    domain::{AsyncQuestion, RequestId},
    text::display_line,
};

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
    pub fn render_folder(self, request: RequestId, handle: &ScrollHandle, cx: &App) -> AnyElement {
        let selector = self.kind.selector();
        self.render_with_scroll(selector, Some((request, handle)), cx)
    }
    fn render_with_scroll(
        self,
        selector: &'static str,
        folder: Option<(RequestId, &ScrollHandle)>,
        cx: &App,
    ) -> AnyElement {
        let t = cx.global::<Theme>();
        let value = div()
            .pl(t.space.0[3])
            .border_l(t.layout.border)
            .border_color(t.colors.border)
            .child(self.text);
        let value = if let Some((request, handle)) = folder {
            div()
                .relative()
                .w_full()
                .child(
                    div()
                        .id(SharedString::from(format!("working-folder-{request}")))
                        .debug_selector(|| "approval-working-folder-scroll".into())
                        .max_h(t.text.label.line_height * 3.)
                        .line_height(t.text.label.line_height)
                        .track_scroll(handle)
                        .overflow_y_scroll()
                        .pr(t.space.0[4])
                        .child(value.debug_selector(|| "approval-working-folder-text".into())),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::vertical(handle).mode(ScrollbarMode::Always)),
                )
                .into_any_element()
        } else {
            value.into_any_element()
        };
        div()
            .debug_selector(move || selector.into())
            .v_flex()
            .gap(t.space.0[1])
            .text_size(t.text.label.size)
            .text_color(t.colors.text_muted)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.label))
            .child(value)
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
