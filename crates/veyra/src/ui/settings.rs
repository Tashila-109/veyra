use super::{actions::SetDefaultMode, composer::labelled_button, theme::Theme};
use gpui_kit::base::Selectable;
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::{PermissionMode, Settings};

pub const MODES: [PermissionMode; 3] = [
    PermissionMode::AskFirst,
    PermissionMode::AcceptEdits,
    PermissionMode::FullAccess,
];
pub fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AskFirst => "Ask first",
        PermissionMode::AcceptEdits => "Accept edits",
        PermissionMode::FullAccess => "Full access",
    }
}
pub fn mode_word(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AskFirst => "ask_first",
        PermissionMode::AcceptEdits => "accept_edits",
        PermissionMode::FullAccess => "full_access",
    }
}
fn description(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AskFirst => {
            "Asks before it edits files. Asks before commands the CLI does not treat as safe. Commands can use the network."
        }
        PermissionMode::AcceptEdits => {
            "Edits files without asking. Asks before commands the CLI does not treat as safe. Commands can use the network."
        }
        PermissionMode::FullAccess => {
            "Runs every edit and command without asking, inside and outside the project folder, with no sandbox."
        }
    }
}
pub fn parse_mode(
    word: Option<&str>,
    current: PermissionMode,
) -> Result<PermissionMode, &'static str> {
    match word {
        Some(word) => MODES
            .into_iter()
            .find(|mode| mode_word(*mode) == word)
            .ok_or("Unknown permission mode"),
        None => Ok(match current {
            PermissionMode::AskFirst => PermissionMode::AcceptEdits,
            PermissionMode::AcceptEdits => PermissionMode::FullAccess,
            PermissionMode::FullAccess => PermissionMode::AskFirst,
        }),
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
                .max_w(t.layout.transcript_max)
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
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(SetDefaultMode {
                                        mode: Some(mode_word(mode).into()),
                                    }),
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
