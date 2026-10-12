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
use veyra_engine::domain::{
    ApprovalTarget, FileOperation, ProposedAction, RequestKind, TargetLocation, WaitReason,
};

pub(super) fn approval_text(action: &ProposedAction, provider: &str) -> (String, String, String) {
    let (label, consequence, preview) = match &action.target {
        ApprovalTarget::Command { command, preview } => (
            command.clone(),
            format!("{provider} will run this command in the project folder."),
            preview,
        ),
        ApprovalTarget::File {
            operation,
            path,
            location,
            preview,
        } => {
            let operation = match operation {
                FileOperation::Write => "writes a file",
                FileOperation::Edit => "edits a file",
                FileOperation::Read => "reads a file",
            };
            let location = match location {
                TargetLocation::InsideProject => "inside the project folder",
                TargetLocation::OutsideProject => "outside the project folder",
                TargetLocation::NotVerified => "at a location that is not verified",
            };
            (
                format!("{} {}", action.tool, path.display()),
                format!("{provider} {operation} {location}."),
                preview,
            )
        }
        ApprovalTarget::Tool { preview } => (
            action.tool.clone(),
            format!("{provider} uses this tool with the arguments below."),
            preview,
        ),
    };
    let mut preview_text = preview.text.clone();
    if preview.truncated {
        let cap = veyra_engine::domain::TOOL_OUTPUT_CHARS;
        let limit = format!("{},{:03}", cap / 1000, cap % 1000);
        preview_text.push_str(&format!("\nPreview cut after {limit} characters."));
    }
    (label, consequence, preview_text)
}

fn dock_chip(button: Button, focused: bool, chip: impl IntoElement) -> Button {
    button.when(focused, |button| button.child(chip))
}

fn button_caption(label: SharedString) -> SharedString {
    let Some((line, _)) = label.split_once('\n') else {
        return label;
    };
    format!("{line}…").into()
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
                .truncate()
                .child(button_caption(label)),
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
    let stop_target = shell.shown_stop(cx);
    let stop = stop_target.is_some();
    let submit_id = stop_target
        .map(|target| SharedString::from(format!("stop-{}-{}", target.thread, target.turn)))
        .unwrap_or_else(|| "send".into());
    let pending = thread
        .map(|thread| shell.state.pending_requests(thread))
        .unwrap_or_default();
    let target = shell.dock_target();
    let request = target.and_then(|target| {
        shell
            .pending_request(Some(target.request))
            .map(|request| (target, request))
    });
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
                .when_some(request, |composer, (target, request)| {
                    let request_id = request.id;
                    let question_index = target.question;
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
                            let (label, consequence, preview) =
                                approval_text(action, shell.chosen_provider().label());
                            dock = dock
                                .key_context("VeyraDock VeyraApproval")
                                .child(
                                    div()
                                        .id("approval-details")
                                        .w_full()
                                        .min_w_0()
                                        .debug_selector(|| "approval-details".into())
                                        .v_flex()
                                        .min_h_0()
                                        .max_h(t.layout.composer_max_height)
                                        .overflow_scroll()
                                        .gap(t.space.0[3])
                                        .child(
                                            div()
                                                .font_family(t.text.mono_font.clone())
                                                .text_size(t.text.code.size)
                                                .line_height(t.text.code.line_height)
                                                .debug_selector(|| "approval-action-text".into())
                                                .child(label),
                                        )
                                        .child(
                                            div()
                                                .text_size(t.text.label.size)
                                                .text_color(t.colors.text_muted)
                                                .child(consequence),
                                        )
                                        .when(!preview.is_empty(), |details| {
                                            details.child(
                                                div()
                                                    .debug_selector(|| "approval-preview".into())
                                                    .font_family(t.text.mono_font.clone())
                                                    .text_size(t.text.code.size)
                                                    .line_height(t.text.code.line_height)
                                                    .child(preview),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .h_flex()
                                        .flex_wrap()
                                        .gap(t.space.0[3])
                                        .child(
                                            dock_chip(
                                                labelled_button(
                                                    SharedString::from(format!(
                                                        "deny-{request_id}"
                                                    )),
                                                    "Deny",
                                                    cx,
                                                )
                                                .debug_selector(|| "approval-deny-button".into()),
                                                shell.dock_focused,
                                                shortcut_chip::<Deny>(cx),
                                            )
                                            .on_click(
                                                cx.listener(move |_, _, window, cx| {
                                                    window.dispatch_action(
                                                        Box::new(Deny {
                                                            request: Some(request_id),
                                                        }),
                                                        cx,
                                                    )
                                                }),
                                            ),
                                        )
                                        .when_some(grant_key.as_ref(), |row, key| {
                                            row.child(
                                                dock_chip(
                                                    labelled_button(
                                                        SharedString::from(format!(
                                                            "allow-thread-{request_id}"
                                                        )),
                                                        format!(
                                                            "Allow `{}` in this thread",
                                                            key.command
                                                        ),
                                                        cx,
                                                    )
                                                    .max_w_full(),
                                                    shell.dock_focused,
                                                    shortcut_chip::<AllowInThread>(cx),
                                                )
                                                .on_click(cx.listener(move |_, _, window, cx| {
                                                    window.dispatch_action(
                                                        Box::new(AllowInThread {
                                                            request: Some(request_id),
                                                        }),
                                                        cx,
                                                    )
                                                })),
                                            )
                                        })
                                        .child(
                                            dock_chip(
                                                labelled_button(
                                                    SharedString::from(format!(
                                                        "allow-once-{request_id}"
                                                    )),
                                                    "Allow once",
                                                    cx,
                                                )
                                                .primary()
                                                .debug_selector(|| "approval-allow-button".into()),
                                                shell.dock_focused,
                                                shortcut_chip::<AllowOnce>(cx),
                                            )
                                            .on_click(
                                                cx.listener(move |_, _, window, cx| {
                                                    window.dispatch_action(
                                                        Box::new(AllowOnce {
                                                            request: Some(request_id),
                                                        }),
                                                        cx,
                                                    )
                                                }),
                                            ),
                                        ),
                                );
                        }
                        RequestKind::Question { questions } => {
                            let index = question_index;
                            if let Some(question) = questions.get(index) {
                                dock = dock.child(question.text.clone()).children(
                                    question.options.iter().enumerate().map(|(option, label)| {
                                        labelled_button(
                                            SharedString::from(format!(
                                                "answer-{request_id}-{index}-{option}"
                                            )),
                                            format!("{}  {label}", option + 1),
                                            cx,
                                        )
                                        .justify_start()
                                        .child(div().flex_1())
                                        .on_click(
                                            cx.listener(move |_, _, window, cx| {
                                                window.dispatch_action(
                                                    Box::new(AnswerQuestion {
                                                        request: Some(request_id),
                                                        question: Some(index),
                                                        option: Some(option),
                                                    }),
                                                    cx,
                                                )
                                            }),
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
                            labelled_button(submit_id, if stop { "Stop" } else { "Send" }, cx)
                                .debug_selector(|| "composer-submit-button".into())
                                .primary()
                                .disabled(!stop && reason.is_some())
                                .when(stop, |button| button.child(shortcut_chip::<Stop>(cx)))
                                .on_click(move |_, window, cx| {
                                    if stop {
                                        window.dispatch_action(
                                            Box::new(Stop {
                                                target: stop_target,
                                            }),
                                            cx,
                                        );
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

#[cfg(test)]
mod tests {
    use super::approval_text;
    use veyra_engine::domain::{ApprovalPreview, ApprovalTarget, ProposedAction};
    #[gpui::test]
    fn grant_caption_keeps_command_whitespace(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        for (label, expected) in [
            (
                "Allow `echo a  b` in this thread",
                "Allow `echo a  b` in this thread",
            ),
            (
                "Allow `echo a  b\nnext line` in this thread",
                "Allow `echo a  b…",
            ),
        ] {
            assert_eq!(super::button_caption(label.into()).as_ref(), expected);
        }
        struct Caption(String);
        impl Render for Caption {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().flex().child(
                    super::labelled_button("grant", self.0.clone(), cx)
                        .debug_selector(|| "grant-caption".into()),
                )
            }
        }
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Caption("Allow `echo a b` in this thread".into())
        });
        let mut widths = Vec::new();
        for caption in [
            "Allow `echo a b` in this thread",
            "Allow `echo a  b` in this thread",
        ] {
            view.update(cx, |view, cx| {
                view.0 = caption.into();
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            widths.push(cx.debug_bounds("grant-caption").unwrap().size.width);
        }
        assert!(
            widths[1] > widths[0],
            "grant captions collapsed distinct command whitespace"
        );
    }
    #[test]
    fn tool_approval_preview_shows_arguments_and_cut_notice() {
        let action = ProposedAction {
            tool: "remote tool".into(),
            target: ApprovalTarget::Tool {
                preview: ApprovalPreview::new("x".repeat(10001)),
            },
        };
        let (label, consequence, preview) = approval_text(&action, "Provider");
        assert_eq!(label, "remote tool");
        assert_eq!(
            consequence,
            "Provider uses this tool with the arguments below."
        );
        assert_eq!(
            preview,
            "x".repeat(10000) + "\nPreview cut after 10,000 characters."
        );
    }
    #[test]
    fn dock_renders_only_typed_provider_neutral_actions() {
        let code = include_str!("composer.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for wire_name in [
            "\"file_path\"",
            "\"path\"",
            "\"notebook_path\"",
            "\"Write\"",
            "\"Edit\"",
            "\"MultiEdit\"",
            "\"Read\"",
            "\"Bash\"",
            "\"WebFetch\"",
            "\"NotebookEdit\"",
            "canonicalize",
            "std::fs",
        ] {
            assert!(
                !code.contains(wire_name),
                "dock must not interpret provider data or touch disk: {wire_name}"
            );
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut folders = vec![root];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let code = source.split("#[cfg(test)]").next().unwrap();
                for tool in [
                    "Bash",
                    "Write",
                    "Edit",
                    "MultiEdit",
                    "Read",
                    "NotebookEdit",
                    "WebFetch",
                ] {
                    assert!(
                        !code.contains(&format!("\"{tool}\"")),
                        "{} interprets provider tool {tool}",
                        path.display()
                    );
                }
                for field in [
                    "command",
                    "file_path",
                    "path",
                    "notebook_path",
                    "old_string",
                    "new_string",
                    "new_source",
                ] {
                    for access in [format!(".get(\"{field}\")"), format!("[\"{field}\"]")] {
                        assert!(
                            !code.contains(&access),
                            "{} interprets provider field {field}",
                            path.display()
                        );
                    }
                }
            }
        }
        assert!(code.contains(".child(preview)"));
    }
}
