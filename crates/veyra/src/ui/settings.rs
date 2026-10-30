use super::{
    actions::{SetDefaultMode, SetWindowBackground},
    composer::{Button, labelled_button},
    theme::Theme,
};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::StyledExt;
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
    use gpui_kit::component::group_box::GroupBoxVariant;
    use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
    let t = cx.global::<Theme>();
    let hidden = StyleRefinement {
        display: Some(Display::None),
        ..Default::default()
    };
    let mode = SettingItem::render(move |_, _, cx| {
        let t = cx.global::<Theme>();
        let control =
            segment_track(cx).children(MODES.into_iter().enumerate().map(|(index, mode)| {
                segment_thumb(
                    labelled_button(("default-mode", index), mode_label(mode), cx)
                        .h(t.layout.segment_button),
                    mode == settings.default_mode,
                    cx,
                )
                .on_click(move |_, window, cx| {
                    window.dispatch_action(Box::new(SetDefaultMode { mode: Some(mode) }), cx)
                })
            }));
        setting_row(
            "settings-default-heading",
            "Default mode for new threads",
            format!(
                "{} Applies to threads you create from now on.",
                description(settings.default_mode)
            ),
            control,
            cx,
        )
    });
    let background = SettingItem::render(|_, _, cx| {
        let t = cx.global::<Theme>();
        let control = segment_track(cx).children(
            [
                (WindowBackground::Translucent, "Translucent"),
                (WindowBackground::Solid, "Solid"),
            ]
            .into_iter()
            .map(|(background, label)| background_button(background, label, cx)),
        );
        let description = t
            .look
            .forced_by()
            .map(|flag| format!("Solid is required by {flag}. Your choice is saved."))
            .unwrap_or_else(|| {
                "Translucent shows the desktop through the sidebar. Solid paints it.".into()
            });
        setting_row(
            "settings-background-heading",
            "Window background",
            description,
            control,
            cx,
        )
    });
    let mut group = SettingGroup::new().item(mode).item(background);
    if let Some(notice) = notice {
        group = group.item(SettingItem::render(move |_, _, cx| {
            let t = cx.global::<Theme>();
            div()
                .debug_selector(|| "settings-error".into())
                .text_size(t.text.ui.size)
                .text_color(t.colors.danger)
                .child(notice.clone())
        }));
    }
    div()
        .v_flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .px(t.layout.gutter)
        .pt(t.space.0[9])
        .child(
            div()
                .v_flex()
                .w_full()
                .max_w(t.layout.composer_max)
                .min_h_0()
                .flex_1()
                .child(
                    gpui_kit::component::setting::Settings::new("device-settings")
                        .sidebar_width(Pixels::ZERO)
                        .sidebar_size_range(Pixels::ZERO..Pixels::ZERO)
                        .sidebar_style(&hidden)
                        .header_style(&hidden)
                        .with_group_variant(GroupBoxVariant::Fill)
                        .page(
                            SettingPage::new("Settings")
                                .header_style(&hidden)
                                .group(group),
                        ),
                ),
        )
}

fn setting_row(
    selector: &'static str,
    label: &'static str,
    description: String,
    control: Div,
    cx: &App,
) -> Div {
    let t = cx.global::<Theme>();
    div()
        .v_flex()
        .w_full()
        .min_w_0()
        .gap(t.space.0[3])
        .child(
            div()
                .h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .justify_between()
                .gap(t.space.0[3])
                .child(
                    div()
                        .debug_selector(move || selector.into())
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(t.text.ui.size)
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
                .child(control.flex_none()),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .text_size(t.text.ui.size)
                .line_height(t.text.ui.line_height)
                .text_color(t.colors.text_muted)
                .child(description),
        )
}

fn segment_track(cx: &App) -> Div {
    let t = cx.global::<Theme>();
    div()
        .h_flex()
        .items_center()
        .h(t.layout.segment_height)
        .p(t.layout.segment_padding)
        .gap(t.layout.segment_padding)
        .rounded(t.radius.track)
        .bg(t.colors.card)
}

pub(super) fn background_button(
    background: WindowBackground,
    label: &'static str,
    cx: &App,
) -> Button {
    let t = cx.global::<Theme>();
    let selected = background == t.look.background();
    segment_thumb(
        labelled_button(label, label, cx).disabled(t.look.forced_by().is_some()),
        selected,
        cx,
    )
    .on_click(move |_, window, cx| {
        window.dispatch_action(
            Box::new(SetWindowBackground {
                background: Some(background),
            }),
            cx,
        )
    })
}
fn segment_thumb(button: Button, selected: bool, cx: &App) -> Button {
    button.selected(selected).when(selected, |button| {
        button.lit_plate(cx.global::<Theme>().colors.raised, cx)
    })
}

#[cfg(test)]
mod tests {
    use super::{Theme, background_button, segment_track};
    use gpui_kit::{Fill, Styled, linear_color_stop, linear_gradient};

    #[gpui::test]
    fn segments_use_card_tracks_and_lit_thumbs(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            for appearance in [
                super::super::Appearance::Dark,
                super::super::Appearance::Light,
            ] {
                cx.set_global(Theme::new(appearance));
                let t = cx.global::<Theme>();
                let mut track = segment_track(cx);
                assert_eq!(
                    track.style().background.as_ref().and_then(Fill::color),
                    Some(t.colors.card.into())
                );
                let mut thumb = background_button(t.look.background(), "Selected", cx);
                assert_eq!(
                    thumb.style().background.as_ref().and_then(Fill::color),
                    Some(linear_gradient(
                        180.,
                        linear_color_stop(
                            super::super::theme::blend(t.colors.hover, t.colors.raised),
                            0.
                        ),
                        linear_color_stop(t.colors.raised, 0.5),
                    ))
                );
                assert_eq!(thumb.style().border_color, Some(t.colors.border));
                assert_eq!(thumb.style().box_shadow, Some(t.plate_highlight()));
                let highlight = thumb.style().box_shadow.clone().unwrap();
                assert_eq!(highlight.len(), 1);
                assert!(highlight[0].inset);
                assert_eq!(highlight[0].color, t.colors.hover);
                assert_eq!(highlight[0].offset.x, gpui_kit::Pixels::ZERO);
                assert_eq!(highlight[0].offset.y, t.layout.border);
                assert_eq!(highlight[0].blur_radius, gpui_kit::Pixels::ZERO);
                assert_eq!(highlight[0].spread_radius, gpui_kit::Pixels::ZERO);
            }
        });
    }
}
