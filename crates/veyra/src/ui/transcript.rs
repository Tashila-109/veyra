use super::{
    Shell,
    actions::*,
    composer::{self, labelled_button},
    theme::Theme,
};
use gpui_kit::component::StyledExt;
use gpui_kit::component::{message_scroller::MessageScroller, text::TextView};
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::*;
fn row(item: &Item, expanded: bool, cx: &App) -> AnyElement {
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
        ItemBody::AssistantText { text, done } => {
            row = row
                .child(TextView::markdown(
                    SharedString::from(format!("assistant-{}", item.id)),
                    text.clone(),
                ))
                .when(!done, |row| {
                    row.child(
                        div()
                            .w(t.motion.stream_caret.width)
                            .h(t.motion.stream_caret.height)
                            .bg(t.colors.accent),
                    )
                })
        }
        ItemBody::Reasoning { text, .. } => {
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
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(ToggleItem { item: Some(id) }), cx)
                    }),
                )
                .when(expanded, |row| {
                    row.child(TextView::markdown(
                        SharedString::from(format!("reasoning-text-{}", item.id)),
                        text.clone(),
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
                    labelled_button(SharedString::from(format!("tool-{id}")), label, cx).on_click(
                        move |_, window, cx| {
                            window.dispatch_action(Box::new(ToggleItem { item: Some(id) }), cx)
                        },
                    ),
                )
                .when(expanded, |row| {
                    row.child(
                        div()
                            .font_family(t.text.mono_font.clone())
                            .text_size(t.text.code.size)
                            .line_height(t.text.code.line_height)
                            .child(tool.call.arguments.to_string()),
                    )
                    .when_some(
                        tool.result
                            .as_ref()
                            .and_then(|result| result.output_tail.clone()),
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
pub fn render(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let items = shell
        .view
        .chat()
        .map(|thread| shell.state.transcript(thread))
        .unwrap_or_default();
    let expanded = shell.expanded.clone();
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
                .child(
                    MessageScroller::new(
                        "transcript",
                        shell.scroller.clone(),
                        move |index, _, cx| {
                            items
                                .get(index)
                                .map(|item| row(item, expanded.contains(&item.id), cx))
                                .unwrap_or_else(|| div().into_any_element())
                        },
                    )
                    .size_full()
                    .with_jump_button_transition(t.motion.menu[0].duration),
                ),
        )
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
            tool.result
                .as_ref()
                .and_then(|result| result.output_tail.clone())
                .unwrap_or_default(),
            tool.result.is_some(),
        ),
        ItemBody::Notice(notice) => ("notice", notice.text.clone(), true),
        _ => ("other", String::new(), true),
    };
    serde_json::json!({"id":item.id,"turn":item.turn,"kind":kind,"text":text,"done":done,"body":item.body})
}
