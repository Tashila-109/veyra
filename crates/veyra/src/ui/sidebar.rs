use super::{actions::*, theme::Theme};
use gpui_kit::component::{
    StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::*;

pub fn render(width: Pixels, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    div()
        .v_flex()
        .w(width)
        .h_full()
        .flex_none()
        .bg(t.colors.shell)
        .child(div().h(t.layout.header).flex_none())
        .child(
            div()
                .v_flex()
                .p(t.space.0[4])
                .gap(t.space.0[4])
                .child(
                    Button::new("add-project")
                        .ghost()
                        .label("+  Add project")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(AddProject), cx)),
                )
                .child(
                    div()
                        .text_size(t.text.label.size)
                        .text_color(t.colors.text_faint)
                        .child("Your projects appear here."),
                ),
        )
}
