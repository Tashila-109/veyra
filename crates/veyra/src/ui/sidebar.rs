use super::{Shell, actions::*, composer::labelled_button, theme::Theme};
use gpui_kit::base::Selectable;
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
use std::collections::BTreeMap;
use veyra_engine::domain::{Status, ThreadId};
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
pub fn render(shell: &Shell, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let mut rows = Vec::new();
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
            let status = if thread.root() == thread.id {
                shell.state.family_status(thread.id)
            } else {
                shell.state.status(thread.id)
            };
            rows.push(
                div()
                    .h_flex()
                    .items_center()
                    .h(t.layout.thread_row)
                    .gap(t.space.0[3])
                    .when(thread.root() != thread.id, |row| {
                        row.pl(t.layout.agent_margin)
                    })
                    .child(
                        div()
                            .w(t.layout.status_glyph)
                            .text_color(match status {
                                Status::Working { .. } => t.colors.accent,
                                Status::Failed => t.colors.danger,
                                Status::Done { .. } => t.colors.success,
                                _ => t.colors.text_muted,
                            })
                            .child(glyph(&status)),
                    )
                    .child(
                        labelled_button(
                            SharedString::from(format!("thread-{id}")),
                            thread.title.clone(),
                            cx,
                        )
                        .flex_1()
                        .min_w_0()
                        .justify_start()
                        .child(div().flex_1())
                        .selected(shell.view.chat() == Some(thread.id))
                        .tooltip(status.words())
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(Box::new(SelectThread { thread: Some(id) }), cx)
                        }),
                    )
                    .child(
                        labelled_button(SharedString::from(format!("stop-thread-{id}")), "■", cx)
                            .w(t.layout.icon_button)
                            .accessibility_label("Stop thread")
                            .tooltip("Stop thread")
                            .on_click(move |_, window, cx| {
                                window
                                    .dispatch_action(Box::new(StopThread { thread: Some(id) }), cx)
                            }),
                    )
                    .into_any_element(),
            );
        }
    }
    div()
        .v_flex()
        .w(shell.sidebar_width)
        .h_full()
        .flex_none()
        .bg(t.colors.shell)
        .child(div().h(t.layout.header).flex_none())
        .child(
            div()
                .id("sidebar-rows")
                .v_flex()
                .flex_1()
                .overflow_y_scroll()
                .p(t.space.0[4])
                .gap(t.space.0[3])
                .child(
                    labelled_button("add-project", "+  Add project", cx)
                        .w_full()
                        .justify_start()
                        .child(div().flex_1())
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(AddProject::default()), cx)
                        }),
                )
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
