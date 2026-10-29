use super::{
    MainView, Shell,
    actions::{OpenThreadMenu, SelectThread, ToggleShellDrawer},
    composer::labelled_button,
    icons,
    theme::Theme,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, StyledExt};
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
fn rule_visible(view: MainView, offset: Pixels) -> bool {
    matches!(view, MainView::Chat(_)) && f32::from(offset) < 0.
}
pub(super) fn title_text(shell: &Shell) -> String {
    match shell.view {
        MainView::Settings { .. } => "Settings".to_owned(),
        MainView::NewThread => "New thread".to_owned(),
        MainView::Chat(thread) => shell
            .state
            .threads
            .get(&thread)
            .map(|thread| thread.title.clone())
            .unwrap_or_default(),
    }
}
fn observed(element: gpui::Div, _name: &'static str) -> AnyElement {
    #[cfg(debug_assertions)]
    let element = super::composer::observe_geometry(element.relative(), move |bounds, cx| {
        if cx.try_global::<super::PaintedControls>().is_some() {
            cx.global_mut::<super::PaintedControls>()
                .0
                .insert(format!("{:?}", ElementId::Name(_name.into())), bounds);
        }
    });
    element.into_any_element()
}
fn status_words(text: String, color: Hsla) -> StyledText {
    let length = text.len();
    StyledText::new(text).with_highlights([(
        0..length,
        HighlightStyle {
            color: Some(color),
            ..Default::default()
        },
    )])
}
pub fn render(shell: &Shell, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let rule = rule_visible(shell.view, shell.scroller.read(cx).scroll_offset());
    let title = title_text(shell);
    let text = shell.view.chat().map(|thread| shell.header_status(thread));
    let status_color = shell.view.chat().map_or(t.colors.text_muted, |thread| {
        super::sidebar::glyph_color(&shell.state.status(thread), t)
    });
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
        .when(rule, |header| header.border_b(t.layout.border))
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
                        super::composer::labelled_text_button_with(
                            "parent-breadcrumb",
                            parent.title.clone(),
                            div().child(parent.title.clone()),
                            cx,
                        )
                        .p_0()
                        .underline()
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(Box::new(SelectThread { thread: Some(id) }), cx)
                        })
                        .into_any_element()
                    })
                    .child(div().text_color(t.colors.text_muted).child("/")),
            )
        })
        .child(observed(
            div()
                .debug_selector(|| "header-title".into())
                .min_w_0()
                .truncate()
                .text_size(t.text.ui.size)
                .line_height(t.text.ui.line_height)
                .font_weight(t.text.title_weight)
                .child(if let Some(thread) = shell.view.chat() {
                    super::composer::labelled_text_button_with(
                        "header-thread-title",
                        title.clone(),
                        title,
                        cx,
                    )
                    .p_0()
                    .text_size(t.text.ui.size)
                    .font_weight(t.text.title_weight)
                    .tooltip_with_action(
                        "Thread actions",
                        &OpenThreadMenu::default(),
                        Some("Veyra"),
                    )
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(
                            Box::new(OpenThreadMenu {
                                thread: Some(thread),
                            }),
                            cx,
                        )
                    })
                    .into_any_element()
                } else {
                    div().child(title).into_any_element()
                }),
            "header-title",
        ))
        .when_some(attention, |header, attention| {
            header.child(
                labelled_button("parent-needs-you", attention, cx)
                    .text_size(t.text.meta.size)
                    .text_color(t.colors.warning)
                    .on_click({
                        let id = parent.map(|parent| parent.id);
                        move |_, window, cx| {
                            window.dispatch_action(Box::new(SelectThread { thread: id }), cx)
                        }
                    }),
            )
        })
        .when_some(text, |this, text| {
            this.child(observed(
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
                    .children(text.split_once(" · ").map_or_else(
                        || vec![status_words(text.clone(), status_color).into_any_element()],
                        |(words, elapsed)| {
                            vec![
                                status_words(words.to_owned(), status_color).into_any_element(),
                                format!("· {elapsed}").into_any_element(),
                            ]
                        },
                    )),
                "header-status",
            ))
        })
        .child(div().flex_1())
        .when_some(project, |this, project| {
            this.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(t.layout.control_gap)
                    .pl(t.layout.row_inset)
                    .pr(t.layout.row_inset)
                    .rounded(t.radius.inline)
                    .bg(t.colors.card)
                    .text_size(t.text.meta.size)
                    .text_color(t.colors.text_muted)
                    .child(Icon::new(IconName::Folder).size(t.layout.small_icon))
                    .when(!shell.branch.is_empty(), |chip| {
                        chip.child(Icon::new(IconName::GitBranch).size(t.layout.small_icon))
                    })
                    .child(project),
            )
        })
        .when(shell.view.chat().is_some(), |header| {
            header.child(
                super::composer::labelled_button_with(
                    "toggle-shell-drawer",
                    "Shell drawer",
                    Icon::new(IconName::Terminal)
                        .size(t.layout.icon)
                        .text_color(t.colors.icon),
                    cx,
                )
                .size(t.layout.icon_button)
                .p_0()
                .when(
                    shell
                        .view
                        .chat()
                        .and_then(|thread| shell.drawers.get(&thread))
                        .is_some_and(|drawer| drawer.open),
                    |button| button.bg(t.colors.card),
                )
                .tooltip_with_action("Shell drawer", &ToggleShellDrawer, Some("Veyra"))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleShellDrawer), cx)),
            )
        })
}

#[cfg(test)]
mod tests {
    #[gpui::test]
    fn status_text_without_wrapper_keeps_glyph_layout(cx: &mut gpui::TestAppContext) {
        use super::*;
        struct Caption {
            flat: bool,
            words: String,
            color: Hsla,
            layout: std::rc::Rc<std::cell::RefCell<Option<TextLayout>>>,
        }
        impl Render for Caption {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let t = cx.global::<Theme>();
                let text = if self.flat {
                    status_words(self.words.clone(), self.color)
                } else {
                    StyledText::new(self.words.clone())
                };
                self.layout.replace(Some(text.layout().clone()));
                let child = if self.flat {
                    text.into_any_element()
                } else {
                    div().text_color(self.color).child(text).into_any_element()
                };
                div()
                    .h_flex()
                    .items_center()
                    .text_size(t.text.meta.size)
                    .line_height(t.text.ui.line_height)
                    .text_color(t.colors.text_muted)
                    .child(child)
            }
        }
        cx.update(gpui_kit::init);
        let (caption, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(super::super::theme::Appearance::Dark, window, cx);
            Caption {
                flat: false,
                words: String::new(),
                color: Hsla::transparent_black(),
                layout: Default::default(),
            }
        });
        for appearance in [
            super::super::theme::Appearance::Dark,
            super::super::theme::Appearance::Light,
        ] {
            cx.update(|window, cx| Theme::apply(appearance, window, cx));
            for words in ["Working", "Done", "Needs you"] {
                let mut snapshots = Vec::new();
                for flat in [false, true] {
                    caption.update(cx, |caption, cx| {
                        caption.flat = flat;
                        caption.words = words.into();
                        caption.color = cx.global::<Theme>().colors.accent;
                        cx.notify();
                    });
                    cx.run_until_parked();
                    cx.update(|window, cx| {
                        window.refresh();
                        window.draw(cx).clear(cx);
                    });
                    snapshots.push(caption.read_with(cx, |caption, _| {
                        let layout = caption.layout.borrow();
                        let layout = layout.as_ref().unwrap();
                        format!(
                            "{:?}",
                            (layout.bounds(), layout.line_height(), layout.line_layouts())
                        )
                    }));
                }
                assert_eq!(snapshots[0], snapshots[1], "{appearance:?} {words}");
            }
        }
    }

    #[test]
    fn negative_zero_is_not_scrolled() {
        assert!(!super::rule_visible(
            super::MainView::Chat(super::ThreadId::new()),
            -super::Pixels::ZERO,
        ));
    }
    #[test]
    fn header_rule_only_when_scrolled() {
        use super::*;
        let step = Theme::new(super::super::theme::Appearance::Dark)
            .layout
            .border;
        let chat = MainView::Chat(ThreadId::new());
        assert!(!rule_visible(chat, Pixels::ZERO));
        assert!(rule_visible(chat, -step));
        assert!(!rule_visible(MainView::NewThread, -step));
        assert!(!rule_visible(MainView::Settings { back: None }, -step));
    }
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
