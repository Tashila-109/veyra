use super::{
    MainView, Shell, actions::NewThread, composer::labelled_button, status_name, theme::Theme,
};
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
use veyra_engine::domain::Status;
pub fn render(shell: &Shell, cx: &App) -> impl IntoElement {
    let t = cx.global::<Theme>();
    let title = match shell.view {
        MainView::Settings => "Settings".to_owned(),
        MainView::NewThread => "New thread".to_owned(),
        MainView::Chat => shell
            .selected
            .and_then(|id| shell.state.threads.get(&id))
            .map(|thread| thread.title.clone())
            .unwrap_or_default(),
    };
    let status = shell
        .selected
        .filter(|_| shell.view == MainView::Chat)
        .map(|thread| shell.state.status(thread));
    let text = status.as_ref().map(|status| match status {
        Status::Working { since } => format!(
            "Working · {}s",
            if *since > 0 {
                (veyra_engine::actor::now() - since).max(0) / 1000
            } else {
                0
            }
        ),
        Status::Done { took } => format!("Done · {}s", took / 1000),
        _ => status_name(status).into(),
    });
    let project = shell
        .selected
        .filter(|_| shell.view == MainView::Chat)
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
        .child(div().overflow_hidden().child(title))
        .when_some(text, |this, text| {
            this.child(
                div()
                    .text_size(t.text.meta.size)
                    .text_color(t.colors.text_muted)
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
        .child(
            labelled_button("new-thread", "New thread  ⌘N", cx)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewThread), cx)),
        )
}
