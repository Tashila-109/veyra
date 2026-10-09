use super::{Shell, actions::*, composer::labelled_button, theme::Theme};
use gpui_kit::base::Selectable;
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::Status;
pub fn glyph(status: &Status) -> &'static str {
    match status {
        Status::Working { .. } => "◔",
        Status::Failed => "×",
        Status::Done { .. } => "✓",
        Status::NeedsYou => "◇",
        _ => "·",
    }
}
pub fn render(shell: &Shell, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let mut rows = Vec::new();
    for project in shell.state.projects.values() {
        let id = project.id.to_string();
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
                window.dispatch_action(
                    Box::new(SelectProject {
                        project: Some(id.clone()),
                    }),
                    cx,
                )
            })
            .into_any_element(),
        );
        for thread in shell
            .state
            .threads
            .values()
            .filter(|thread| thread.project == project.id && !thread.archived)
        {
            let id = thread.id.to_string();
            let status = shell.state.status(thread.id);
            rows.push(
                div()
                    .h_flex()
                    .items_center()
                    .h(t.layout.thread_row)
                    .gap(t.space.0[3])
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
                        .selected(
                            shell.selected == Some(thread.id)
                                && shell.view == super::MainView::Chat,
                        )
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(
                                Box::new(SelectThread {
                                    thread: Some(id.clone()),
                                }),
                                cx,
                            )
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
