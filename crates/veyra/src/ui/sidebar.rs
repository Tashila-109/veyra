use super::{
    Shell,
    actions::*,
    composer::{button_text, labelled_button_with},
    icons,
    theme::Theme,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon, StyledExt,
    button::{Button, ButtonCustomVariant, ButtonVariants},
    empty::Empty,
    popover::Popover,
};
use gpui_kit::{prelude::*, *};
use std::collections::BTreeMap;
use veyra_engine::domain::{Status, ThreadId};
#[derive(Default)]
pub(super) struct SidebarScroll {
    handle: ScrollHandle,
    selected: std::cell::Cell<Option<ThreadId>>,
    row_focus: std::cell::RefCell<BTreeMap<ThreadId, FocusHandle>>,
}
#[cfg(debug_assertions)]
pub fn glyph(status: &Status) -> &'static str {
    icons::status_path(status)
}
pub(super) fn glyph_color(status: &Status, theme: &Theme) -> Hsla {
    match status {
        Status::Working { .. } => theme.colors.accent,
        Status::Failed => theme.colors.danger,
        Status::Done { .. } => theme.colors.success,
        Status::NeedsYou => theme.colors.warning,
        _ => theme.colors.text_muted,
    }
}
pub(super) fn row_status(state: &super::AppState, thread: &veyra_engine::domain::Thread) -> Status {
    if thread.root() == thread.id {
        state.family_status(thread.id)
    } else {
        state.status(thread.id)
    }
}
pub(super) fn ordered_threads(state: &super::AppState) -> Vec<ThreadId> {
    let mut order = vec![];
    for project in state.projects.values() {
        for root in state.threads.values().filter(|thread| {
            thread.project == project.id
                && !thread.archived
                && (thread.root() == thread.id
                    || state
                        .threads
                        .get(&thread.root())
                        .is_none_or(|root| root.archived))
        }) {
            order.push(root.id);
            order.extend(
                state
                    .threads
                    .values()
                    .filter(|thread| {
                        !thread.archived && thread.root() == root.id && thread.id != root.id
                    })
                    .map(|thread| thread.id),
            );
        }
    }
    order
}
pub fn next_needs_you(
    order: &[ThreadId],
    selected: Option<ThreadId>,
    statuses: &BTreeMap<ThreadId, Status>,
) -> Option<ThreadId> {
    let start = selected
        .and_then(|id| order.iter().position(|row| *row == id))
        .map_or(0, |index| index + 1);
    for target in [Status::NeedsYou, Status::Failed] {
        for offset in 0..order.len() {
            let id = order[(start + offset) % order.len()];
            if statuses.get(&id) == Some(&target) {
                return Some(id);
            }
        }
    }
    None
}
pub(super) fn nested(state: &super::AppState, thread: &veyra_engine::domain::Thread) -> bool {
    thread.root() != thread.id
        && state
            .threads
            .get(&thread.root())
            .is_some_and(|root| !root.archived)
}
pub(super) fn can_keep_children(state: &super::AppState, root: ThreadId) -> bool {
    state
        .threads
        .get(&root)
        .is_some_and(|thread| thread.root() == root)
        && state
            .threads
            .values()
            .any(|thread| thread.root() == root && thread.id != root && !thread.archived)
}

fn button_style(cx: &App) -> ButtonCustomVariant {
    let t = cx.global::<Theme>();
    ButtonCustomVariant::new(cx)
        .foreground(t.colors.icon)
        .hover(t.sidebar_plate(t.colors.hover))
        .active(t.sidebar_plate(t.colors.sel))
}

fn icon_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: Icon,
    hover_color: Hsla,
    cx: &App,
) -> Button {
    let t = cx.global::<Theme>();
    let id = id.into();
    let label = label.into();
    Button::new(id.clone())
        .group(id.clone())
        .custom(button_style(cx))
        .size(t.layout.icon_button)
        .p_0()
        .rounded(t.radius.control)
        .accessibility_label(label.clone())
        .tooltip(label)
        .text_color(t.colors.icon)
        .child(
            div()
                .text_color(t.colors.icon)
                .group_hover(id, |style| style.text_color(hover_color))
                .child(icon),
        )
}

pub fn render(shell: &Shell, window: &mut Window, cx: &Context<Shell>) -> impl IntoElement {
    let t = cx.global::<Theme>();
    shell.sidebar_scroll.row_focus.borrow_mut().retain(|id, _| {
        shell
            .state
            .threads
            .get(id)
            .is_some_and(|thread| !thread.archived)
    });
    let mut rows = Vec::new();
    let mut selected_row = None;
    for (index, project) in shell.state.projects.values().enumerate() {
        let id = project.id;
        let group = SharedString::from(format!("project-{id}"));
        rows.push(
            labelled_button_with(
                group.clone(),
                project.name.clone(),
                div()
                    .h_flex()
                    .min_w_0()
                    .gap(t.layout.control_gap)
                    .group_hover(group.clone(), |style| style.text_color(t.colors.text))
                    .child(Icon::new(IconName::Folder).size(t.layout.icon))
                    .child(button_text(project.name.clone().into(), cx).font_weight(
                        if shell.project == Some(project.id) {
                            t.text.title_weight
                        } else {
                            t.text.label.weight
                        },
                    )),
                cx,
            )
            .w_full()
            .justify_start()
            .child(div().flex_1())
            .h(t.layout.project_row)
            .flex_none()
            .pl(t.layout.row_inset)
            .pr(t.layout.row_inset)
            .text_color(t.colors.text_muted)
            .custom(button_style(cx))
            .group(group)
            .when(index > 0, |row| {
                row.mt(t.layout.group_gap - t.layout.row_gap)
            })
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(SelectProject { project: Some(id) }), cx)
            })
            .into_any_element(),
        );
        for thread in ordered_threads(&shell.state)
            .iter()
            .map(|id| &shell.state.threads[id])
            .filter(|thread| thread.project == project.id)
        {
            let id = thread.id;
            if shell.view.chat() == Some(id) {
                selected_row = Some(rows.len());
            }
            let quiet = shell.state.quiet_minutes(id);
            let row_focus = shell
                .sidebar_scroll
                .row_focus
                .borrow_mut()
                .entry(id)
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let row_focused = row_focus.contains_focused(window, cx);
            let status = row_status(&shell.state, thread);
            let tooltip = super::header::status_text_at(&status, shell.clock.elapsed_at);
            let menu = shell
                .thread_menu
                .as_ref()
                .filter(|(thread, _)| *thread == id)
                .map(|(_, menu)| menu.clone());
            let this = cx.entity().downgrade();
            let row_menu_open = menu.is_some();
            let row_menu = Popover::new(SharedString::from(format!("thread-menu-{id}")))
                .appearance(false)
                .open(menu.is_some())
                .when_some(menu.as_ref(), |popover, menu| {
                    popover.track_focus(&menu.focus_handle(cx))
                })
                .trigger(
                    icon_button(
                        SharedString::from(format!("thread-menu-trigger-{id}")),
                        "Thread actions",
                        Icon::new(IconName::Ellipsis).size(t.layout.small_icon),
                        t.colors.text,
                        cx,
                    )
                    .size(t.layout.small_icon_button)
                    .debug_selector(|| "sidebar-thread-menu".into())
                    .accessibility_label("Thread actions")
                    .tooltip("Thread actions")
                    .opacity(0.)
                    .when(row_focused || row_menu_open, |button| button.opacity(1.))
                    .group_hover(SharedString::from(format!("thread-row-{id}")), |style| {
                        style.opacity(1.)
                    })
                    .focus_visible(|style| style.opacity(1.)),
                )
                .on_open_change(move |open, window, cx| {
                    if *open {
                        window.dispatch_action(Box::new(OpenThreadMenu { thread: Some(id) }), cx);
                    } else {
                        let _ =
                            this.update(cx, |shell, cx| shell.set_thread_menu(None, window, cx));
                    }
                })
                .content(move |_, _, cx| {
                    super::composer::floating_panel(cx).children(menu.clone())
                });
            let content = div()
                .h_flex()
                .w_full()
                .h_full()
                .min_w_0()
                .items_center()
                .gap(t.space.0[3])
                .when(nested(&shell.state, thread), |content| {
                    content
                        .pl(t.layout.agent_margin)
                        .border_l(t.layout.guide)
                        .border_color(t.colors.border)
                })
                .child(icons::status_mark(
                    &status,
                    shell.clock.phase,
                    t.layout.status_glyph,
                    glyph_color(&status, t),
                ))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .min_w_0()
                        .debug_selector(|| "sidebar-thread-title".into())
                        .flex_1()
                        .pl(t.space.0[3])
                        .pr(t.space.0[3])
                        .when(
                            thread.root() != thread.id && !nested(&shell.state, thread),
                            |title| {
                                title.child(
                                    div()
                                        .id(SharedString::from(format!("lineage-{id}")))
                                        .debug_selector(|| "sidebar-lineage".into())
                                        .flex_none()
                                        .child(
                                            Icon::default()
                                                .path("marks/lineage.svg")
                                                .size(t.layout.small_icon)
                                                .text_color(t.colors.icon),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                "From archived parent",
                                            )
                                            .build(window, cx)
                                        }),
                                )
                            },
                        )
                        .child(
                            button_text(thread.title.clone().into(), cx)
                                .text_size(t.text.ui.size)
                                .line_height(t.text.ui.line_height)
                                .font_weight(if shell.view.chat() == Some(id) {
                                    t.text.title_weight
                                } else {
                                    t.text.ui.weight
                                })
                                .flex_1()
                                .min_w(t.layout.small_icon_button),
                        )
                        .children(quiet.map(|minutes| {
                            let compact = shell.sidebar_width < t.layout.sidebar_default;
                            div()
                                .debug_selector(|| "sidebar-quiet".into())
                                .flex_none()
                                .whitespace_nowrap()
                                .min_w(t.layout.status_glyph * 6.)
                                .text_size(t.text.meta.size)
                                .text_color(t.colors.text_muted)
                                .child(if compact {
                                    format!("Quiet {minutes}m")
                                } else {
                                    format!("Quiet for {minutes} min")
                                })
                        })),
                )
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .flex_none()
                        .w(t.layout.small_icon_button * 2.)
                        .child(
                            div()
                                .debug_selector(move || format!("sidebar-stop-slot-{id}"))
                                .w(t.layout.small_icon_button)
                                .flex_none()
                                .when(shell.row_can_stop(id), |slot| {
                                    slot.child(
                                        icon_button(
                                            SharedString::from(format!("stop-thread-{id}")),
                                            "Stop thread",
                                            Icon::new(IconName::Square).size(t.layout.small_icon),
                                            t.colors.danger,
                                            cx,
                                        )
                                        .debug_selector(|| "sidebar-thread-stop".into())
                                        .size(t.layout.small_icon_button)
                                        .accessibility_label("Stop thread")
                                        .tooltip("Stop thread")
                                        .on_click(
                                            move |_, window, cx| {
                                                window.dispatch_action(
                                                    Box::new(StopThread { thread: Some(id) }),
                                                    cx,
                                                );
                                            },
                                        ),
                                    )
                                }),
                        )
                        .child(row_menu),
                );
            let keyboard_focus = row_focus.clone();
            rows.push(
                div()
                    .id(SharedString::from(format!("thread-{id}")))
                    .key_context("VeyraSidebarRow")
                    .track_focus(&row_focus)
                    .tab_index(0)
                    .role(gpui::accesskit::Role::Button)
                    .aria_label(thread.title.clone())
                    .pl(t.layout.row_inset)
                    .pr(t.layout.row_inset)
                    .rounded(t.radius.control)
                    .debug_selector(|| "sidebar-thread-row".into())
                    .group(SharedString::from(format!("thread-row-{id}")))
                    .h(t.layout.thread_row)
                    .w_full()
                    .flex_none()
                    .min_w_0()
                    .relative()
                    .when(shell.view.chat() == Some(id), |row| {
                        row.bg(t.sidebar_plate(t.colors.sel))
                    })
                    .hover(|style| style.bg(t.sidebar_plate(t.colors.hover)))
                    .when(
                        row_focus.is_focused(window) && window.last_input_was_keyboard(),
                        |row| row.child(t.focus_ring()),
                    )
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                            .build(window, cx)
                    })
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&row_focus, cx)
                    })
                    .on_action(move |_: &ActivateThreadRow, window, cx| {
                        if keyboard_focus.is_focused(window) {
                            window.dispatch_action(Box::new(SelectThread { thread: Some(id) }), cx);
                        } else {
                            cx.propagate();
                        }
                    })
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(SelectThread { thread: Some(id) }), cx);
                    })
                    .child(content)
                    .into_any_element(),
            );
        }
    }
    if let Some(index) = selected_row {
        if shell.sidebar_scroll.selected.replace(shell.view.chat()) != shell.view.chat() {
            shell.sidebar_scroll.handle.scroll_to_item(index);
        }
    } else if shell.view.chat().is_none() {
        shell.sidebar_scroll.selected.set(None);
    }
    let needs_you = ordered_threads(&shell.state)
        .into_iter()
        .filter(|thread| shell.state.status(*thread) == Status::NeedsYou)
        .count();
    let rows_continue = shell.sidebar_scroll.handle.max_offset().y
        + shell.sidebar_scroll.handle.offset().y
        > Pixels::ZERO;
    div()
        .v_flex()
        .w(shell.sidebar_width)
        .h_full()
        .flex_none()
        .bg(t.sidebar_fill())
        .child(div().h(t.layout.header).flex_none())
        .child(
            div()
                .h_flex()
                .flex_none()
                .p(t.layout.row_inset)
                .gap(t.layout.control_gap)
                .when(
                    shell.sidebar_scroll.handle.offset().y < Pixels::ZERO,
                    |strip| {
                        strip
                            .border_b(t.layout.border)
                            .border_color(t.colors.border)
                    },
                )
                .when(needs_you > 0, |strip| {
                    strip.child(
                        labelled_button_with(
                            "needs-you",
                            "Next thread needing you",
                            div()
                                .h_flex()
                                .items_center()
                                .gap(t.layout.row_inset)
                                .child(icons::status_mark(
                                    &Status::NeedsYou,
                                    0,
                                    t.layout.small_icon,
                                    t.colors.warning,
                                ))
                                .child(
                                    button_text(
                                        format!(
                                            "{needs_you} {} you",
                                            if needs_you == 1 { "needs" } else { "need" }
                                        )
                                        .into(),
                                        cx,
                                    )
                                    .text_color(t.colors.text_muted),
                                ),
                            cx,
                        )
                        .pl(t.layout.row_inset)
                        .pr(t.layout.row_inset)
                        .custom(button_style(cx))
                        .tooltip_with_action(
                            "Next thread needing you",
                            &NextNeedsYou,
                            Some("Veyra"),
                        )
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(NextNeedsYou), cx)
                        }),
                    )
                })
                .child(div().flex_1())
                .child(
                    icon_button(
                        "add-project",
                        "Add project",
                        Icon::new(IconName::FolderPlus).size(t.layout.icon),
                        t.colors.text,
                        cx,
                    )
                    .tooltip_with_action("Add project", &AddProject::default(), Some("Veyra"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(AddProject::default()), cx)
                    }),
                )
                .child(
                    icon_button(
                        "new-thread",
                        "New thread",
                        Icon::new(IconName::SquarePen).size(t.layout.icon),
                        t.colors.text,
                        cx,
                    )
                    .tooltip_with_action("New thread", &NewThread, Some("Veyra"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(NewThread), cx)),
                ),
        )
        .child(
            div()
                .id("sidebar-rows")
                .debug_selector(|| "sidebar-rows".into())
                .track_scroll(&shell.sidebar_scroll.handle)
                .v_flex()
                .min_h_0()
                .flex_1()
                .overflow_y_scroll()
                .py(t.layout.row_inset)
                .pl(t.layout.row_inset)
                .pr(t.layout.row_inset)
                .gap(t.layout.row_gap)
                .when(rows.is_empty(), |this| {
                    this.child(
                        Empty::new()
                            .p(t.layout.row_inset)
                            .rounded(t.radius.control)
                            .text_size(t.text.meta.size)
                            .text_color(t.colors.text_muted)
                            .child("Your projects appear here."),
                    )
                })
                .children(rows),
        )
        .child(
            div()
                .flex_none()
                .p(t.layout.row_inset)
                .when(rows_continue, |foot| {
                    foot.border_t(t.layout.border).border_color(t.colors.border)
                })
                .child(
                    labelled_button_with(
                        "settings",
                        "Settings",
                        div()
                            .h_flex()
                            .w_full()
                            .justify_start()
                            .items_center()
                            .gap(t.layout.control_gap)
                            .group_hover("sidebar-settings", |style| {
                                style.text_color(t.colors.text)
                            })
                            .child(Icon::new(IconName::Settings).size(t.layout.icon))
                            .child(button_text("Settings".into(), cx).text_size(t.text.ui.size)),
                        cx,
                    )
                    .w_full()
                    .h(t.layout.thread_row)
                    .pl(t.layout.row_inset)
                    .pr(t.layout.row_inset)
                    .justify_start()
                    .when(
                        matches!(shell.view, super::MainView::Settings { .. }),
                        |button| button.bg(t.sidebar_plate(t.colors.sel)),
                    )
                    .custom(button_style(cx))
                    .group("sidebar-settings")
                    .tooltip_with_action("Settings", &OpenSettings, Some("Veyra"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::next_needs_you;
    use std::collections::BTreeMap;
    use veyra_engine::domain::{Status, ThreadId};
    #[test]
    fn next_needs_you_order() {
        let root = ThreadId::new();
        let child = ThreadId::new();
        let failed = ThreadId::new();
        let other = ThreadId::new();
        let order = [root, child, failed, other];
        let mut statuses = BTreeMap::from([
            (root, Status::Done { took: 1 }),
            (child, Status::NeedsYou),
            (failed, Status::Failed),
            (other, Status::NeedsYou),
        ]);
        assert_eq!(next_needs_you(&order, Some(root), &statuses), Some(child));
        assert_eq!(next_needs_you(&order, Some(child), &statuses), Some(other));
        assert_eq!(next_needs_you(&order, Some(other), &statuses), Some(child));
        statuses.insert(child, Status::Done { took: 1 });
        statuses.insert(other, Status::Idle);
        assert_eq!(next_needs_you(&order, Some(other), &statuses), Some(failed));
        statuses.insert(failed, Status::Idle);
        assert_eq!(next_needs_you(&order, Some(root), &statuses), None);
        assert_eq!(next_needs_you(&[], None, &statuses), None);
    }
}
