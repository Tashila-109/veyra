use super::{
    Shell,
    actions::*,
    composer::{button_text, labelled_button},
    theme::Theme,
};
use gpui_kit::base::Selectable;
use gpui_kit::component::{StyledExt, popover::Popover};
use gpui_kit::{prelude::*, *};
use std::collections::BTreeMap;
use veyra_engine::domain::{Status, ThreadId};
#[derive(Default)]
pub(super) struct SidebarScroll {
    handle: ScrollHandle,
    selected: std::cell::Cell<Option<ThreadId>>,
    row_focus: std::cell::RefCell<BTreeMap<ThreadId, FocusHandle>>,
}
pub fn glyph(status: &Status) -> &'static str {
    match status {
        Status::Working { .. } => "◔",
        Status::Failed => "×",
        Status::Done { .. } => "✓",
        Status::NeedsYou | Status::Held => "◇",
        Status::Interrupted => "×",
        _ => "·",
    }
}
pub(super) fn glyph_color(status: &Status, theme: &Theme) -> Hsla {
    match status {
        Status::Working { .. } => theme.colors.accent,
        Status::Failed => theme.colors.danger,
        Status::Done { .. } => theme.colors.success,
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
    for project in shell.state.projects.values() {
        let id = project.id;
        rows.push(
            labelled_button(
                SharedString::from(format!("project-{id}")),
                project.name.clone(),
                cx,
            )
            .w_full()
            .justify_start()
            .child(div().flex_1())
            .h(t.layout.project_row)
            .flex_none()
            .selected(shell.project == Some(project.id))
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
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let row_focused = row_focus.contains_focused(window, cx);
            let status = row_status(&shell.state, thread);
            let tooltip = super::header::status_text(&status);
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
                    labelled_button(
                        SharedString::from(format!("thread-menu-trigger-{id}")),
                        "…",
                        cx,
                    )
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
            let content =
                div()
                    .h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(t.space.0[3])
                    .child(
                        div()
                            .w(t.layout.status_glyph)
                            .flex_none()
                            .text_color(glyph_color(&status, t))
                            .child(glyph(&status)),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .min_w_0()
                            .debug_selector(|| "sidebar-thread-title".into())
                            .flex_1()
                            .pl(t.space.0[3])
                            .pr(t.space.0[3])
                            .child(
                                button_text(thread.title.clone().into(), cx)
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
                            .w(t.layout.icon_button * 2.)
                            .child(
                                div()
                                    .debug_selector(move || format!("sidebar-stop-slot-{id}"))
                                    .w(t.layout.icon_button)
                                    .flex_none()
                                    .when(
                                        shell.thread_stop_target(id).is_some()
                                            || shell
                                                .engine
                                                .window_threads()
                                                .get(&id)
                                                .is_some_and(|work| work.busy),
                                        |slot| {
                                            slot.child(
                                                labelled_button(
                                                    SharedString::from(format!("stop-thread-{id}")),
                                                    "■",
                                                    cx,
                                                )
                                                .debug_selector(|| "sidebar-thread-stop".into())
                                                .w(t.layout.icon_button)
                                                .accessibility_label("Stop thread")
                                                .tooltip("Stop thread")
                                                .on_click(move |_, window, cx| {
                                                    window.dispatch_action(
                                                        Box::new(StopThread { thread: Some(id) }),
                                                        cx,
                                                    );
                                                }),
                                            )
                                        },
                                    ),
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
                    .pl(t.space.0[3])
                    .pr(t.space.0[3])
                    .rounded(t.radius.control)
                    .debug_selector(|| "sidebar-thread-row".into())
                    .group(SharedString::from(format!("thread-row-{id}")))
                    .h(t.layout.thread_row)
                    .w_full()
                    .flex_none()
                    .min_w_0()
                    .when(shell.view.chat() == Some(id), |row| row.bg(t.colors.sel))
                    .hover(|style| style.bg(t.colors.hover))
                    .focus_visible(|style| style.bg(t.colors.sel))
                    .when(nested(&shell.state, thread), |row| {
                        row.ml(t.layout.agent_margin)
                            .pl(t.space.0[3])
                            .border_l(t.layout.guide)
                            .border_color(t.colors.border)
                    })
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
                .p(t.space.0[4])
                .gap(t.space.0[1])
                .when(
                    shell.sidebar_scroll.handle.offset().y < Pixels::ZERO,
                    |strip| {
                        strip
                            .border_b(t.layout.border)
                            .border_color(t.colors.border)
                    },
                )
                .child(
                    labelled_button("add-project", "+ Project", cx)
                        .flex_1()
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(AddProject::default()), cx)
                        }),
                )
                .child(
                    labelled_button("new-thread", "+ Thread", cx)
                        .flex_1()
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
                .py(t.space.0[4])
                .pl(t.space.0[0])
                .pr(t.space.0[0])
                .gap(t.space.0[3])
                .when(rows.is_empty(), |this| {
                    this.child(
                        div()
                            .text_size(t.text.meta.size)
                            .text_color(t.colors.text_faint)
                            .child("Your projects appear here."),
                    )
                })
                .children(rows),
        )
        .child(
            labelled_button("settings", "Settings  ⌘,", cx)
                .m(t.space.0[4])
                .flex_none()
                .justify_start()
                .selected(matches!(shell.view, super::MainView::Settings { .. }))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
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
