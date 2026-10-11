use super::{MainView, Shell, actions::*, settings, theme::Theme};
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    StyledExt,
    button::{Button, ButtonVariants},
    input::Textarea,
    menu::DropdownMenu,
    popover::Popover,
};
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::{ProposedAction, RequestKind, WaitReason};

pub(super) fn approval_text(
    action: &ProposedAction,
    cwd: &std::path::Path,
    provider: &str,
) -> (String, String) {
    if let Some(input) = &action.input {
        let target = input["file_path"]
            .as_str()
            .or_else(|| input["path"].as_str());
        let label = target.map_or_else(
            || action.tool.clone(),
            |path| format!("{} {path}", action.tool),
        );
        let operation = match action.tool.as_str() {
            "Write" => "writes a file",
            "Edit" | "MultiEdit" => "edits a file",
            "Read" => "reads a file",
            _ => "uses this tool",
        };
        let location = target.map_or(String::new(), |path| {
            let path = cwd.join(path);
            let resolved = path
                .canonicalize()
                .or_else(|_| {
                    path.parent()
                        .unwrap_or(cwd)
                        .canonicalize()
                        .map(|parent| parent.join(path.file_name().unwrap_or_default()))
                })
                .unwrap_or(path);
            let normalized =
                resolved
                    .components()
                    .fold(std::path::PathBuf::new(), |mut path, part| {
                        if part == std::path::Component::ParentDir {
                            path.pop();
                        } else if part != std::path::Component::CurDir {
                            path.push(part);
                        }
                        path
                    });
            if normalized.starts_with(cwd.canonicalize().unwrap_or_else(|_| cwd.to_owned())) {
                " inside the project folder".into()
            } else {
                " outside the project folder".into()
            }
        });
        (label, format!("{provider} {operation}{location}."))
    } else {
        (
            action.command.clone(),
            format!("{provider} will run this command in the project folder."),
        )
    }
}

fn dock_chip(button: Button, focused: bool, chip: impl IntoElement) -> Button {
    button.when(focused, |button| button.child(chip))
}

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
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(label),
        )
}
pub fn render(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let config = shell.composer_config().clone();
    let provider = shell.chosen_provider();
    let models = shell
        .probe(provider)
        .map(|probe| probe.models.clone())
        .unwrap_or_default();
    let efforts = models
        .iter()
        .find(|model| Some(&model.value) == config.model.as_ref())
        .or_else(|| models.first())
        .map(|model| model.effort_levels.clone())
        .unwrap_or_default();
    let this = cx.entity().downgrade();
    let menu = shell.provider_picker.clone();
    let provider_button = Popover::new("provider-picker")
        .appearance(false)
        .open(menu.is_some())
        .when_some(menu.as_ref(), |popover, menu| {
            popover.track_focus(&menu.focus_handle(cx))
        })
        .trigger(
            labelled_button("provider", provider.label(), cx)
                .disabled(shell.view != MainView::NewThread),
        )
        .on_open_change(move |open, window, cx| {
            let _ = this.update(cx, |shell, cx| shell.set_provider_picker(*open, window, cx));
        })
        .content(move |_, _, _| div().children(menu.clone()));
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
                    Box::new(PickPermission { mode: Some(mode) }),
                );
            }
            menu
        });
    let reason = shell.disabled_reason();
    let thread = shell.view.chat();
    let stop = thread.is_some_and(|thread| shell.running(thread))
        && shell.input.read(cx).value().trim().is_empty();
    let pending = thread
        .map(|thread| shell.state.pending_requests(thread))
        .unwrap_or_default();
    let request = pending.first().copied();
    let hold = thread
        .and_then(|thread| shell.state.threads.get(&thread))
        .and_then(|thread| thread.hold);
    div()
        .v_flex()
        .w_full()
        .max_w(t.layout.composer_max)
        .gap(t.space.0[3])
        .when_some(
            reason.clone().filter(|_| shell.provider_picker.is_some()),
            |this, reason| {
                this.child(
                    div()
                        .text_size(t.text.meta.size)
                        .text_color(t.colors.text_muted)
                        .child(reason),
                )
            },
        )
        .child(
            div()
                .v_flex()
                .w_full()
                .rounded(t.radius.large)
                .bg(t.colors.card)
                .border(t.layout.border)
                .border_color(t.colors.border)
                .p(t.space.0[5])
                .when_some(hold, |composer, hold| {
                    composer.child(
                        div()
                            .v_flex()
                            .gap(t.space.0[3])
                            .pb(t.space.0[4])
                            .child(
                                div()
                                    .text_size(t.text.label.size)
                                    .text_color(t.colors.text_muted)
                                    .child(WaitReason::Held(hold).to_string()),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap(t.space.0[3])
                                    .child(
                                        labelled_button("continue", "Continue", cx)
                                            .child(shortcut_chip::<Continue>(cx))
                                            .on_click(|_, window, cx| {
                                                window.dispatch_action(Box::new(Continue), cx)
                                            }),
                                    )
                                    .when(shell.discard_queued_visible(), |row| {
                                        row.child(
                                            labelled_button("discard-queued", "Discard queued", cx)
                                                .child(shortcut_chip::<DiscardQueued>(cx))
                                                .on_click(|_, window, cx| {
                                                    window.dispatch_action(
                                                        Box::new(DiscardQueued),
                                                        cx,
                                                    )
                                                }),
                                        )
                                    }),
                            ),
                    )
                })
                .when_some(request, |composer, request| {
                    let mut dock = div()
                        .v_flex()
                        .w_full()
                        .gap(t.space.0[3])
                        .p(t.space.0[4])
                        .mb(t.space.0[5])
                        .rounded(t.radius.control)
                        .border(t.layout.border)
                        .border_color(if shell.dock_focused {
                            t.colors.accent
                        } else {
                            t.colors.border
                        })
                        .track_focus(&shell.dock_focus)
                        .key_context("VeyraDock")
                        .when(pending.len() > 1, |dock| {
                            dock.child(
                                div()
                                    .text_size(t.text.meta.size)
                                    .text_color(t.colors.text_muted)
                                    .child(format!("1 of {}", pending.len())),
                            )
                        });
                    match &request.kind {
                        RequestKind::Approval { action, grant_key } => {
                            let (label, consequence) = approval_text(
                                action,
                                shell.approval_cwd(),
                                shell.chosen_provider().label(),
                            );
                            dock =
                                dock.key_context("VeyraDock VeyraApproval")
                                    .child(
                                        div()
                                            .font_family(t.text.mono_font.clone())
                                            .text_size(t.text.code.size)
                                            .line_height(t.text.code.line_height)
                                            .child(label),
                                    )
                                    .child(
                                        div()
                                            .text_size(t.text.label.size)
                                            .text_color(t.colors.text_muted)
                                            .child(consequence),
                                    )
                                    .child(
                                        div()
                                            .h_flex()
                                            .flex_wrap()
                                            .gap(t.space.0[3])
                                            .child(
                                                dock_chip(
                                                    labelled_button("deny", "Deny", cx),
                                                    shell.dock_focused,
                                                    shortcut_chip::<Deny>(cx),
                                                )
                                                .on_click(|_, window, cx| {
                                                    window.dispatch_action(Box::new(Deny), cx)
                                                }),
                                            )
                                            .when(grant_key.is_some(), |row| {
                                                row.child(
                                                    dock_chip(
                                                        labelled_button(
                                                            "allow-thread",
                                                            format!(
                                                                "Allow `{}` in this thread",
                                                                action.command
                                                            ),
                                                            cx,
                                                        ),
                                                        shell.dock_focused,
                                                        shortcut_chip::<AllowInThread>(cx),
                                                    )
                                                    .on_click(|_, window, cx| {
                                                        window.dispatch_action(
                                                            Box::new(AllowInThread),
                                                            cx,
                                                        )
                                                    }),
                                                )
                                            })
                                            .child(
                                                dock_chip(
                                                    labelled_button("allow-once", "Allow once", cx)
                                                        .primary(),
                                                    shell.dock_focused,
                                                    shortcut_chip::<AllowOnce>(cx),
                                                )
                                                .on_click(|_, window, cx| {
                                                    window.dispatch_action(Box::new(AllowOnce), cx)
                                                }),
                                            ),
                                    );
                        }
                        RequestKind::Question { questions } => {
                            let index = shell
                                .question_draft
                                .as_ref()
                                .filter(|(id, _)| *id == request.id)
                                .map_or(0, |(_, answers)| answers.len());
                            if let Some(question) = questions.get(index) {
                                dock = dock.child(question.text.clone()).children(
                                    question.options.iter().enumerate().map(|(option, label)| {
                                        labelled_button(
                                            SharedString::from(format!("answer-{option}")),
                                            format!("{}  {label}", option + 1),
                                            cx,
                                        )
                                        .justify_start()
                                        .child(div().flex_1())
                                        .on_click(
                                            move |_, window, cx| {
                                                window.dispatch_action(
                                                    Box::new(AnswerQuestion {
                                                        option: Some(option),
                                                    }),
                                                    cx,
                                                )
                                            },
                                        )
                                    }),
                                );
                            }
                        }
                    }
                    composer.child(dock.when(!shell.dock_focused, |dock| {
                        dock.child(
                            labelled_button("focus-dock", "Focus to use shortcuts", cx)
                                .child(shortcut_chip::<FocusDock>(cx))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(FocusDock), cx)
                                }),
                        )
                    }))
                })
                .child(
                    div().w_full().key_context("VeyraComposer").child(
                        Textarea::new(&shell.input)
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .min_h(t.layout.composer_min_height)
                            .max_h(t.layout.composer_max_height)
                            .text_size(t.text.read.size)
                            .line_height(t.text.read.line_height)
                            .aria_label("Message"),
                    ),
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
                            labelled_button("send", if stop { "Stop" } else { "Send" }, cx)
                                .primary()
                                .disabled(!stop && reason.is_some())
                                .when(stop, |button| button.child(shortcut_chip::<Stop>(cx)))
                                .on_click(move |_, window, cx| {
                                    if stop {
                                        window.dispatch_action(Box::new(Stop), cx);
                                    } else {
                                        window.dispatch_action(Box::new(Send), cx);
                                    }
                                }),
                        ),
                ),
        )
        .when_some(
            reason.filter(|_| shell.provider_picker.is_none()),
            |this, reason| {
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
            },
        )
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
