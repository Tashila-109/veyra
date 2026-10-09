use super::{MainView, Shell, actions::*, settings, theme::Theme};
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    StyledExt,
    button::{Button, ButtonVariants},
    input::Textarea,
    menu::DropdownMenu,
};
use gpui_kit::{prelude::*, *};
use veyra_engine::provider::{ProviderKind, Readiness};

pub fn labelled_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Button {
    let t = cx.global::<Theme>();
    let label = label.into();
    Button::new(id)
        .ghost()
        .h(t.layout.icon_button)
        .px(t.space.0[3])
        .accessibility_label(label.clone())
        .child(
            div()
                .text_size(t.text.label.size)
                .line_height(t.text.label.line_height)
                .font_weight(t.text.label.weight)
                .child(label),
        )
}
pub fn render(shell: &Shell, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let config = shell.composer_config().clone();
    let provider = shell.chosen_provider();
    let provider_rows: Vec<_> = [
        ProviderKind::Claude,
        ProviderKind::Codex,
        ProviderKind::Grok,
    ]
    .into_iter()
    .map(|provider| {
        let state = shell
            .providers
            .iter()
            .find(|probe| probe.provider == provider)
            .map(|probe| &probe.readiness)
            .unwrap_or(&Readiness::Checking);
        let state = match state {
            Readiness::Ready => "Ready",
            Readiness::NotInstalled => "Not installed",
            Readiness::SignedOut => "Signed out",
            Readiness::Checking => "Checking…",
        };
        (provider, format!("{} · {state}", provider.label()))
    })
    .collect();
    let models = shell
        .providers
        .iter()
        .find(|probe| probe.provider == provider)
        .map(|probe| probe.models.clone())
        .unwrap_or_default();
    let efforts = models
        .iter()
        .find(|model| Some(&model.value) == config.model.as_ref())
        .or_else(|| models.first())
        .map(|model| model.effort_levels.clone())
        .unwrap_or_default();
    let provider_button = labelled_button("provider", provider.label(), cx)
        .disabled(shell.view != MainView::NewThread)
        .dropdown_menu(move |mut menu, _, _| {
            for (provider, label) in &provider_rows {
                menu = menu.menu(
                    label.clone(),
                    Box::new(PickProvider {
                        provider: Some(provider.binary().into()),
                    }),
                );
            }
            menu
        });
    let model_button = labelled_button(
        "model",
        config
            .model
            .clone()
            .unwrap_or_else(|| "Default model".into()),
        cx,
    )
    .dropdown_menu(move |mut menu, _, _| {
        for model in &models {
            menu = menu.menu(
                model.display_name.clone(),
                Box::new(PickModel {
                    model: Some(model.value.clone()),
                }),
            );
        }
        menu
    });
    let effort_button = labelled_button(
        "effort",
        config.effort.clone().unwrap_or_else(|| "Effort".into()),
        cx,
    )
    .dropdown_menu(move |mut menu, _, _| {
        for effort in &efforts {
            menu = menu.menu(
                effort.clone(),
                Box::new(PickEffort {
                    effort: Some(effort.clone()),
                }),
            );
        }
        menu
    });
    let permission_button = labelled_button("permission", settings::mode_label(config.mode), cx)
        .dropdown_menu(|mut menu, _, _| {
            for mode in settings::MODES {
                menu = menu.menu(
                    settings::mode_label(mode),
                    Box::new(PickPermission {
                        mode: Some(settings::mode_word(mode).into()),
                    }),
                );
            }
            menu
        });
    let reason = shell.disabled_reason();
    div()
        .v_flex()
        .w_full()
        .max_w(t.layout.composer_max)
        .gap(t.space.0[3])
        .child(
            div()
                .v_flex()
                .w_full()
                .rounded(t.radius.large)
                .bg(t.colors.card)
                .border(t.layout.border)
                .border_color(t.colors.border)
                .p(t.space.0[5])
                .child(
                    Textarea::new(&shell.input)
                        .appearance(false)
                        .bordered(false)
                        .w_full()
                        .min_h(t.layout.composer_min_height)
                        .max_h(t.layout.composer_max_height)
                        .text_size(t.text.read.size)
                        .line_height(t.text.read.line_height)
                        .aria_label("Message"),
                )
                .child(
                    div()
                        .h_flex()
                        .h(t.layout.composer_toolbar)
                        .items_center()
                        .gap(t.space.0[1])
                        .child(provider_button)
                        .child(model_button)
                        .child(effort_button)
                        .child(permission_button)
                        .child(div().flex_1())
                        .child(
                            labelled_button("send", "Send", cx)
                                .primary()
                                .disabled(reason.is_some())
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(Send), cx)
                                }),
                        ),
                ),
        )
        .when_some(reason, |this, reason| {
            this.child(
                div()
                    .text_size(t.text.meta.size)
                    .line_height(t.text.meta.line_height)
                    .text_color(t.colors.text_muted)
                    .child(reason),
            )
            .when(!shell.ready(provider), |this| {
                this.child(labelled_button("check-again", "Check again", cx).on_click(
                    |_, window, cx| window.dispatch_action(Box::new(RecheckProviders), cx),
                ))
            })
        })
        .when_some(shell.notice.clone(), |this, notice| {
            this.child(
                div()
                    .text_size(t.text.meta.size)
                    .line_height(t.text.meta.line_height)
                    .text_color(t.colors.danger)
                    .child(notice),
            )
        })
}
