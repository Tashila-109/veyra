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
#[derive(Clone, Copy)]
enum QuoteScroll<'a> {
    Folder(&'a ScrollHandle),
    Question(&'a ScrollHandle),
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
        self.render_with_scroll(selector, Some(QuoteScroll::Folder(handle)), cx)
    }
    pub fn render_question(self, handle: &ScrollHandle, cx: &App) -> AnyElement {
        self.render_with_scroll(
            "dock-provider-question",
            Some(QuoteScroll::Question(handle)),
            cx,
        )
    }
    fn render_with_scroll(
        self,
        selector: &'static str,
        scroll: Option<QuoteScroll<'_>>,
        cx: &App,
    ) -> AnyElement {
        let t = cx.global::<Theme>();
        let (text_selector, region_minimum, line_height, viewport) = match scroll {
            Some(QuoteScroll::Folder(handle)) => (
                "approval-working-folder-text".to_owned(),
                Length::Auto,
                Some(t.text.label.line_height),
                Some((
                    "working-folder",
                    "approval-working-folder-scroll",
                    handle,
                    t.text.label.line_height.into(),
                    (t.text.label.line_height * 3.).into(),
                )),
            ),
            Some(QuoteScroll::Question(handle)) => (
                format!("{selector}-value"),
                t.layout.dock_question_min_height.into(),
                Some(t.text.ui.line_height),
                Some((
                    "question-text",
                    "dock-question-scroll",
                    handle,
                    t.text.ui.line_height.into(),
                    t.layout.composer_max_height.into(),
                )),
            ),
            None => (format!("{selector}-value"), Length::Auto, None, None),
        };
        let question = matches!(self.kind, QuoteKind::Question);
        let reason = matches!(self.kind, QuoteKind::Reason);
        let text = if question {
            self.text
                .strip_prefix('“')
                .and_then(|text| text.strip_suffix('”'))
                .unwrap_or(&self.text)
                .to_owned()
        } else {
            self.text
        };
        let value = div()
            .when(reason, |value| value.flex_1().min_w_0())
            .pl(t.space.0[3])
            .border_l(t.layout.border)
            .border_color(t.colors.border)
            .debug_selector(move || text_selector.clone())
            .child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("{selector}-content"))
                    .child(text),
            );
        let value = if let Some((id, viewport, handle, min, max)) = viewport {
            bounded_scroll(id, viewport, handle, value, min, max, cx).into_any_element()
        } else {
            value.into_any_element()
        };
        div()
            .debug_selector(move || selector.into())
            .min_h(region_minimum)
            .v_flex()
            .when(reason, |row| row.h_flex().items_start())
            .when_some(line_height, |value, height| value.line_height(height))
            .gap(t.space.0[1])
            .text_size(t.text.label.size)
            .text_color(t.colors.text_muted)
            .child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("{selector}-label"))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        div()
                            .flex_none()
                            .debug_selector(move || format!("{selector}-label-content"))
                            .child(self.label),
                    ),
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
    min_height: Length,
    max_height: Length,
    cx: &App,
) -> Div {
    let t = cx.global::<Theme>();
    div()
        .relative()
        .v_flex()
        .w_full()
        .min_h(min_height)
        .max_h(max_height)
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
            div().absolute().inset_0().child(
                Scrollbar::vertical(handle)
                    .mode(ScrollbarMode::Always)
                    .styles(|style| style.thumb(|thumb| thumb.min_length(t.space.0[5]))),
            ),
        )
}
#[derive(serde::Serialize)]
pub(super) struct QuestionDisplay {
    pub title: String,
    pub options: Vec<String>,
}
pub(super) fn question_display(question: &AsyncQuestion) -> QuestionDisplay {
    QuestionDisplay {
        title: display_line(&question.title, false).text,
        options: question
            .options
            .iter()
            .map(|option| display_line(option, false).text)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn one_line_overflow_has_a_short_moving_scrollbar_thumb(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Probe(ScrollHandle);
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let t = cx.global::<super::Theme>();
                super::bounded_scroll(
                    "thumb-probe",
                    "thumb-probe-scroll",
                    &self.0,
                    div()
                        .line_height(t.text.code.line_height)
                        .child("line\n".repeat(100)),
                    t.text.code.line_height.into(),
                    t.text.code.line_height.into(),
                    cx,
                )
            }
        }
        cx.update(gpui_kit::init);
        let (probe, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Probe(ScrollHandle::new())
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..2 {
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
            }
        };
        draw(cx);
        let viewport = cx.debug_bounds("thumb-probe-scroll").unwrap();
        let thumb = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| {
                let track = viewport.scale(window.scale_factor());
                window.painted_quads().iter().find_map(|quad| {
                    (!quad.background.is_transparent()
                        && quad.bounds.left() > track.center().x
                        && quad.bounds.right() <= track.right()
                        && quad.bounds.top() >= track.top()
                        && quad.bounds.bottom() <= track.bottom()
                        && quad.bounds.size.width < track.size.width / 2.
                        && quad.bounds.size.height.0 > 0.
                        && quad.bounds.size.height < track.size.height)
                        .then_some(quad.bounds)
                })
            })
        };
        let first = thumb(cx).expect("overflow thumb must be shorter than a one-line track");
        probe.update(cx, |probe, _| {
            assert!(probe.0.max_offset().y > Pixels::ZERO);
            probe
                .0
                .set_offset(point(Pixels::ZERO, -probe.0.max_offset().y));
        });
        draw(cx);
        let last = thumb(cx).unwrap();
        assert!(
            last.top() > first.top(),
            "the thumb must travel when scrolling"
        );
    }

    #[test]
    fn quotes_have_the_provider_text_cap_and_collapsed_spaces() {
        assert_eq!(super::quoted("one   \n\t two", false), "“one two”");
        let text = super::quoted(&"a".repeat(1000), false);
        assert!(text.starts_with('“'));
        assert!(text.ends_with("…” [truncated]"));
        assert_eq!(text, format!("“{}…” [truncated]", "a".repeat(400)));
    }
}
