use super::{actions::*, theme::Theme};
use gpui_kit::component::{
    StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::*;

const DEFAULT_NOTICE: &str =
    "Add a project to start a thread. Agent connections arrive in a later milestone.";

pub fn render(notice: Option<SharedString>, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    div()
        .v_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(t.colors.bg)
        .child(
            div()
                .h(t.layout.header)
                .flex_none()
                .h_flex()
                .items_center()
                .px(t.space.0[6])
                .border_b(t.layout.border)
                .border_color(t.colors.border)
                .child("New thread")
                .child(div().flex_1())
                .child(
                    Button::new("new-thread")
                        .ghost()
                        .h(t.layout.icon_button)
                        .px(t.space.0[3])
                        .accessibility_label("New thread  ⌘N")
                        .child(
                            div()
                                .text_size(t.text.label.size)
                                .line_height(t.text.label.line_height)
                                .font_weight(t.text.label.weight)
                                .child("New thread  ⌘N"),
                        )
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(NewThread), cx)),
                ),
        )
        .child(
            div()
                .flex_1()
                .v_flex()
                .justify_center()
                .items_center()
                .px(t.layout.gutter)
                .child(
                    div()
                        .v_flex()
                        .w_full()
                        .max_w(t.layout.composer_max)
                        .gap(t.space.0[4])
                        .child(
                            div()
                                .text_size(t.text.h2.size)
                                .line_height(t.text.h2.line_height)
                                .font_weight(t.text.h2.weight)
                                .child("What should the agents work on?"),
                        )
                        .child(
                            div()
                                .v_flex()
                                .w_full()
                                .rounded(t.radius.large)
                                .bg(t.colors.card)
                                .border(t.layout.border)
                                .border_color(t.colors.border)
                                .p(t.space.0[4])
                                .child(
                                    div()
                                        .h(t.layout.composer_min_height)
                                        .text_size(t.text.read.size)
                                        .line_height(t.text.read.line_height)
                                        .text_color(t.colors.text_faint)
                                        .child("Describe the task"),
                                )
                                .child(
                                    div()
                                        .h_flex()
                                        .h(t.layout.composer_toolbar)
                                        .items_center()
                                        .gap(t.space.0[1])
                                        .child(
                                            Button::new("provider")
                                                .ghost()
                                                .h(t.layout.icon_button)
                                                .px(t.space.0[3])
                                                .accessibility_label("Claude Code")
                                                .child(
                                                    div()
                                                        .text_size(t.text.label.size)
                                                        .line_height(t.text.label.line_height)
                                                        .font_weight(t.text.label.weight)
                                                        .child("Claude Code"),
                                                )
                                                .on_click(|_, w, cx| {
                                                    w.dispatch_action(Box::new(PickProvider), cx)
                                                }),
                                        )
                                        .child(
                                            Button::new("model")
                                                .ghost()
                                                .h(t.layout.icon_button)
                                                .px(t.space.0[3])
                                                .accessibility_label("Model")
                                                .child(
                                                    div()
                                                        .text_size(t.text.label.size)
                                                        .line_height(t.text.label.line_height)
                                                        .font_weight(t.text.label.weight)
                                                        .child("Model"),
                                                )
                                                .on_click(|_, w, cx| {
                                                    w.dispatch_action(Box::new(PickModel), cx)
                                                }),
                                        )
                                        .child(
                                            Button::new("effort")
                                                .ghost()
                                                .h(t.layout.icon_button)
                                                .px(t.space.0[3])
                                                .accessibility_label("High")
                                                .child(
                                                    div()
                                                        .text_size(t.text.label.size)
                                                        .line_height(t.text.label.line_height)
                                                        .font_weight(t.text.label.weight)
                                                        .child("High"),
                                                )
                                                .on_click(|_, w, cx| {
                                                    w.dispatch_action(Box::new(PickEffort), cx)
                                                }),
                                        )
                                        .child(
                                            Button::new("permission")
                                                .ghost()
                                                .h(t.layout.icon_button)
                                                .px(t.space.0[3])
                                                .accessibility_label("Ask first")
                                                .child(
                                                    div()
                                                        .text_size(t.text.label.size)
                                                        .line_height(t.text.label.line_height)
                                                        .font_weight(t.text.label.weight)
                                                        .child("Ask first"),
                                                )
                                                .on_click(|_, w, cx| {
                                                    w.dispatch_action(Box::new(PickPermission), cx)
                                                }),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            Button::new("send")
                                                .primary()
                                                .compact()
                                                .rounded(t.radius.full)
                                                .w(t.layout.icon_button)
                                                .h(t.layout.icon_button)
                                                .p(t.space.0[0])
                                                .accessibility_label("Send")
                                                .child(
                                                    div()
                                                        .text_size(t.text.label.size)
                                                        .line_height(t.text.label.line_height)
                                                        .font_weight(t.text.label.weight)
                                                        .child("↑"),
                                                )
                                                .on_click(|_, w, cx| {
                                                    w.dispatch_action(Box::new(Send), cx)
                                                }),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .text_size(t.text.meta.size)
                                .line_height(t.text.meta.line_height)
                                .text_color(t.colors.text_faint)
                                .child(notice.unwrap_or_else(|| DEFAULT_NOTICE.into())),
                        ),
                ),
        )
}
