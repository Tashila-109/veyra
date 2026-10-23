use super::{
    actions::{SetDefaultMode, SetWindowBackground},
    composer::labelled_button,
    theme::Theme,
};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::StyledExt;
use gpui_kit::component::button::Button;
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::{PermissionMode, Settings, WindowBackground};

pub const MODES: [PermissionMode; 3] = [
    PermissionMode::AskFirst,
    PermissionMode::AcceptEdits,
    PermissionMode::FullAccess,
];
pub fn next_mode(mode: PermissionMode) -> PermissionMode {
    match mode {
        PermissionMode::AskFirst => PermissionMode::AcceptEdits,
        PermissionMode::AcceptEdits => PermissionMode::FullAccess,
        PermissionMode::FullAccess => PermissionMode::AskFirst,
    }
}
pub fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AskFirst => "Ask first",
        PermissionMode::AcceptEdits => "Accept edits",
        PermissionMode::FullAccess => "Full access",
    }
}
fn description(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AskFirst => {
            concat!(
                "Asks before it edits files. Asks before commands the CLI does not treat as safe. ",
                "Commands can use the network."
            )
        }
        PermissionMode::AcceptEdits => {
            concat!(
                "Edits files without asking. Asks before commands the CLI does not treat as safe. ",
                "Commands can use the network."
            )
        }
        PermissionMode::FullAccess => {
            "Runs every edit and command without asking, inside and outside the project folder, with no sandbox."
        }
    }
}
pub fn render(settings: Settings, notice: Option<SharedString>, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    div()
        .v_flex()
        .flex_1()
        .items_center()
        .px(t.layout.gutter)
        .pt(t.space.0[9])
        .child(
            div()
                .v_flex()
                .w_full()
                .max_w(t.layout.composer_max)
                .pl(t.space.0[5])
                .pr(t.space.0[5])
                .gap(t.space.0[5])
                .child(
                    div()
                        .text_size(t.text.h2.size)
                        .line_height(t.text.h2.line_height)
                        .font_weight(t.text.h2.weight)
                        .child("Default mode for new threads"),
                )
                .child(div().h_flex().gap(t.layout.segment_padding).children(
                    MODES.into_iter().enumerate().map(|(index, mode)| {
                        labelled_button(("default-mode", index), mode_label(mode), cx)
                            .selected(mode == settings.default_mode)
                            .when(mode == settings.default_mode, |button| {
                                button
                                    .bg(t.plate(t.colors.card))
                                    .border(t.layout.border)
                                    .border_color(t.colors.border)
                                    .shadow(t.plate_highlight())
                            })
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(SetDefaultMode { mode: Some(mode) }),
                                    cx,
                                )
                            })
                    }),
                ))
                .child(
                    div()
                        .text_size(t.text.read.size)
                        .line_height(t.text.read.line_height)
                        .child(description(settings.default_mode)),
                )
                .child(
                    div()
                        .text_size(t.text.meta.size)
                        .line_height(t.text.meta.line_height)
                        .text_color(t.colors.text_muted)
                        .child("Applies to threads you create from now on"),
                )
                .child(
                    div()
                        .mt(t.space.0[7])
                        .text_size(t.text.h2.size)
                        .line_height(t.text.h2.line_height)
                        .font_weight(t.text.h2.weight)
                        .child("Window background"),
                )
                .child(
                    div().h_flex().gap(t.layout.segment_padding).children(
                        [
                            (WindowBackground::Translucent, "Translucent"),
                            (WindowBackground::Solid, "Solid"),
                        ]
                        .into_iter()
                        .map(|(background, label)| background_button(background, label, cx)),
                    ),
                )
                .child(
                    div()
                        .text_size(t.text.meta.size)
                        .line_height(t.text.meta.line_height)
                        .text_color(t.colors.text_muted)
                        .child(
                            t.look
                                .forced_by()
                                .map(|flag| {
                                    format!("Solid is required by {flag}. Your choice is saved.")
                                })
                                .unwrap_or_else(|| {
                                    "Translucent shows your desktop softly through the sidebar."
                                        .into()
                                }),
                        ),
                ),
        )
        .when_some(notice, |this, notice| {
            this.child(
                div()
                    .text_size(t.text.meta.size)
                    .line_height(t.text.meta.line_height)
                    .text_color(t.colors.danger)
                    .child(notice),
            )
        })
}

pub(super) fn background_button(
    background: WindowBackground,
    label: &'static str,
    cx: &App,
) -> Button {
    let t = cx.global::<Theme>();
    let selected = background == t.look.background();
    labelled_button(label, label, cx)
        .selected(selected)
        .disabled(t.look.forced_by().is_some())
        .when(selected, |button| {
            button
                .bg(t.plate(t.colors.card))
                .border(t.layout.border)
                .border_color(t.colors.border)
                .shadow(t.plate_highlight())
        })
        .on_click(move |_, window, cx| {
            window.dispatch_action(
                Box::new(SetWindowBackground {
                    background: Some(background),
                }),
                cx,
            )
        })
}
