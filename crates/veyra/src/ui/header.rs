use super::{
    MainView, Shell,
    actions::{SelectThread, ToggleShellDrawer},
    composer::labelled_button,
    icons,
    theme::Theme,
};
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
use std::collections::BTreeMap;
use veyra_engine::{
    domain::{Millis, Status, ThreadId},
    events::{ChangeEvent, UiEvent},
};
pub(super) fn starting_text(
    provider: veyra_engine::provider::ProviderKind,
    wait: Option<veyra_engine::domain::WaitReason>,
) -> Option<String> {
    (wait == Some(veyra_engine::domain::WaitReason::Starting))
        .then(|| format!("Starting {}", provider.label()))
}
pub(super) fn status_text_at(status: &Status, now: Millis) -> String {
    match status {
        Status::Working { since } => format!(
            "Working · {}s",
            if *since > 0 {
                (now - since).max(0) / 1000
            } else {
                0
            }
        ),
        Status::NeedsYou => "Needs you".into(),
        Status::Done { took } => format!("Done · {}s", took / 1000),
        _ => status.words().into(),
    }
}
pub(super) fn note_activity(event: &UiEvent, now: Millis) -> Option<(ThreadId, Millis)> {
    match event {
        UiEvent::TextAppended { thread, .. } => Some((*thread, now)),
        UiEvent::Change { event, .. } => match event {
            ChangeEvent::TurnStarted(turn) => {
                Some((turn.thread, turn.state.started().unwrap_or(now)))
            }
            ChangeEvent::ItemAdded(item) | ChangeEvent::ItemCompleted(item) => {
                Some((item.thread, now))
            }
            ChangeEvent::RequestOpened(request) => Some((request.thread, now)),
            _ => None,
        },
        _ => None,
    }
}
pub(super) fn quiet_for(
    status: &Status,
    activity: Option<Millis>,
    now: Millis,
    open_tool: bool,
    docked: bool,
) -> Option<Millis> {
    let Status::Working { since } = status else {
        return None;
    };
    let minutes = now.saturating_sub(activity.unwrap_or(*since)) / 60_000;
    (minutes >= 10 && !open_tool && !docked).then_some(minutes)
}
pub(super) fn cadence_due(statuses: &BTreeMap<ThreadId, Status>) -> bool {
    statuses
        .values()
        .any(|status| matches!(status, Status::Working { .. }))
}
pub(super) fn parent_attention(state: &super::AppState, thread: ThreadId) -> Option<String> {
    state
        .parent(thread)
        .filter(|parent| state.status(parent.id) == Status::NeedsYou)
        .map(|parent| format!("{} needs you", parent.provider.label()))
}
pub fn render(shell: &Shell, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let title = match shell.view {
        MainView::Settings { .. } => "Settings".to_owned(),
        MainView::NewThread => "New thread".to_owned(),
        MainView::Chat(thread) => shell
            .state
            .threads
            .get(&thread)
            .map(|thread| thread.title.clone())
            .unwrap_or_default(),
    };
    let text = shell.view.chat().map(|thread| shell.header_status(thread));
    let parent = shell
        .view
        .chat()
        .and_then(|thread| shell.state.parent(thread));
    let attention = shell
        .view
        .chat()
        .and_then(|thread| parent_attention(&shell.state, thread));
    let project = shell
        .view
        .chat()
        .and_then(|id| shell.state.threads.get(&id))
        .and_then(|thread| shell.state.projects.get(&thread.project))
        .or_else(|| shell.project.and_then(|id| shell.state.projects.get(&id)))
        .map(|project| {
            if shell.branch.is_empty() {
                project.name.clone()
            } else {
                format!("{} · {}", project.name, shell.branch)
            }
        });
    div()
        .h(t.layout.header)
        .flex_none()
        .h_flex()
        .items_center()
        .px(t.space.0[6])
        .gap(t.space.0[4])
        .border_b(t.layout.border)
        .border_color(t.colors.border)
        .when_some(parent, |header, parent| {
            header.child(
                div()
                    .h_flex()
                    .min_w_0()
                    .gap(t.space.0[3])
                    .child(if parent.archived {
                        div().child(parent.title.clone()).into_any_element()
                    } else {
                        let id = parent.id;
                        labelled_button("parent-breadcrumb", parent.title.clone(), cx)
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(SelectThread { thread: Some(id) }),
                                    cx,
                                )
                            })
                            .into_any_element()
                    })
                    .child("/"),
            )
        })
        .child(
            div()
                .debug_selector(|| "header-title".into())
                .min_w_0()
                .truncate()
                .text_size(t.text.ui.size)
                .line_height(t.text.ui.line_height)
                .font_weight(t.text.title_weight)
                .child(title),
        )
        .when_some(attention, |header, attention| {
            header.child(
                div()
                    .text_size(t.text.meta.size)
                    .text_color(t.colors.accent)
                    .child(attention),
            )
        })
        .when_some(text, |this, text| {
            this.child(
                div()
                    .debug_selector(|| "header-status".into())
                    .h_flex()
                    .items_center()
                    .gap(t.layout.control_gap)
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(t.text.meta.size)
                    .text_color(t.colors.text_muted)
                    .when_some(shell.view.chat(), |row, thread| {
                        let status = shell.state.status(thread);
                        row.child(icons::status_mark(
                            &status,
                            shell.clock.phase,
                            t.layout.status_glyph,
                            super::sidebar::glyph_color(&status, t),
                        ))
                    })
                    .child(text),
            )
        })
        .child(div().flex_1())
        .when_some(project, |this, project| {
            this.child(
                div()
                    .text_size(t.text.meta.size)
                    .text_color(t.colors.text_muted)
                    .child(project),
            )
        })
        .when(shell.view.chat().is_some(), |header| {
            header.child(
                labelled_button("toggle-shell-drawer", "⌘J", cx).on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(ToggleShellDrawer), cx)
                }),
            )
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn needs_you_header_uses_readable_words() {
        assert_eq!(
            super::status_text_at(&veyra_engine::domain::Status::NeedsYou, 0),
            "Needs you"
        );
    }
}

#[cfg(test)]
mod quiet_tests {
    use super::{cadence_due, note_activity, quiet_for};
    use std::collections::BTreeMap;
    use veyra_engine::{
        domain::{Status, ThreadId},
        events::UiEvent,
    };
    #[test]
    fn quiet_for_after_ten_minutes() {
        let working = Status::Working { since: 0 };
        assert_eq!(quiet_for(&working, Some(0), 540_000, false, false), None);
        assert_eq!(
            quiet_for(&working, Some(0), 600_000, false, false),
            Some(10)
        );
        assert_eq!(
            quiet_for(&working, Some(0), 720_000, false, false),
            Some(12)
        );
        assert_eq!(quiet_for(&working, Some(0), 600_000, true, false), None);
        assert_eq!(quiet_for(&working, Some(0), 600_000, false, true), None);
        assert_eq!(
            quiet_for(&Status::Idle, Some(0), 600_000, false, false),
            None
        );
    }
    #[test]
    fn quiet_clock_counts_streamed_text() {
        let thread = ThreadId::new();
        let working = Status::Working { since: 0 };
        let mut clock = None;
        for minute in 1..=12 {
            let now = minute * 60_000;
            let event = UiEvent::TextAppended {
                thread,
                item: veyra_engine::domain::ItemId::new(),
                text: "still streaming".into(),
                counter: minute as u64,
                #[cfg(debug_assertions)]
                delta_count: 1,
            };
            assert_eq!(note_activity(&event, now).unwrap().0, thread);
            clock = Some(note_activity(&event, now).unwrap().1);
            assert_eq!(quiet_for(&working, clock, now, false, false), None);
        }
        assert_eq!(quiet_for(&working, clock, 21 * 60_000, false, false), None);
        assert_eq!(
            quiet_for(&working, clock, 22 * 60_000, false, false),
            Some(10)
        );
    }
    #[test]
    fn cadence_due_for_unselected_thread() {
        let mut statuses = BTreeMap::from([
            (ThreadId::new(), Status::Idle),
            (ThreadId::new(), Status::Working { since: 0 }),
        ]);
        assert!(cadence_due(&statuses));
        for status in statuses.values_mut() {
            *status = Status::Done { took: 1 };
        }
        assert!(!cadence_due(&statuses));
    }
}

#[cfg(test)]
mod starting_tests {
    #[test]
    fn starting_header_names_each_threads_provider() {
        use veyra_engine::{domain::WaitReason, provider::ProviderKind};
        for provider in ProviderKind::ALL {
            assert_eq!(
                super::starting_text(provider, Some(WaitReason::Starting)),
                Some(format!("Starting {}", provider.label()))
            );
            assert_eq!(
                super::starting_text(provider, Some(WaitReason::TurnRunning)),
                None
            );
        }
    }
}
