use super::*;
use gpui_kit::component::{
    Theme as KitTheme,
    notification::{Notification, NotificationList},
};

fn status_word(status: &Status) -> &str {
    match status {
        Status::NeedsYou => "Needs you",
        _ => status.words(),
    }
}
#[derive(Clone, PartialEq)]
pub(super) struct Attention {
    pub root: ThreadId,
    pub thread: ThreadId,
    pub title: String,
    pub status: Status,
}
#[derive(Clone, PartialEq)]
enum Toast {
    Notice(SharedString),
    Storage(SharedString),
    Attention(Attention),
}
#[derive(Default)]
pub(super) struct Toasts {
    pub blocked: Option<SharedString>,
    pub attention: Vec<Attention>,
    mounted: Vec<Toast>,
    list: Option<Entity<NotificationList>>,
    generation: u64,
    storage_height: std::rc::Rc<std::cell::Cell<Pixels>>,
}
impl Toasts {
    pub fn attention(&mut self, value: Attention) {
        self.attention.retain(|old| old.root != value.root);
        self.attention.push(value);
    }
    pub fn selected(&mut self, root: ThreadId) {
        self.attention.retain(|toast| toast.root != root);
    }
}
fn content(value: &Toast, cx: &App) -> Div {
    let t = cx.global::<Theme>();
    let (title, helper, status, name) = match value {
        Toast::Notice(text) => (
            "Notice".into(),
            veyra_engine::text::display_text(text),
            Status::Idle,
            "notice".into(),
        ),
        Toast::Storage(text) => (
            "Storage blocked".into(),
            veyra_engine::text::display_text(text),
            Status::Failed,
            "storage".into(),
        ),
        Toast::Attention(value) => (
            value.title.clone(),
            status_word(&value.status).to_owned(),
            value.status.clone(),
            value.root.to_string(),
        ),
    };
    let action = match value {
        Toast::Notice(_) | Toast::Storage(_) => {
            composer::labelled_button("copy-notice", "Copy", cx)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(CopyNotice), cx))
        }
        Toast::Attention(value) => {
            let thread = value.thread;
            composer::labelled_button(
                SharedString::from(format!("open-toast-{}", value.root)),
                "Open",
                cx,
            )
            .on_click(move |_, window, cx| {
                window.dispatch_action(
                    Box::new(SelectThread {
                        thread: Some(thread),
                    }),
                    cx,
                )
            })
        }
    };
    let mut context = KeyContext::default();
    context.add("VeyraToast");
    if let Toast::Attention(value) = value {
        context.set("toast_root", value.root.to_string());
    }
    div()
        .h_flex()
        .items_start()
        .gap(t.layout.control_gap)
        .key_context(context)
        .child(composer::observed_part(
            div()
                .flex_none()
                .size(t.layout.icon)
                .child(icons::status_mark(
                    &status,
                    0,
                    t.layout.icon,
                    sidebar::glyph_color(&status, t),
                )),
            format!("toast-mark-{name}"),
        ))
        .child(
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(t.text.ui.size)
                        .line_height(t.text.ui.line_height)
                        .font_weight(t.text.title_weight)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(t.text.label.size)
                        .line_height(t.text.label.line_height)
                        .text_color(t.colors.text_muted)
                        .child(helper),
                ),
        )
        .child(action.flex_none())
}
impl Shell {
    pub(super) fn notice_text(&self) -> Option<&SharedString> {
        self.toasts.blocked.as_ref().or(self.notice.as_ref())
    }
    pub(super) fn dismiss_notice(&mut self) {
        if self.toasts.blocked.is_none() {
            self.notice = None;
        }
    }
    fn toast_rows(&self) -> Vec<Toast> {
        let notice = self.notice_text().map(|text| {
            if self.toasts.blocked.is_some() {
                Toast::Storage(text.clone())
            } else {
                Toast::Notice(text.clone())
            }
        });
        let attention = self.toasts.attention.iter().cloned().map(Toast::Attention);
        if self.toasts.blocked.is_some() {
            notice.into_iter().chain(attention).collect()
        } else {
            attention.chain(notice).collect()
        }
    }
    pub(super) fn sync_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toasts.attention.retain(|toast| {
            self.state
                .threads
                .get(&toast.thread)
                .is_some_and(|row| !row.archived)
        });
        let cap =
            cx.global::<Theme>().layout.toasts_shown - usize::from(self.toasts.blocked.is_some());
        if KitTheme::global(cx).notification.max_items != cap {
            KitTheme::update(cx, |theme| theme.notification.max_items = cap);
        }
        let rows = self.toast_rows();
        if self.toasts.mounted == rows && self.toasts.list.is_some() {
            return;
        }
        self.toasts.generation += 1;
        let generation = self.toasts.generation;
        self.toasts.mounted = rows.clone();
        let list = cx.new(|cx| NotificationList::new(window, cx));
        for (index, toast) in rows.into_iter().enumerate() {
            if matches!(toast, Toast::Storage(_)) {
                continue;
            }
            let close_owner = cx.entity().downgrade();
            let value = toast.clone();
            let t = cx.global::<Theme>();
            let notification = Notification::new()
                .id1::<Toasts>(index)
                .autohide(false)
                .bg(t.colors.floating)
                .p(t.layout.toast_inset)
                .rounded(t.radius.floating)
                .border(t.layout.border)
                .border_color(t.colors.border)
                .shadow(t.shadows.floating.clone())
                .content(move |_, _, cx| content(&value, cx).into_any_element())
                .on_close(move |window, cx| {
                    let _ = close_owner.update(cx, |shell, cx| {
                        if shell.toasts.generation != generation {
                            return;
                        }
                        match &toast {
                            Toast::Notice(_) | Toast::Storage(_) => {
                                window.dispatch_action(Box::new(DismissNotice), cx)
                            }
                            Toast::Attention(value) => window.dispatch_action(
                                Box::new(DismissToast {
                                    root: Some(value.root),
                                }),
                                cx,
                            ),
                        }
                    });
                });
            list.update(cx, |list, cx| list.push(notification, window, cx));
        }
        self.toasts.list = Some(list);
    }
    pub(super) fn render_toasts(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let list = self.toasts.list.as_ref()?;
        let t = cx.global::<Theme>();
        let storage = self.toasts.blocked.as_ref().map(|text| {
            let height = self.toasts.storage_height.clone();
            let owner = cx.entity().downgrade();
            let card = div()
                .id("storage-notice")
                .occlude()
                .absolute()
                .top(t.layout.header + t.layout.toast_inset)
                .right(t.layout.toast_inset)
                .w(t.layout.toast_width)
                .h_flex()
                .p(t.layout.toast_inset)
                .bg(t.colors.floating)
                .border(t.layout.border)
                .border_color(t.colors.border)
                .rounded(t.radius.floating)
                .shadow(t.shadows.floating.clone())
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .overflow_hidden()
                        .child(content(&Toast::Storage(text.clone()), cx)),
                );
            let card = composer::observe_geometry(card, move |bounds, cx| {
                if height.replace(bounds.size.height) != bounds.size.height {
                    let _ = owner.update(cx, |_, cx| cx.notify());
                }
            });
            composer::observed_part(card, "storage-toast")
        });
        let offset = if storage.is_some() {
            self.toasts.storage_height.get() + t.layout.control_gap
        } else {
            Pixels::ZERO
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .child(div().absolute().inset_0().top(offset).child(list.clone()))
                .children(storage)
                .into_any_element(),
        )
    }
    #[cfg(debug_assertions)]
    #[allow(clippy::disallowed_types)] // The driver exposes the notification models as wire JSON.
    pub(super) fn toasts_dump(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.toast_rows()
                .into_iter()
                .map(|toast| {
                    let blocked = matches!(&toast, Toast::Storage(_));
                    match toast {
                        Toast::Notice(text) | Toast::Storage(text) => {
                            serde_json::json!({"kind":"notice","text":text,"storage_blocked":blocked})
                        }
                        Toast::Attention(value) => {
                            serde_json::json!({"kind":"attention","text":value.title,
                                "root":value.root,"thread":value.thread,"status":status_word(&value.status)})
                        }
                    }
                })
                .collect(),
        )
    }
}
