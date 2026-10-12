use super::{
    Shell,
    actions::*,
    composer::{self, labelled_button},
    theme::Theme,
};
use gpui_kit::component::StyledExt;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::text::TextView;
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::*;
pub(super) struct MessageScrollerState {
    list: ListState,
    follow: bool,
}
impl MessageScrollerState {
    pub fn new(count: usize, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(
            count,
            ListAlignment::Top,
            cx.global::<Theme>().layout.window_default.height,
        );
        list.set_follow_mode(FollowMode::Tail);
        let this = cx.weak_entity();
        list.set_scroll_handler(move |_, window, cx| {
            let line_height = cx
                .global::<Theme>()
                .text
                .read
                .line_height
                .to_pixels(window.rem_size());
            let this = this.clone();
            cx.defer(move |cx| {
                let _ = this.update(cx, |state, cx| {
                    state.follow = state.distance_to_bottom() <= line_height;
                    if state.follow {
                        state.list.set_follow_mode(FollowMode::Tail);
                    } else {
                        state.list.set_follow_mode(FollowMode::Normal);
                    }
                    cx.notify();
                });
            });
        });
        Self { list, follow: true }
    }
    fn distance_to_bottom(&self) -> Pixels {
        self.list.max_offset_for_scrollbar().y + self.list.scroll_px_offset_for_scrollbar().y
    }
    fn latest_visible(&self) -> bool {
        !self.follow && self.distance_to_bottom() > Pixels::ZERO
    }
    pub fn item_count(&self) -> usize {
        self.list.item_count()
    }
    pub fn reset(&mut self, count: usize, cx: &mut Context<Self>) {
        self.list.reset(count);
        self.scroll_to_end(cx);
    }
    pub fn append(&mut self, count: usize, cx: &mut Context<Self>) {
        let end = self.item_count();
        self.list.splice(end..end, count);
        cx.notify();
    }
    pub fn remeasure_item(&mut self, index: usize, cx: &mut Context<Self>) {
        self.list.remeasure_items(index..index + 1);
        cx.notify();
    }
    pub fn remeasure(&mut self, cx: &mut Context<Self>) {
        let count = self.item_count();
        self.list.remeasure_items(0..count);
        cx.notify();
    }
    pub fn expand(&mut self, index: usize, cx: &mut Context<Self>) {
        self.follow = false;
        self.list.set_follow_mode(FollowMode::Normal);
        self.list.remeasure_items(index..index + 1);
        cx.notify();
    }
    pub fn scroll_to_item(&mut self, index: usize, cx: &mut Context<Self>) {
        self.follow = false;
        self.list.set_follow_mode(FollowMode::Normal);
        self.list.scroll_to(ListOffset {
            item_ix: index,
            offset_in_item: Pixels::ZERO,
        });
        cx.notify();
    }
    pub fn scroll_by(&mut self, distance: Pixels, window: &Window, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Normal);
        self.list.scroll_by(distance);
        self.follow = self.distance_to_bottom()
            <= cx
                .global::<Theme>()
                .text
                .read
                .line_height
                .to_pixels(window.rem_size());
        if self.follow {
            self.scroll_to_end(cx);
        }
        cx.notify();
    }
    pub fn scroll_down(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.scroll_by(cx.global::<Theme>().layout.window_min.height, window, cx);
    }
    #[cfg(debug_assertions)]
    pub fn dump(&self) -> serde_json::Value {
        let top = self.list.logical_scroll_top();
        serde_json::json!({"follow":self.follow, "gap":f32::from(self.distance_to_bottom()),
            "top_item":top.item_ix, "top_offset":f32::from(top.offset_in_item),
            "item_tops":(0..self.item_count()).map(|index|
                self.list.bounds_for_item(index).map(|bounds| f32::from(bounds.origin.y))).collect::<Vec<_>>()})
    }
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.follow = true;
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
    }
}
pub(super) fn markdown_text(item: &Item) -> Option<SharedString> {
    match &item.body {
        ItemBody::AssistantText { text, done } => Some(if *done {
            text.clone().into()
        } else {
            format!("{text} ▏").into()
        }),
        ItemBody::Reasoning { text, .. } => Some(text.clone().into()),
        _ => None,
    }
}
fn row(
    item: &Item,
    expanded: bool,
    markdown: &std::collections::BTreeMap<ItemId, SharedString>,
    cx: &App,
) -> AnyElement {
    let t = cx.global::<Theme>();
    let mut row = div()
        .v_flex()
        .w_full()
        .gap(t.space.0[3])
        .text_size(t.text.read.size)
        .line_height(t.text.read.line_height);
    match &item.body {
        ItemBody::UserText(text) => {
            row = row.items_end().child(
                div()
                    .rounded(t.radius.large)
                    .bg(t.colors.bubble)
                    .px(t.space.0[5])
                    .py(t.space.0[4])
                    .child(text.clone()),
            )
        }
        ItemBody::AssistantText { .. } => {
            row = row.child(TextView::markdown(
                SharedString::from(format!("assistant-{}", item.id)),
                markdown
                    .get(&item.id)
                    .cloned()
                    .unwrap_or_else(|| markdown_text(item).unwrap()),
            ));
        }
        ItemBody::Reasoning { .. } => {
            let id = item.id;
            row = row
                .child(
                    labelled_button(
                        SharedString::from(format!("reasoning-{id}")),
                        if expanded {
                            "▾ Reasoning"
                        } else {
                            "▸ Reasoning"
                        },
                        cx,
                    )
                    .justify_start()
                    .child(div().flex_1())
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(ToggleItem { item: Some(id) }), cx)
                    }),
                )
                .when(expanded, |row| {
                    row.child(TextView::markdown(
                        SharedString::from(format!("reasoning-text-{}", item.id)),
                        markdown
                            .get(&item.id)
                            .cloned()
                            .unwrap_or_else(|| markdown_text(item).unwrap()),
                    ))
                });
        }
        ItemBody::Tool(tool) => {
            let id = item.id;
            let label = format!(
                "{} {} · {}",
                if expanded { "▾" } else { "▸" },
                tool.call.name,
                match &tool.result {
                    None => "Working",
                    Some(result) if result.ok => "Done",
                    Some(_) => "Failed",
                }
            );
            row = row
                .child(
                    labelled_button(SharedString::from(format!("tool-{id}")), label, cx)
                        .justify_start()
                        .child(div().flex_1())
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(Box::new(ToggleItem { item: Some(id) }), cx)
                        }),
                )
                .when_some(failed_tool_last_line(tool), |row, line| {
                    row.child(
                        div()
                            .debug_selector(|| "failed-tool-preview".into())
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(t.text.label.size)
                            .line_height(t.text.label.line_height)
                            .text_color(t.colors.danger)
                            .child(line.to_owned()),
                    )
                })
                .when(expanded, |row| {
                    row.child(
                        div()
                            .font_family(t.text.mono_font.clone())
                            .text_size(t.text.code.size)
                            .line_height(t.text.code.line_height)
                            .child(tool_input(&tool.call)),
                    )
                    .when_some(
                        tool_output(tool).map(str::to_owned),
                        |row, text| {
                            row.child(TextView::markdown(
                                SharedString::from(format!("tool-output-{}", item.id)),
                                format!("~~~text\n{text}\n~~~"),
                            ))
                        },
                    )
                });
        }
        ItemBody::Notice(notice) => {
            row = row
                .text_color(t.colors.text_muted)
                .child(notice.text.clone())
        }
        ItemBody::Plan(steps) => {
            row = row.children(steps.iter().map(|step| {
                div().child(format!(
                    "{} {}",
                    if step.done { "✓" } else { "·" },
                    step.text
                ))
            }))
        }
        ItemBody::TerminalSpan(span) => row = row.child(span.text.clone()),
        ItemBody::TaskCard(_) => {}
    }
    row.into_any_element()
}
pub fn turn_outcome_line(turn: &Turn) -> Option<String> {
    let TurnState::Ended {
        outcome,
        started,
        at,
    } = &turn.state
    else {
        return None;
    };
    let seconds = started.map(|started| at.saturating_sub(started) / 1000);
    Some(match outcome {
        TurnOutcome::Completed => {
            seconds.map_or_else(|| "Done".into(), |seconds| format!("Done in {seconds}s"))
        }
        TurnOutcome::Interrupted { reason } => seconds.map_or_else(
            || reason.clone(),
            |seconds| format!("{reason} after {seconds}s"),
        ),
        TurnOutcome::Failed { message } => {
            format!("Failed · {}", message.lines().next().unwrap_or_default())
        }
    })
}
fn tool_output(tool: &ToolItem) -> Option<&str> {
    tool.result
        .as_ref()
        .and_then(|result| result.output_tail.as_deref())
        .or_else(|| (!tool.output_tail.is_empty()).then_some(tool.output_tail.as_str()))
}
#[cfg(any(test, debug_assertions))]
pub fn row_outcome(
    items: &[Item],
    index: usize,
    turns: &std::collections::BTreeMap<TurnId, Turn>,
) -> Option<String> {
    let item = items.get(index)?;
    if items
        .get(index + 1)
        .is_some_and(|next| next.turn == item.turn)
    {
        return None;
    }
    turns.get(&item.turn).and_then(turn_outcome_line)
}
pub fn failed_tool_last_line(tool: &ToolItem) -> Option<&str> {
    let result = tool.result.as_ref().filter(|result| !result.ok)?;
    result
        .output_tail
        .as_ref()?
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim_end)
}
fn tool_input(call: &ToolCall) -> String {
    if let Some(summary) = &call.input_summary {
        return summary.clone();
    }
    let arguments = &call.arguments;
    arguments
        .as_object()
        .map(|fields| {
            fields
                .iter()
                .map(|(name, value)| {
                    format!(
                        "{name}: {}",
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| arguments.to_string())
}
pub fn render(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let thread = shell.view.chat();
    let entity = cx.entity();
    let wait_reason = shell
        .view
        .chat()
        .and_then(|thread| {
            shell
                .engine
                .window_threads()
                .get(&thread)
                .and_then(|state| state.wait_reason)
        })
        .map(|reason| reason.to_string());
    let mut content = div()
        .v_flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .px(t.layout.gutter);
    if let Some(message) = shell
        .view
        .chat()
        .and_then(|thread| shell.state.failure(thread))
        .map(str::to_owned)
    {
        let (cause, stderr) = message
            .split_once('\n')
            .map(|(cause, tail)| (cause.to_owned(), Some(tail.to_owned())))
            .unwrap_or((message, None));
        content =
            content.child(
                div()
                    .v_flex()
                    .w_full()
                    .max_w(t.layout.transcript_max)
                    .py(t.space.0[8])
                    .gap(t.space.0[4])
                    .child(
                        div()
                            .text_size(t.text.h2.size)
                            .text_color(t.colors.danger)
                            .child("The thread failed"),
                    )
                    .child(
                        div()
                            .text_size(t.text.read.size)
                            .line_height(t.text.read.line_height)
                            .child(cause),
                    )
                    .when_some(stderr, |row, stderr| {
                        row.child(TextView::markdown(
                            "failure-stderr",
                            format!("~~~text\n{stderr}\n~~~"),
                        ))
                    })
                    .child(
                        div()
                            .text_size(t.text.meta.size)
                            .text_color(t.colors.text_muted)
                            .child(format!(
                                "Check the CLI installation and sign in with {}, then retry.",
                                shell.chosen_provider().sign_in_command()
                            )),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(t.space.0[3])
                            .child(labelled_button("retry", "Retry", cx).on_click(
                                |_, window, cx| window.dispatch_action(Box::new(Retry), cx),
                            ))
                            .child(labelled_button("terminal", "Open Terminal", cx).on_click(
                                |_, window, cx| window.dispatch_action(Box::new(OpenTerminal), cx),
                            )),
                    ),
            );
    }
    content
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .max_w(t.layout.transcript_max)
                .vertical_scrollbar(&shell.scroller.read(cx).list)
                .child(
                    list(shell.scroller.read(cx).list.clone(), move |index, _, cx| {
                        let shell = entity.read(cx);
                        thread
                            .and_then(|thread| shell.state.transcript_item(thread, index))
                            .map(|item| {
                                let t = cx.global::<Theme>();
                                let turn = shell.state.turns.get(&item.turn);
                                let last = thread
                                    .and_then(|thread| {
                                        shell.state.transcript_item(thread, index + 1)
                                    })
                                    .is_none_or(|next| next.turn != item.turn);
                                let outcome = turn.filter(|_| last).and_then(turn_outcome_line);
                                div()
                                    .v_flex()
                                    .w_full()
                                    .gap(t.space.0[3])
                                    .child(row(
                                        item,
                                        shell.expanded.contains(&item.id),
                                        &shell.state.markdown_text,
                                        cx,
                                    ))
                                    .when_some(
                                        turn.filter(|turn| turn.state == TurnState::Queued)
                                            .and(wait_reason.as_ref()),
                                        |row, reason| {
                                            row.child(
                                                div()
                                                    .text_size(t.text.meta.size)
                                                    .text_color(t.colors.text_muted)
                                                    .child(reason.clone()),
                                            )
                                        },
                                    )
                                    .when_some(outcome, |row, outcome| {
                                        row.child(
                                            div()
                                                .text_size(t.text.meta.size)
                                                .text_color(t.colors.text_muted)
                                                .child(outcome),
                                        )
                                    })
                                    .into_any_element()
                            })
                            .unwrap_or_else(|| div().into_any_element())
                    })
                    .size_full()
                    .into_any_element(),
                ),
        )
        .when(shell.scroller.read(cx).latest_visible(), |content| {
            content.child(
                labelled_button("latest", "Latest", cx).on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(ScrollTranscriptEnd), cx)
                }),
            )
        })
        .child(
            div()
                .w_full()
                .max_w(t.layout.composer_max)
                .py(t.space.0[5])
                .child(composer::render(shell, cx)),
        )
}
#[cfg(debug_assertions)]
pub fn dump_item(item: &Item) -> serde_json::Value {
    let (kind, text, done) = match &item.body {
        ItemBody::UserText(text) => ("user", text.clone(), true),
        ItemBody::AssistantText { text, done } => ("assistant", text.clone(), *done),
        ItemBody::Reasoning { text, done } => ("reasoning", text.clone(), *done),
        ItemBody::Tool(tool) => (
            "tool",
            tool_output(tool).unwrap_or_default().to_owned(),
            tool.result.is_some(),
        ),
        ItemBody::Notice(notice) => ("notice", notice.text.clone(), true),
        _ => ("other", String::new(), true),
    };
    serde_json::json!({"id":item.id,"turn":item.turn,"kind":kind,"text":text,"done":done,"body":item.body})
}

#[cfg(test)]
mod tests {
    use super::tool_input;
    use gpui_kit::AppContext;
    use veyra_engine::domain::*;
    #[test]
    fn tool_input_formats_arguments_without_a_stored_summary() {
        for (name, arguments, expected) in [
            (
                "remote tool",
                serde_json::json!({"command":"remote operation"}),
                "command: remote operation",
            ),
            (
                "file tool",
                serde_json::json!({"content":"contents", "file_path":"/tmp/x"}),
                "content: contents\nfile_path: /tmp/x",
            ),
        ] {
            let call = ToolCall {
                name: name.into(),
                arguments,
                input_summary: None,
            };
            assert_eq!(tool_input(&call), expected);
        }
    }
    #[test]
    fn turn_outcome_line() {
        let mut turn = Turn {
            id: TurnId::new(),
            thread: ThreadId::new(),
            ordinal: 1,
            origin: TurnOrigin::Provider,
            state: TurnState::Running { since: 1000 },
        };
        assert_eq!(super::turn_outcome_line(&turn), None);
        turn.state = TurnState::Ended {
            outcome: TurnOutcome::Completed,
            started: Some(1000),
            at: 43_000,
        };
        assert_eq!(super::turn_outcome_line(&turn), Some("Done in 42s".into()));
        turn.state = TurnState::Ended {
            outcome: TurnOutcome::Interrupted {
                reason: "Stopped by you".into(),
            },
            started: Some(1000),
            at: 17_000,
        };
        assert_eq!(
            super::turn_outcome_line(&turn),
            Some("Stopped by you after 16s".into())
        );
        turn.state = TurnState::Ended {
            outcome: TurnOutcome::Failed {
                message: "No account\nprivate tail".into(),
            },
            started: None,
            at: 10,
        };
        assert_eq!(
            super::turn_outcome_line(&turn),
            Some("Failed · No account".into())
        );
    }
    #[gpui::test]
    fn failed_tool_preview_stays_one_line_at_narrow_width(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Preview(Item);
        impl Render for Preview {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(cx.global::<super::Theme>().layout.transcript_min)
                    .child(super::row(&self.0, false, &Default::default(), cx))
            }
        }
        cx.update(gpui_kit::init);
        let item = Item {
            id: ItemId::new(),
            thread: ThreadId::new(),
            turn: TurnId::new(),
            seq: 0,
            body: ItemBody::Tool(ToolItem {
                call: ToolCall {
                    name: "Bash".into(),
                    arguments: serde_json::json!({"command":"false"}),
                    input_summary: None,
                },
                output_tail: String::new(),
                result: Some(ToolResult {
                    ok: false,
                    output_tail: Some("a very long failure diagnostic ".repeat(40)),
                }),
            }),
        };
        let (view, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Preview(item)
        });
        let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
        cx.draw(
            Point::default(),
            size(
                AvailableSpace::Definite(theme.layout.transcript_min),
                AvailableSpace::Definite(theme.layout.window_default.height),
            ),
            |_, _| div().child(view),
        );
        let bounds = cx.debug_bounds("failed-tool-preview").unwrap();
        assert!(
            bounds.size.height
                <= theme
                    .text
                    .read
                    .line_height
                    .to_pixels(cx.update(|window, _| window.rem_size())),
            "preview wrapped: {:?}",
            bounds.size
        );
        assert!(
            bounds.size.width <= theme.layout.transcript_min,
            "preview exceeded its row width"
        );
    }
    #[gpui::test]
    fn latest_is_hidden_when_detached_at_bottom(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(super::Theme::new(crate::ui::theme::Appearance::Dark)));
        let state = cx.new(|cx| super::MessageScrollerState::new(1, cx));
        state.update(cx, |state, cx| state.scroll_to_item(0, cx));
        assert!(!state.read_with(cx, |state, _| state.latest_visible()));
    }
    #[test]
    fn running_subagent_tail_is_visible_before_result() {
        for name in ["Task", "Agent"] {
            let mut tool = ToolItem {
                call: ToolCall {
                    name: name.into(),
                    arguments: serde_json::json!({}),
                    input_summary: None,
                },
                output_tail: "still working".into(),
                result: None,
            };
            assert_eq!(super::tool_output(&tool), Some("still working"));
            tool.result = Some(ToolResult {
                ok: true,
                output_tail: Some("finished".into()),
            });
            assert_eq!(super::tool_output(&tool), Some("finished"));
        }
    }
    #[test]
    fn outcome_annotates_only_the_turns_last_item() {
        let thread = ThreadId::new();
        let turn = Turn {
            id: TurnId::new(),
            thread,
            ordinal: 1,
            origin: TurnOrigin::Provider,
            state: TurnState::Ended {
                outcome: TurnOutcome::Completed,
                started: Some(1000),
                at: 3000,
            },
        };
        let items = ["prompt", "reply"]
            .into_iter()
            .enumerate()
            .map(|(seq, text)| Item {
                id: ItemId::new(),
                thread,
                turn: turn.id,
                seq: seq as u32,
                body: ItemBody::UserText(text.into()),
            })
            .collect::<Vec<_>>();
        let turns = [(turn.id, turn)].into_iter().collect();
        assert_eq!(super::row_outcome(&items, 0, &turns), None);
        assert_eq!(
            super::row_outcome(&items, 1, &turns),
            Some("Done in 2s".into())
        );
    }
    #[test]
    fn failed_tool_last_line() {
        let mut tool = ToolItem {
            call: ToolCall {
                name: "Bash".into(),
                arguments: serde_json::json!({"command":"false"}),
                input_summary: Some("false".into()),
            },
            output_tail: String::new(),
            result: Some(ToolResult {
                ok: false,
                output_tail: Some("first\nlast line  \n\n".into()),
            }),
        };
        assert_eq!(super::failed_tool_last_line(&tool), Some("last line"));
        tool.result.as_mut().unwrap().ok = true;
        assert_eq!(super::failed_tool_last_line(&tool), None);
        assert_eq!(tool_input(&tool.call), "false");
    }
}
