use super::{Shell, composer, theme::Theme};
use gpui_kit::component::StyledExt;
use gpui_kit::*;
pub fn render(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let t = cx.global::<Theme>();
    div()
        .v_flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .px(t.layout.gutter)
        .child(div().flex_1())
        .child(
            div()
                .v_flex()
                .w_full()
                .max_w(t.layout.composer_max)
                .gap(t.space.0[5])
                .child(
                    div()
                        .text_size(t.text.h2.size)
                        .line_height(t.text.h2.line_height)
                        .font_weight(t.text.h2.weight)
                        .child("What should the agents work on?"),
                )
                .child(composer::render(shell, cx))
                .child(
                    div()
                        .text_size(t.text.meta.size)
                        .line_height(t.text.meta.line_height)
                        .text_color(t.colors.text_faint)
                        .child(if shell.chosen_provider() == super::ProviderKind::Grok {
                            "The CLI loads your own hooks and MCP servers. Grok follows your Claude and Grok permission settings, so Ask first asks only when they do not already allow the action."
                        } else {
                            "The CLI loads your own hooks and MCP servers."
                        }),
                ),
        )
        .child(div().flex_grow(2.))
}
