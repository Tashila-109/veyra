#![allow(clippy::disallowed_types)] // Debug transcript dump methods serialize item wire JSON.
use super::{
    Shell,
    actions::*,
    composer::{self, labelled_button, labelled_button_with},
    provider_text::{QuoteKind, QuotedText},
    theme::Theme,
};
use gpui_kit::base::Disableable;
use gpui_kit::component::StyledExt;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::text::{
    MarkdownNode, MarkdownParseContext, MarkdownPlugin, TextView, markdown_ast,
};
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::*;
pub(super) fn notice_text(notice: &Notice, now_seconds: i64) -> String {
    let Some(reset) = notice.reset_at else {
        return notice.text.clone();
    };
    let seconds = reset.saturating_sub(now_seconds);
    let remaining = if seconds <= 0 {
        "The reset time has passed.".to_owned()
    } else if seconds < 60 {
        "Resets in less than a minute.".to_owned()
    } else {
        let minutes = seconds.saturating_add(59) / 60;
        if minutes >= 60 {
            format!("Resets in {} h {} min.", minutes / 60, minutes % 60)
        } else {
            format!("Resets in {minutes} min.")
        }
    };
    format!("{} {remaining}", notice.text)
}
pub(super) fn item_visible(item: &Item) -> bool {
    !matches!(&item.body, ItemBody::AssistantText { text, done: true } if text.trim().is_empty())
}
fn question_message(text: &str, questions: &[AsyncQuestion]) -> Option<SharedString> {
    let repeated: std::collections::BTreeSet<_> = questions
        .iter()
        .flat_map(|question| {
            std::iter::once(question.title.trim().to_owned()).chain(
                question
                    .options
                    .iter()
                    .map(|option| format!("- {}", option.trim())),
            )
        })
        .collect();
    let retained = text
        .lines()
        .filter(|line| !repeated.contains(line.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    (!retained.trim().is_empty()).then(|| veyra_engine::text::display_text(retained.trim()).into())
}
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
    pub fn insert(&mut self, index: usize, cx: &mut Context<Self>) {
        self.list.splice(index..index, 1);
        cx.notify();
    }
    #[cfg(test)]
    pub fn prepend(&mut self, cx: &mut Context<Self>) {
        self.insert(0, cx);
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
            veyra_engine::text::display_text(text).into()
        } else {
            format!("{} ▏", veyra_engine::text::display_text(text)).into()
        }),
        ItemBody::AsyncQuestions { text, questions } => question_message(text, questions),
        ItemBody::Reasoning { text, .. } => Some(veyra_engine::text::display_text(text).into()),
        _ => None,
    }
}
struct ReplyLineBreaks;
fn soft_break_offsets(node: &markdown_ast::Node, source: &str, offsets: &mut Vec<usize>) {
    if let markdown_ast::Node::Text(text) = node {
        if text.value.contains(['\n', '\r'])
            && let Some(position) = node.position()
        {
            let raw = &source[position.start.offset..position.end.offset];
            offsets.extend(raw.char_indices().filter_map(|(index, ch)| {
                (ch == '\r' || (ch == '\n' && !raw[..index].ends_with('\r')))
                    .then_some(position.start.offset + index)
            }));
        }
    } else if let Some(children) = node.children() {
        for child in children {
            soft_break_offsets(child, source, offsets);
        }
    }
}
impl MarkdownPlugin for ReplyLineBreaks {
    fn name(&self) -> &str {
        "veyra-reply-lines"
    }
    fn is_block(&self) -> bool {
        true
    }
    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let mut offsets = Vec::new();
        soft_break_offsets(node, cx.source(), &mut offsets);
        if offsets.is_empty() {
            return None;
        }
        let position = node.position()?;
        let mut source = cx.node_source(node)?.to_owned();
        for offset in offsets.into_iter().rev() {
            source.insert(offset - position.start.offset, '\\');
        }
        Some(
            MarkdownNode::new(self.name(), source.clone())
                .text(cx.node_source(node)?.to_owned())
                .markdown(cx.node_source(node)?.to_owned()),
        )
    }
    fn render(&self, node: &MarkdownNode, _: &mut Window, _: &mut App) -> impl IntoElement {
        TextView::markdown(
            SharedString::from(format!(
                "reply-lines-{:?}",
                node.source_range().map(|range| range.start)
            )),
            node.data::<String>().expect("reply line source").clone(),
        )
    }
}
fn reply_markdown(item: &Item, markdown: SharedString) -> impl IntoElement {
    TextView::markdown(
        SharedString::from(format!("assistant-{}", item.id)),
        markdown,
    )
    .plugin(ReplyLineBreaks)
}
pub(super) fn user_text(item: &Item, state: Option<&super::AppState>) -> String {
    let ItemBody::UserText(text) = &item.body else {
        return String::new();
    };
    let choice = state
        .and_then(|state| state.items.get(&item.thread))
        .is_some_and(|items| {
            items.values().any(|previous| {
                previous.seq < item.seq
                    && matches!(&previous.body,
            ItemBody::AsyncQuestions { questions, .. } if questions.iter().any(|question|
                question.selected.and_then(|index| question.options.get(index))
                    .is_some_and(|option| option == text)))
            })
        });
    if choice {
        veyra_engine::text::display_line(text, false).text
    } else {
        veyra_engine::text::display_text(text)
    }
}
pub(super) fn tool_status(item: &Item, state: Option<&super::AppState>) -> &'static str {
    let ItemBody::Tool(tool) = &item.body else {
        return "";
    };
    if tool.result.as_ref().is_some_and(|result| result.ok) {
        return "Done";
    }
    let denied = state.is_some_and(|state| {
        state.requests.iter().any(|request| {
            if request.turn != item.turn || request.state != RequestState::Answered(Decision::Deny)
            {
                return false;
            }
            let RequestKind::Approval { action } = &request.kind else {
                return false;
            };
            match &action.target {
                ApprovalTarget::Command { command, .. } => {
                    tool.call
                        .arguments
                        .get("command")
                        .and_then(|value| value.as_str())
                        .or(tool.call.input_summary.as_deref())
                        == Some(command.as_str())
                }
                ApprovalTarget::File { path, .. } => ["file_path", "path", "notebook_path"]
                    .into_iter()
                    .any(|key| {
                        tool.call
                            .arguments
                            .get(key)
                            .and_then(|value| value.as_str())
                            .is_some_and(|value| std::path::Path::new(value) == path)
                    }),
                ApprovalTarget::Tool { .. } => tool.call.name == action.tool,
            }
        })
    });
    if denied {
        return "Denied by you";
    }
    match &tool.result {
        None => "Working",
        Some(result)
            if result.output_tail.as_deref()
                == Some("The turn ended before this tool returned")
                && state.is_some_and(|state| {
                    state.turns.get(&item.turn).is_some_and(|turn| {
                        matches!(
                            turn.state,
                            TurnState::Ended {
                                outcome: TurnOutcome::Interrupted { .. },
                                ..
                            }
                        )
                    })
                }) =>
        {
            "Stopped"
        }
        Some(_) => "Failed",
    }
}
#[cfg(test)]
fn row(
    item: &Item,
    expanded: bool,
    markdown: Option<SharedString>,
    choices_active: bool,
    cx: &App,
) -> AnyElement {
    row_with_state(item, expanded, markdown, choices_active, None, cx)
}
fn row_with_state(
    item: &Item,
    expanded: bool,
    markdown: Option<SharedString>,
    choices_active: bool,
    state: Option<&super::AppState>,
    cx: &App,
) -> AnyElement {
    let t = cx.global::<Theme>();
    let row_id = item.id;
    let mut row = div()
        .debug_selector(move || format!("transcript-body-{row_id}"))
        .v_flex()
        .w_full()
        .gap(t.space.0[3])
        .text_size(t.text.read.size)
        .line_height(t.text.read.line_height);
    match &item.body {
        ItemBody::UserText(_) => {
            row = row.items_end().child(
                div()
                    .debug_selector(|| "user-bubble".into())
                    .rounded(t.radius.large)
                    .bg(t.colors.bubble)
                    .px(t.space.0[5])
                    .py(t.space.0[4])
                    .child(user_text(item, state)),
            )
        }
        ItemBody::AssistantText { .. } => {
            row = row.child(reply_markdown(
                item,
                markdown.expect("markdown rows have cached text"),
            ));
        }
        ItemBody::AsyncQuestions { questions, .. } => {
            if let Some(markdown) = markdown {
                row = row.child(
                    div()
                        .debug_selector(|| "async-question-message".into())
                        .child(reply_markdown(item, markdown)),
                );
            }
            for (question_index, question) in questions.iter().enumerate() {
                let display = super::provider_text::question_display(question);
                let mut card = div()
                    .v_flex()
                    .w_full()
                    .gap(t.space.0[3])
                    .rounded(t.radius.large)
                    .bg(t.colors.card)
                    .p(t.space.0[4])
                    .child(
                        QuotedText::from_quoted(
                            QuoteKind::Question,
                            "Question from the provider".into(),
                            display.title,
                        )
                        .render("async-question-title", cx),
                    )
                    .child(
                        div()
                            .text_size(t.text.label.size)
                            .text_color(t.colors.text_muted)
                            .child("Choosing sends this option as a message, stops current work first and conti\
                                nues queued messages."),
                    );
                for (option_index, _) in question.options.iter().enumerate() {
                    let action = ChooseAsyncOption {
                        item: Some(item.id),
                        question: Some(question_index),
                        option: Some(option_index),
                    };
                    card = card.child(
                        labelled_button_with(
                            SharedString::from(format!(
                                "async-{}-{question_index}-{option_index}",
                                item.id
                            )),
                            display.options[option_index].clone(),
                            div()
                                .whitespace_normal()
                                .line_clamp(2)
                                .text_ellipsis()
                                .child(display.options[option_index].clone()),
                            cx,
                        )
                        .h_auto()
                        .min_h(t.layout.icon_button)
                        .py(t.space.0[2])
                        .justify_start()
                        .border(t.layout.border)
                        .border_color(t.colors.border_strong)
                        .bg(t.colors.card)
                        .debug_selector(move || format!("async-option-{option_index}"))
                        .disabled(!choices_active || question.selected.is_some())
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(Box::new(action.clone()), cx)
                        }),
                    );
                }
                if let Some(selected) = question
                    .selected
                    .and_then(|index| question.options.get(index))
                {
                    card = card.child(
                        div()
                            .debug_selector(|| "answered-choice".into())
                            .text_color(t.colors.text_muted)
                            .child(format!(
                                "Answered: {}",
                                super::provider_text::quoted(selected, false)
                            )),
                    );
                }
                if !choices_active && question.selected.is_none() {
                    card = card.child(
                        div()
                            .text_color(t.colors.text_muted)
                            .child("Conversation continued; choices are closed."),
                    );
                }
                row = row.child(card);
            }
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
                        markdown.expect("markdown rows have cached text"),
                    ))
                });
        }
        ItemBody::Tool(tool) => {
            let id = item.id;
            let label = format!(
                "{} {} · {}",
                if expanded { "▾" } else { "▸" },
                tool_label(&tool.call),
                tool_status(item, state)
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
                    .when_some(tool_output(tool), |row, text| {
                        row.child(TextView::markdown(
                            SharedString::from(format!("tool-output-{}", item.id)),
                            format!("~~~text\n{text}\n~~~"),
                        ))
                    })
                });
        }
        ItemBody::Notice(notice) => {
            row = row
                .text_color(t.colors.text_muted)
                .child(notice_text(notice, veyra_engine::actor::now() / 1000))
        }
        ItemBody::Plan(steps) => {
            row = row.children(steps.iter().map(|step| {
                div().child(format!(
                    "{} {}",
                    if step.done { "✓" } else { "·" },
                    super::provider_text::quoted(&step.text, false)
                ))
            }))
        }
        ItemBody::TerminalSpan(span) => {
            row = row
                .debug_selector(|| "stored-terminal-span".into())
                .p(t.space.0[4])
                .rounded(t.radius.card)
                .border(t.layout.border)
                .border_color(t.colors.border)
                .bg(t.colors.card)
                .child(
                    div()
                        .debug_selector(|| "stored-terminal-label".into())
                        .text_size(t.text.meta.size)
                        .text_color(t.colors.text_muted)
                        .child("This turn ran in the terminal"),
                )
                .child(
                    div()
                        .debug_selector(|| "stored-terminal-text".into())
                        .font_family(t.text.mono_font.clone())
                        .text_size(t.text.terminal.size)
                        .line_height(t.text.terminal.line_height)
                        .child(veyra_engine::text::display_text(&span.text)),
                );
        }
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
    let recovered = matches!(outcome, TurnOutcome::Interrupted { reason }
        if reason == veyra_engine::provider::StopReason::AppClosing.text());
    let seconds = started
        .filter(|_| !recovered)
        .map(|started| at.saturating_sub(started) / 1000);
    let words = outcome.words();
    Some(match (outcome, seconds) {
        (TurnOutcome::Completed, Some(seconds)) => format!("{words} · {seconds}s"),
        (TurnOutcome::Interrupted { .. }, Some(seconds)) => format!("{words} · {seconds}s"),
        _ => words,
    })
}
fn tool_output(tool: &ToolItem) -> Option<String> {
    let raw = tool
        .result
        .as_ref()
        .and_then(|result| result.output_tail.as_deref())
        .or_else(|| (!tool.output_tail.is_empty()).then_some(tool.output_tail.as_str()))?;
    fn text(value: &serde_json::Value) -> Option<String> {
        if let Some(value) = value.as_str() {
            return Some(value.to_owned());
        }
        if let Some(value) = value.get("text").and_then(|value| value.as_str()) {
            return Some(value.to_owned());
        }
        if let Some(content) = value.get("content") {
            return text(content);
        }
        value
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|text| !text.is_empty())
    }
    let projected = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .as_ref()
        .and_then(text);
    Some(veyra_engine::text::display_text(
        projected.as_deref().unwrap_or(raw),
    ))
}
pub(super) fn row_outcome(
    item: &Item,
    next: Option<&Item>,
    state: &super::AppState,
) -> Option<String> {
    if next.is_some_and(|next| next.turn == item.turn) {
        return None;
    }
    if state
        .failed_turn(item.thread)
        .is_some_and(|turn| turn.id == item.turn)
    {
        return Some("Failed".into());
    }
    state.turns.get(&item.turn).and_then(turn_outcome_line)
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
pub(super) fn family_children(state: &super::AppState, parent: ThreadId) -> Vec<&Thread> {
    state
        .threads
        .values()
        .filter(|thread| thread.id != parent && thread.root() == parent)
        .collect()
}
pub(super) struct FamilyCard {
    pub turn: Option<TurnId>,
    pub children: Vec<ThreadId>,
}
pub(super) fn family_cards(state: &super::AppState, parent: ThreadId) -> Vec<FamilyCard> {
    let mut groups: std::collections::BTreeMap<Option<TurnId>, Vec<ThreadId>> = Default::default();
    for child in family_children(state, parent) {
        let task = state
            .tasks
            .values()
            .filter(|task| task.parent == parent && task.child == child.id)
            .min_by_key(|task| state.turns.get(&task.turn).map(|turn| turn.ordinal));
        let source = task.and_then(|task| task.request_key.strip_prefix("start_thread:")).and_then(|key|
            state.items.get(&parent).into_iter().flat_map(|items| items.values()).find(|item|
                matches!(&item.body, ItemBody::Tool(tool) if tool.call.arguments.get("client_request_id")
                    .and_then(|value| value.as_str()) == Some(key)))).map(|item| item.turn);
        let started = task
            .and_then(|task| state.turns.get(&task.turn))
            .and_then(|turn| turn.state.started());
        let turn = source.or_else(|| {
            state
                .turns
                .values()
                .filter(|turn| turn.thread == parent)
                .filter(|turn| {
                    started
                        .is_none_or(|started| turn.state.started().is_some_and(|at| at <= started))
                })
                .max_by_key(|turn| turn.recency())
                .map(|turn| turn.id)
        });
        groups.entry(turn).or_default().push(child.id);
    }
    let mut groups: Vec<_> = groups
        .into_iter()
        .map(|(turn, children)| FamilyCard { turn, children })
        .collect();
    groups.sort_by_key(|card| {
        card.turn
            .and_then(|turn| state.turns.get(&turn))
            .map(|turn| turn.recency())
    });
    groups
}
pub(super) fn family_rows(
    state: &super::AppState,
    parent: ThreadId,
) -> Vec<(Option<TurnId>, usize)> {
    family_cards(state, parent)
        .into_iter()
        .enumerate()
        .map(|(offset, card)| {
            let after = card
                .turn
                .and_then(|turn| state.turns.get(&turn))
                .map(|turn| turn.recency());
            let index = (0..state.transcript_len(parent))
                .find(|&index| {
                    state
                        .transcript_item(parent, index)
                        .and_then(|item| state.turns.get(&item.turn))
                        .is_some_and(|turn| after.is_some_and(|after| turn.recency() > after))
                })
                .unwrap_or(state.transcript_len(parent));
            (card.turn, index + offset)
        })
        .collect()
}
pub(super) fn family_title(count: usize) -> String {
    format!("Started {count} agent{}", if count == 1 { "" } else { "s" })
}
fn family_card(shell: &Shell, children: &[ThreadId], cx: &App) -> AnyElement {
    let t = cx.global::<Theme>();
    let children: Vec<_> = children
        .iter()
        .filter_map(|child| shell.state.threads.get(child))
        .collect();
    div()
        .debug_selector(|| "family-card".into())
        .v_flex()
        .w_full()
        .max_w(t.layout.transcript_max)
        .rounded(t.radius.card)
        .bg(t.colors.card)
        .border(t.layout.border)
        .border_color(t.colors.border)
        .p(t.space.0[4])
        .gap(t.space.0[3])
        .child(family_title(children.len()))
        .children(children.into_iter().map(|child| {
            let id = child.id;
            let status = shell.state.status(id);
            div()
                .h_flex()
                .items_center()
                .gap(t.space.0[3])
                .child(
                    div()
                        .w(t.layout.status_glyph)
                        .text_color(super::sidebar::glyph_color(&status, t))
                        .child(super::sidebar::glyph(&status)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(super::provider_text::quoted(&child.title, false)),
                )
                .child(
                    div()
                        .text_size(t.text.meta.size)
                        .text_color(t.colors.text_muted)
                        .child(super::header::status_text(&status)),
                )
                .child(
                    labelled_button(SharedString::from(format!("child-open-{id}")), "Open", cx)
                        .disabled(child.archived)
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(Box::new(SelectThread { thread: Some(id) }), cx)
                        }),
                )
                .when(shell.family_stop_visible(child), |row| {
                    row.child(
                        labelled_button(SharedString::from(format!("child-stop-{id}")), "Stop", cx)
                            .debug_selector(|| "family-child-stop".into())
                            .on_click(move |_, window, cx| {
                                window
                                    .dispatch_action(Box::new(StopThread { thread: Some(id) }), cx)
                            }),
                    )
                })
        }))
        .into_any_element()
}
fn task_card(state: &super::AppState, task: TaskId, cx: &App) -> AnyElement {
    let t = cx.global::<Theme>();
    div()
        .debug_selector(|| "task-card".into())
        .rounded(t.radius.card)
        .bg(t.colors.card)
        .border(t.layout.border)
        .border_color(t.colors.border)
        .p(t.space.0[4])
        .child(task_card_text(state, task))
        .into_any_element()
}
fn task_from(name: &str) -> String {
    format!("Task from {name}")
}
pub(super) fn task_card_text(state: &super::AppState, task: TaskId) -> String {
    let parent = state
        .tasks
        .get(&task)
        .and_then(|task| state.threads.get(&task.parent));
    task_from(
        parent
            .map(|thread| thread.provider.label())
            .unwrap_or("parent"),
    )
}
pub(super) fn turn_label(state: &super::AppState, turn: &Turn) -> Option<String> {
    match &turn.origin {
        TurnOrigin::Task { task } => state
            .tasks
            .get(task)
            .and_then(|task| state.threads.get(&task.parent))
            .map(|parent| task_from(&super::provider_text::quoted(&parent.title, false))),
        TurnOrigin::Wake { tasks } => {
            let count = tasks
                .iter()
                .filter_map(|task| state.tasks.get(task).map(|task| task.child))
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            Some(format!(
                "Reports from {count} child thread{}",
                if count == 1 { "" } else { "s" }
            ))
        }
        TurnOrigin::User { .. } | TurnOrigin::Provider => None,
    }
}
pub(super) fn report_delivered(state: &super::AppState, item: &Item) -> bool {
    matches!(item.body, ItemBody::AssistantText { .. })
        && state.tasks.values().any(|task| {
            task.child == item.thread
                && matches!(task.state, TaskState::Settled {
                    source: Some(source), delivery: Delivery::Delivered { .. }, ..
                } if source == item.id)
        })
}

fn wait_row(reason: String, cx: &App) -> Div {
    let t = cx.global::<Theme>();
    div().h_flex().w_full().justify_end().child(
        div()
            .debug_selector(|| "queued-message-wait".into())
            .text_size(t.text.meta.size)
            .text_color(t.colors.text_muted)
            .child(reason),
    )
}
fn failure_card(message: &str, cx: &App) -> AnyElement {
    let t = cx.global::<Theme>();
    let (cause, stderr) = message.split_once('\n').unwrap_or((message, ""));
    div()
        .v_flex()
        .w_full()
        .gap(t.space.0[4])
        .py(t.space.0[5])
        .debug_selector(|| "failure-card".into())
        .child(
            div()
                .text_size(t.text.h2.size)
                .text_color(t.colors.danger)
                .child("The thread failed"),
        )
        .child(
            div()
                .debug_selector(|| "failure-cause".into())
                .text_size(t.text.read.size)
                .line_height(t.text.read.line_height)
                .child(cause.to_owned()),
        )
        .when(!stderr.is_empty(), |card| {
            card.child(TextView::markdown(
                "failure-stderr",
                format!("~~~text\n{stderr}\n~~~"),
            ))
        })
        .child(
            div()
                .h_flex()
                .gap(t.space.0[3])
                .child(
                    labelled_button("retry", "Retry", cx)
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(Retry), cx)),
                )
                .child(
                    labelled_button("terminal", "Open Terminal", cx).on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(OpenTerminal), cx)
                    }),
                ),
        )
        .into_any_element()
}
fn outcome_row(outcome: String, cx: &App) -> Div {
    let t = cx.global::<Theme>();
    let (words, elapsed) = outcome
        .rsplit_once(" · ")
        .filter(|(_, tail)| {
            tail.strip_suffix('s')
                .is_some_and(|seconds| seconds.parse::<u64>().is_ok())
        })
        .map_or((outcome.as_str(), None), |(words, elapsed)| {
            (words, Some(elapsed))
        });
    div()
        .h_flex()
        .gap(t.space.0[3])
        .text_size(t.text.meta.size)
        .text_color(t.colors.text_muted)
        .child(
            div()
                .debug_selector(|| "outcome-words".into())
                .child(words.to_owned()),
        )
        .when_some(elapsed, |row, elapsed| {
            row.child(
                div()
                    .debug_selector(|| "outcome-elapsed".into())
                    .child(elapsed.to_owned()),
            )
        })
}
pub fn render(shell: &mut Shell, cx: &Context<Shell>) -> impl IntoElement {
    #[cfg(debug_assertions)]
    if let Some(open) = &mut shell.state.transcript_open
        && shell.view.chat() == Some(open.thread)
        && open.installed
        && open.milliseconds.is_none()
    {
        open.milliseconds = Some(open.requested.elapsed().as_secs_f64() * 1000.);
    }
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
    let content = div()
        .v_flex()
        .relative()
        .flex_1()
        .min_h_0()
        .items_center()
        .px(t.layout.gutter);
    content
        .child(
            div()
                .flex_1()
                .min_h(t.text.read.line_height.to_pixels(t.text.base) * 3.)
                .w_full()
                .max_w(t.layout.transcript_max)
                .vertical_scrollbar(&shell.scroller.read(cx).list)
                .child(
                    list(shell.scroller.read(cx).list.clone(), move |index, _, cx| {
                        let shell = entity.read(cx);
                        let families = thread
                            .map(|thread| family_rows(&shell.state, thread))
                            .unwrap_or_default();
                        if let Some(card) =
                            families.iter().position(|(_, position)| *position == index)
                        {
                            let groups = family_cards(&shell.state, thread.unwrap());
                            return family_card(shell, &groups[card].children, cx);
                        }
                        let index = index
                            - families
                                .iter()
                                .filter(|(_, position)| *position < index)
                                .count();
                        thread
                            .and_then(|thread| shell.state.transcript_item(thread, index))
                            .map(|item| {
                                let t = cx.global::<Theme>();
                                let turn = shell.state.turns.get(&item.turn);
                                let next = thread.and_then(|thread| {
                                    shell.state.transcript_item(thread, index + 1)
                                });
                                let outcome = row_outcome(item, next, &shell.state);
                                let first = index == 0
                                    || thread
                                        .and_then(|thread| {
                                            shell.state.transcript_item(thread, index - 1)
                                        })
                                        .is_none_or(|previous| previous.turn != item.turn);
                                let label = turn
                                    .filter(|_| first)
                                    .and_then(|turn| turn_label(&shell.state, turn));
                                div()
                                    .v_flex()
                                    .w_full()
                                    .pb(t.space.0[5])
                                    .gap(t.space.0[3])
                                    .when_some(label, |row, label| {
                                        row.child(
                                            div()
                                                .debug_selector(|| "turn-label".into())
                                                .text_size(t.text.meta.size)
                                                .text_color(t.colors.text_muted)
                                                .child(label),
                                        )
                                    })
                                    .child(if let ItemBody::TaskCard(task) = item.body {
                                        task_card(&shell.state, task, cx)
                                    } else {
                                        row_with_state(
                                            item,
                                            shell.expanded.contains(&item.id),
                                            shell.state.markdown_text.get(&item.id).cloned(),
                                            async_question_active(
                                                item,
                                                shell
                                                    .state
                                                    .items
                                                    .get(&item.thread)
                                                    .into_iter()
                                                    .flat_map(|items| items.values()),
                                            ),
                                            Some(&shell.state),
                                            cx,
                                        )
                                    })
                                    .when(report_delivered(&shell.state, item), |row| {
                                        row.child(
                                            div()
                                                .text_size(t.text.meta.size)
                                                .text_color(t.colors.text_muted)
                                                .child("Report delivered"),
                                        )
                                    })
                                    .when_some(
                                        turn.filter(|turn| turn.state == TurnState::Queued)
                                            .and(wait_reason.as_ref()),
                                        |row, reason| row.child(wait_row(reason.clone(), cx)),
                                    )
                                    .when(
                                        shell
                                            .state
                                            .failed_turn(item.thread)
                                            .is_some_and(|turn| turn.id == item.turn)
                                            && next.is_none_or(|next| next.turn != item.turn),
                                        |row| {
                                            row.child(failure_card(
                                                shell
                                                    .state
                                                    .failure(item.thread)
                                                    .unwrap_or_default(),
                                                cx,
                                            ))
                                        },
                                    )
                                    .when_some(outcome, |row, outcome| {
                                        row.child(outcome_row(outcome, cx))
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
                .v_flex()
                .min_h_0()
                .flex_none()
                .max_h(
                    (shell.drawer_viewport
                        - t.layout.header
                        - t.text.read.line_height.to_pixels(t.text.base) * 3.
                        - if shell.active_shell().is_some() {
                            t.layout.drawer_min
                        } else {
                            Pixels::ZERO
                        })
                    .max(t.layout.composer_min_height),
                )
                .py(t.space.0[3])
                .relative()
                .debug_selector(|| "composition-surface".into())
                .child(composer::render(shell, cx))
                .child(
                    canvas(
                        {
                            let shell = cx.entity().downgrade();
                            move |bounds, _, cx| {
                                let _ = shell.update(cx, |shell, cx| {
                                    if shell.composition_height != bounds.size.height {
                                        shell.composition_height = bounds.size.height;
                                        cx.notify();
                                    }
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                ),
        )
        .child(
            div()
                .absolute()
                .bottom(shell.composition_height)
                .w_full()
                .max_w(t.layout.composer_max)
                .h(t.layout.fade)
                .bg(t.composer_fade()),
        )
}
fn tool_label(call: &ToolCall) -> String {
    let name = veyra_engine::text::display_line(&call.name, false).text;
    let argument = call
        .input_summary
        .as_deref()
        .or_else(|| {
            ["command", "query", "pattern", "file_path", "path", "url"]
                .into_iter()
                .find_map(|key| call.arguments.get(key).and_then(|value| value.as_str()))
        })
        .map(|value| veyra_engine::text::display_line(value, false).text);
    match argument.filter(|argument| !argument.is_empty() && !name.contains(argument)) {
        Some(argument) => format!("{name} · {argument}"),
        None => name,
    }
}

#[cfg(debug_assertions)]
pub fn dump_item(item: &Item) -> serde_json::Value {
    let (kind, text, done) = match &item.body {
        ItemBody::UserText(text) => ("user", veyra_engine::text::display_text(text), true),
        ItemBody::AssistantText { text, done } => {
            ("assistant", veyra_engine::text::display_text(text), *done)
        }
        ItemBody::AsyncQuestions { text, questions } => (
            "async_questions",
            question_message(text, questions)
                .unwrap_or_default()
                .to_string(),
            true,
        ),
        ItemBody::Reasoning { text, done } => ("reasoning", text.clone(), *done),
        ItemBody::Tool(tool) => (
            "tool",
            tool_output(tool).unwrap_or_default().to_owned(),
            tool.result.is_some(),
        ),
        ItemBody::Notice(notice) => (
            "notice",
            notice_text(notice, veyra_engine::actor::now() / 1000),
            true,
        ),
        ItemBody::TerminalSpan(span) => ("terminal_span", span.text.clone(), true),
        _ => ("other", String::new(), true),
    };
    let mut value = serde_json::json!({"id":item.id,"turn":item.turn,"kind":kind,"text":text,"done":done,"body\
        ":item.body});
    if let ItemBody::Tool(tool) = &item.body {
        value["tool_label"] = serde_json::json!(tool_label(&tool.call));
    }
    if let ItemBody::AsyncQuestions { questions, .. } = &item.body {
        value["display_questions"] = serde_json::json!(
            questions
                .iter()
                .map(super::provider_text::question_display)
                .collect::<Vec<_>>()
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::StyledExt;
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn outcome_elapsed_is_a_separate_segment(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Outcome;
        impl Render for Outcome {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                super::outcome_row("Stopped after you denied a request · 2s".into(), cx)
            }
        }
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Outcome
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let words = cx.debug_bounds("outcome-words").unwrap();
        let elapsed = cx.debug_bounds("outcome-elapsed").unwrap();
        assert!(elapsed.left() > words.right());
        assert_eq!(elapsed.top(), words.top());
    }
    #[test]
    fn timed_notice_renders_relative_time_and_keeps_legacy_words() {
        let notice = Notice {
            text: "Usage limit.".into(),
            reset_at: Some(10_000),
        };
        assert_eq!(
            super::notice_text(&notice, 2_200),
            "Usage limit. Resets in 2 h 10 min."
        );
        assert_eq!(
            super::notice_text(&notice, 9_000),
            "Usage limit. Resets in 17 min."
        );
        assert_eq!(
            super::notice_text(&notice, 9_999),
            "Usage limit. Resets in less than a minute."
        );
        assert_eq!(
            super::notice_text(&notice, 10_000),
            "Usage limit. The reset time has passed."
        );
        let old: Notice =
            serde_json::from_str(r#"{"text":"Usage limit; resets at 12345."}"#).unwrap();
        assert_eq!(super::notice_text(&old, 10_000), old.text);
        assert_eq!(notice.reset_at, Some(10_000));
    }
    #[test]
    fn reconciled_outcome_has_no_elapsed_segment() {
        let turn = Turn {
            id: TurnId::new(),
            thread: ThreadId::new(),
            ordinal: 1,
            origin: TurnOrigin::Provider,
            state: TurnState::Ended {
                started: Some(1),
                at: 8_000_000,
                outcome: TurnOutcome::Interrupted {
                    reason: veyra_engine::provider::StopReason::AppClosing.text().into(),
                },
            },
        };
        assert_eq!(
            super::turn_outcome_line(&turn).unwrap(),
            veyra_engine::provider::StopReason::AppClosing.text()
        );
    }
    #[test]
    fn tool_output_prefers_text_from_structured_payload() {
        for (raw, expected) in [
            (
                r#"{"content":[{"type":"text","text":"first"},{"type":"text","text":"second"}]}"#,
                "first\nsecond",
            ),
            (
                r#"{"text":"plain output","metadata":{"count":2}}"#,
                "plain output",
            ),
            (r#"{"count":2}"#, r#"{"count":2}"#),
            ("plain\noutput", "plain\noutput"),
        ] {
            let tool = ToolItem {
                call: ToolCall {
                    name: "Read".into(),
                    veyra_label: false,
                    arguments: serde_json::json!({}),
                    input_summary: None,
                },
                output_tail: String::new(),
                result: Some(ToolResult {
                    ok: true,
                    output_tail: Some(raw.into()),
                }),
            };
            assert_eq!(super::tool_output(&tool).as_deref(), Some(expected));
        }
    }
    #[test]
    fn tool_titles_are_unquoted_and_name_main_arguments() {
        for (name, arguments, expected) in [
            (
                "Bash",
                serde_json::json!({"command":"echo violet"}),
                "Bash · echo violet",
            ),
            (
                "Grep",
                serde_json::json!({"pattern":"needle"}),
                "Grep · needle",
            ),
        ] {
            assert_eq!(
                super::tool_label(&ToolCall {
                    name: name.into(),
                    arguments,
                    veyra_label: false,
                    input_summary: None
                }),
                expected
            );
        }
    }
    #[test]
    fn denied_tools_name_the_exact_human_decision_for_each_provider() {
        for name in ["Bash", "Command", "run_terminal_command"] {
            let item = Item {
                id: ItemId::new(),
                thread: ThreadId::new(),
                turn: TurnId::new(),
                seq: 1,
                body: ItemBody::Tool(ToolItem {
                    call: ToolCall {
                        name: name.into(),
                        veyra_label: false,
                        arguments: serde_json::json!({"command":"touch denied"}),
                        input_summary: None,
                    },
                    output_tail: String::new(),
                    result: Some(ToolResult {
                        ok: false,
                        output_tail: None,
                    }),
                }),
            };
            let mut state = super::super::AppState::default();
            state.requests.push(Request {
                id: RequestId::new(),
                thread: item.thread,
                turn: item.turn,
                native: NativeRequestRef("fixture".into()),
                state: RequestState::Answered(Decision::Deny),
                kind: RequestKind::Approval {
                    action: ProposedAction::new(
                        name.into(),
                        ApprovalTarget::Command {
                            command: "touch denied".into(),
                            cwd: CommandCwd::ProjectFolder,
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    ),
                },
            });
            assert_eq!(super::tool_status(&item, Some(&state)), "Denied by you");
            let RequestKind::Approval { action } = &mut state.requests[0].kind else {
                unreachable!()
            };
            let ApprovalTarget::Command { command, .. } = &mut action.target else {
                unreachable!()
            };
            *command = "different command".into();
            assert_eq!(super::tool_status(&item, Some(&state)), "Failed");
        }
    }
    #[test]
    fn completed_tool_stays_done_while_only_open_tools_read_stopped() {
        let turn = Turn {
            id: TurnId::new(),
            thread: ThreadId::new(),
            ordinal: 1,
            origin: TurnOrigin::Provider,
            state: TurnState::Ended {
                started: Some(1),
                at: 100,
                outcome: TurnOutcome::Interrupted {
                    reason: "Stopped by you".into(),
                },
            },
        };
        let mut state = super::super::AppState::default();
        state.turns.insert(turn.id, turn.clone());
        let mut item = Item {
            id: ItemId::new(),
            thread: turn.thread,
            turn: turn.id,
            seq: 1,
            body: ItemBody::Tool(ToolItem {
                call: ToolCall {
                    name: "Command".into(),
                    veyra_label: true,
                    arguments: serde_json::json!({}),
                    input_summary: None,
                },
                output_tail: String::new(),
                result: Some(ToolResult {
                    ok: true,
                    output_tail: Some("finished result".into()),
                }),
            }),
        };
        assert_eq!(super::tool_status(&item, Some(&state)), "Done");
        let ItemBody::Tool(tool) = &mut item.body else {
            unreachable!()
        };
        assert_eq!(super::tool_output(tool).as_deref(), Some("finished result"));
        tool.result = Some(ToolResult {
            ok: false,
            output_tail: Some("The turn ended before this tool returned".into()),
        });
        assert_eq!(super::tool_status(&item, Some(&state)), "Stopped");
    }
    #[test]
    fn question_copy_appears_once_and_empty_finished_replies_are_hidden() {
        let questions = [AsyncQuestion {
            title: "Choose a colour?".into(),
            options: vec!["Violet".into(), "Blue".into()],
            selected: None,
        }];
        assert_eq!(
            super::question_message("Choose a colour?\n- Violet\n- Blue", &questions),
            None
        );
        assert_eq!(
            super::question_message(
                "A useful preface.\n\nChoose a colour?\n- Violet\n- Blue",
                &questions
            )
            .as_deref(),
            Some("A useful preface.")
        );
        let item = Item {
            id: ItemId::new(),
            thread: ThreadId::new(),
            turn: TurnId::new(),
            seq: 1,
            body: ItemBody::AssistantText {
                text: String::new(),
                done: true,
            },
        };
        assert!(!super::item_visible(&item));
    }
    #[test]
    fn async_answer_bubble_uses_card_display_and_keeps_exact_bytes() {
        let raw = "Vi\u{202e}olet\nnext";
        let thread = ThreadId::new();
        let card = Item {
            id: ItemId::new(),
            thread,
            turn: TurnId::new(),
            seq: 1,
            body: ItemBody::AsyncQuestions {
                text: String::new(),
                questions: vec![AsyncQuestion {
                    title: "Choose".into(),
                    options: vec![raw.into()],
                    selected: Some(0),
                }],
            },
        };
        let answer = Item {
            id: ItemId::new(),
            thread,
            turn: TurnId::new(),
            seq: 2,
            body: ItemBody::UserText(raw.into()),
        };
        let mut state = super::super::AppState::default();
        state
            .items
            .insert(thread, [(card.id, card)].into_iter().collect());
        assert_eq!(super::user_text(&answer, Some(&state)), "Violet next");
        assert_eq!(answer.body, ItemBody::UserText(raw.into()));
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn queued_wait_words_follow_their_own_bubble(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Waiting(Item);
        impl Render for Waiting {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let t = cx.global::<super::Theme>();
                div()
                    .v_flex()
                    .w(t.layout.transcript_min)
                    .gap(t.space.0[2])
                    .child(super::row(&self.0, false, None, false, cx))
                    .child(super::wait_row("Waiting for the current turn".into(), cx))
            }
        }
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Waiting(Item {
                id: ItemId::new(),
                thread: ThreadId::new(),
                turn: TurnId::new(),
                seq: 1,
                body: ItemBody::UserText("Queued prompt".into()),
            })
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let bubble = cx.debug_bounds("user-bubble").unwrap();
        let wait = cx.debug_bounds("queued-message-wait").unwrap();
        assert!(wait.top() >= bubble.bottom());
        assert_eq!(wait.right(), bubble.right());
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn first_frame_draws_the_end_of_a_tall_reply(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Tall(Entity<super::MessageScrollerState>);
        impl Render for Tall {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let t = cx.global::<super::Theme>();
                div()
                    .w(t.layout.transcript_min)
                    .h(t.text.read.line_height * 8.)
                    .child(
                        list(self.0.read(cx).list.clone(), |_, _, cx| {
                            let t = cx.global::<super::Theme>();
                            div()
                                .v_flex()
                                .children((0..80).map(|line| {
                                    div()
                                        .h(t.text.read.line_height)
                                        .when(line == 79, |line| {
                                            line.debug_selector(|| "first-frame-tail".into())
                                        })
                                        .child(format!("Reply line {line}"))
                                }))
                                .into_any_element()
                        })
                        .size_full(),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Tall(cx.new(|cx| super::MessageScrollerState::new(1, cx)))
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let tail = cx
            .debug_bounds("first-frame-tail")
            .expect("tail must paint in the first draw");
        assert!(
            tail.top() >= gpui::Pixels::ZERO && tail.bottom() <= gpui::Pixels::from(200.),
            "first frame missed the end: {tail:?}"
        );
    }
    #[cfg(debug_assertions)]
    #[test]
    fn terminal_span_dump_keeps_imported_text() {
        let item = super::Item {
            id: super::ItemId::new(),
            thread: super::ThreadId::new(),
            turn: super::TurnId::new(),
            seq: 1,
            body: super::ItemBody::TerminalSpan(super::TerminalSpan {
                text: "window heron\nfake reply: window heron".into(),
            }),
        };
        let dump = super::dump_item(&item);
        assert_eq!(dump["kind"], "terminal_span");
        assert_eq!(dump["text"], "window heron\nfake reply: window heron");
    }
    #[test]
    fn tool_labels_distinguish_veyra_from_provider_names() {
        let command: super::ToolCall = serde_json::from_value(serde_json::json!({
            "name":"Command", "arguments":{}, "veyra_label":true
        }))
        .unwrap();
        assert_eq!(super::tool_label(&command), "Command");
        let provider: super::ToolCall = serde_json::from_value(serde_json::json!({
            "name":"server / tool", "arguments":{}
        }))
        .unwrap();
        assert_eq!(super::tool_label(&provider), "server / tool");
        let stored: super::ToolCall = serde_json::from_value(serde_json::json!({
            "name":"Command", "arguments":{}
        }))
        .unwrap();
        assert_eq!(super::tool_label(&stored), "Command");
    }
    #[test]
    fn async_question_message_keeps_full_markdown_and_paragraphs() {
        let text = format!(
            "{}\n\n**The complete second paragraph ends here.**",
            "A full reply before the question. ".repeat(24)
        );
        let item = super::Item {
            id: super::ItemId::new(),
            thread: super::ThreadId::new(),
            turn: super::TurnId::new(),
            seq: 0,
            body: super::ItemBody::AsyncQuestions {
                text: text.clone(),
                questions: vec![super::AsyncQuestion {
                    title: "Which colour?".into(),
                    options: vec!["Violet".into()],
                    selected: None,
                }],
            },
        };
        assert!(text.chars().count() > 400);
        assert_eq!(super::markdown_text(&item).as_deref(), Some(text.as_str()));
        let assistant = super::Item {
            body: super::ItemBody::AssistantText {
                text: text.clone(),
                done: true,
            },
            ..item.clone()
        };
        assert_eq!(
            super::markdown_text(&item),
            super::markdown_text(&assistant)
        );
        let titles_only = super::Item {
            body: super::ItemBody::AsyncQuestions {
                text: "  Which colour?  ".into(),
                questions: vec![super::AsyncQuestion {
                    title: "Which colour?".into(),
                    options: vec!["Violet".into()],
                    selected: None,
                }],
            },
            ..item
        };
        assert_eq!(super::markdown_text(&titles_only), None);
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn async_question_title_inherits_transcript_row_line_height(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Card(super::Item);
        impl Render for Card {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w_full()
                    .child(super::row(&self.0, false, None, false, cx))
            }
        }
        cx.update(gpui_kit::init);
        let item = super::Item {
            id: super::ItemId::new(),
            thread: super::ThreadId::new(),
            turn: super::TurnId::new(),
            seq: 0,
            body: super::ItemBody::AsyncQuestions {
                text: String::new(),
                questions: vec![super::AsyncQuestion {
                    title: "Which colour?".into(),
                    options: vec!["Violet".into(), "Blue".into()],
                    selected: None,
                }],
            },
        };
        let (card, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Card(item)
        });
        let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
        let line = cx.update(|window, _| theme.text.read.line_height.to_pixels(window.rem_size()));
        for viewport in [theme.layout.window_min, theme.layout.window_default] {
            cx.simulate_resize(viewport);
            for _ in 0..3 {
                cx.run_until_parked();
                cx.draw(
                    Point::default(),
                    size(
                        AvailableSpace::Definite(viewport.width),
                        AvailableSpace::Definite(viewport.height),
                    ),
                    |_, _| div().child(card.clone()),
                );
            }
            for selector in ["async-question-title-label", "async-question-title-value"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert_eq!(
                    bounds.size.height, line,
                    "{viewport:?}: {selector} must inherit the transcript row line height"
                );
            }
        }
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn async_question_message_uses_reply_markdown_layout(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Reply(super::Item);
        impl Render for Reply {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(cx.global::<super::Theme>().layout.transcript_min)
                    .child(
                        div()
                            .debug_selector(|| "reply-content".into())
                            .child(super::row(
                                &self.0,
                                false,
                                super::markdown_text(&self.0),
                                false,
                                cx,
                            )),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let text = format!(
            "{}\n\n**The second paragraph remains complete.**",
            "The full reply has useful detail. ".repeat(24)
        );
        let item = super::Item {
            id: super::ItemId::new(),
            thread: super::ThreadId::new(),
            turn: super::TurnId::new(),
            seq: 0,
            body: super::ItemBody::AssistantText {
                text: text.clone(),
                done: true,
            },
        };
        let (view, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Reply(item)
        });
        let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
        let mut heights = Vec::new();
        for card in [false, true] {
            if card {
                view.update(cx, |view, cx| {
                    view.0.body = super::ItemBody::AsyncQuestions {
                        text: text.clone(),
                        questions: vec![super::AsyncQuestion {
                            title: "Which colour?".into(),
                            options: vec!["Violet".into()],
                            selected: None,
                        }],
                    };
                    cx.notify();
                });
            }
            for _ in 0..3 {
                cx.run_until_parked();
                cx.draw(
                    Point::default(),
                    size(
                        AvailableSpace::Definite(theme.layout.transcript_min),
                        AvailableSpace::Definite(theme.layout.window_default.height),
                    ),
                    |_, _| div().child(view.clone()),
                );
            }
            heights.push(
                cx.debug_bounds(if card {
                    "async-question-message"
                } else {
                    "reply-content"
                })
                .expect("reply rendered above card")
                .size
                .height,
            );
        }
        assert_eq!(
            heights[0], heights[1],
            "async message must use the same complete markdown layout as a reply"
        );
    }
    use super::tool_input;
    use gpui_kit::AppContext;
    use veyra_engine::domain::*;
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn reply_preserves_single_newlines(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Reply(Item);
        impl Render for Reply {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(cx.global::<super::Theme>().layout.transcript_min)
                    .child(
                        div()
                            .debug_selector(|| "reply-lines".into())
                            .child(super::row(
                                &self.0,
                                false,
                                super::markdown_text(&self.0),
                                false,
                                cx,
                            )),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let item = Item {
            id: ItemId::new(),
            thread: ThreadId::new(),
            turn: TurnId::new(),
            seq: 0,
            body: ItemBody::AssistantText {
                text: "first line\n**second line**\nthird line".into(),
                done: true,
            },
        };
        let (view, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(crate::ui::theme::Appearance::Dark, window, cx);
            Reply(item)
        });
        let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
        let line = cx.update(|window, _| theme.text.read.line_height.to_pixels(window.rem_size()));
        for _ in 0..3 {
            cx.run_until_parked();
            cx.draw(
                Point::default(),
                size(
                    AvailableSpace::Definite(theme.layout.transcript_min),
                    AvailableSpace::Definite(theme.layout.window_default.height),
                ),
                |_, _| div().child(view.clone()),
            );
        }
        let height = cx.debug_bounds("reply-lines").unwrap().size.height;
        assert!(
            height >= line * 3.,
            "three provider lines collapsed: {height:?}"
        );
    }
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
                veyra_label: false,
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
        assert_eq!(super::turn_outcome_line(&turn), Some("Done · 42s".into()));
        turn.state = TurnState::Ended {
            outcome: TurnOutcome::Interrupted {
                reason: "Stopped by you".into(),
            },
            started: Some(1000),
            at: 17_000,
        };
        assert_eq!(
            super::turn_outcome_line(&turn),
            Some("Stopped by you · 16s".into())
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
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn failed_tool_preview_stays_one_line_at_narrow_width(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{prelude::*, *};
        struct Preview(Item);
        impl Render for Preview {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(cx.global::<super::Theme>().layout.transcript_min)
                    .child(super::row(&self.0, false, None, false, cx))
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
                    veyra_label: false,
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
    fn inserting_family_card_keeps_the_detached_item(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(super::Theme::new(crate::ui::theme::Appearance::Dark)));
        let state = cx.new(|cx| super::MessageScrollerState::new(3, cx));
        state.update(cx, |state, cx| state.scroll_to_item(1, cx));
        state.update(cx, |state, cx| state.prepend(cx));
        state.read_with(cx, |state, _| {
            assert_eq!(state.list.logical_scroll_top().item_ix, 2);
            assert_eq!(state.item_count(), 4);
            assert!(!state.follow);
        });
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
                    veyra_label: false,
                    name: name.into(),
                    arguments: serde_json::json!({}),
                    input_summary: None,
                },
                output_tail: "still working".into(),
                result: None,
            };
            assert_eq!(super::tool_output(&tool).as_deref(), Some("still working"));
            tool.result = Some(ToolResult {
                ok: true,
                output_tail: Some("finished".into()),
            });
            assert_eq!(super::tool_output(&tool).as_deref(), Some("finished"));
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
        let state = super::super::AppState {
            turns: [(turn.id, turn)].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(super::row_outcome(&items[0], items.get(1), &state), None);
        assert_eq!(
            super::row_outcome(&items[1], items.get(2), &state),
            Some("Done · 2s".into())
        );
    }
    #[test]
    fn failure_caption_defers_to_banner_and_preserves_history() {
        let thread = ThreadId::new();
        let message =
            "Claude is signed out. Sign in from a terminal with `claude`, then send again.";
        let turn = Turn {
            id: TurnId::new(),
            thread,
            ordinal: 1,
            origin: TurnOrigin::Provider,
            state: TurnState::Ended {
                outcome: TurnOutcome::Failed {
                    message: message.into(),
                },
                started: Some(1000),
                at: 3000,
            },
        };
        let item = Item {
            id: ItemId::new(),
            thread,
            turn: turn.id,
            seq: 0,
            body: ItemBody::AssistantText {
                text: "account error".into(),
                done: true,
            },
        };
        let mut state = super::super::AppState::default();
        state.turns.insert(turn.id, turn);
        assert_eq!(state.failure(thread), Some(message));
        assert_eq!(
            super::row_outcome(&item, None, &state),
            Some("Failed".into())
        );
        let later = Turn {
            id: TurnId::new(),
            thread,
            ordinal: 2,
            origin: TurnOrigin::Provider,
            state: TurnState::Running { since: 4000 },
        };
        state.turns.insert(later.id, later);
        assert_eq!(state.failure(thread), None);
        assert_eq!(
            super::row_outcome(&item, None, &state),
            Some(format!("Failed · {message}"))
        );
    }
    #[test]
    fn failed_tool_last_line() {
        let mut tool = ToolItem {
            call: ToolCall {
                veyra_label: false,
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

#[cfg(all(test, debug_assertions))]
mod question_security_tests {
    use super::dump_item;
    use veyra_engine::domain::*;
    #[test]
    fn async_option_display_removes_bidi_but_preserves_sent_text() {
        let option = "Vi\u{202e}olet\nnext";
        let item = Item {
            id: ItemId::new(),
            thread: ThreadId::new(),
            turn: TurnId::new(),
            seq: 1,
            body: ItemBody::AsyncQuestions {
                text: "question".into(),
                questions: vec![AsyncQuestion {
                    title: "Colour\u{200b}".into(),
                    options: vec![option.into()],
                    selected: None,
                }],
            },
        };
        let displayed = dump_item(&item);
        assert_eq!(
            displayed["display_questions"][0]["options"][0],
            "“Violet next”"
        );
        assert_eq!(
            displayed["body"]["AsyncQuestions"]["questions"][0]["options"][0],
            option
        );
    }
}
