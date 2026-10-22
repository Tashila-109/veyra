#![allow(clippy::disallowed_types)] // Debug drawer state is serialized for the control protocol.
use super::{
    Shell, actions::*, composer::labelled_button, terminal_view::TerminalView, theme::Theme,
};
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
use veyra_engine::{
    commands::Command,
    domain::ThreadId,
    provider::TerminalHandle,
    terminal::{PtySize, SHELL_TAB_LIMIT, ShellStatus},
};

pub(super) struct Tab {
    pub number: u64,
    pub label: String,
    pub handle: Option<TerminalHandle>,
    pub view: Entity<TerminalView>,
    pub error: Option<String>,
}
pub(super) struct Drawer {
    pub open: bool,
    pub height: Pixels,
    pub active: usize,
    pub next: u64,
    pub tabs: Vec<Tab>,
}
pub(super) enum CloseTarget {
    Tab {
        thread: ThreadId,
        number: u64,
    },
    Archive {
        thread: ThreadId,
        keep_children: bool,
        threads: Vec<ThreadId>,
    },
}
pub(super) struct CloseSheet {
    pub target: CloseTarget,
    pub processes: Vec<String>,
    pub drawer_focus: bool,
}
impl Shell {
    pub(super) fn active_shell(&self) -> Option<&Tab> {
        let drawer = self.drawers.get(&self.view.chat()?)?;
        drawer
            .open
            .then(|| drawer.tabs.get(drawer.active))
            .flatten()
    }
    pub(super) fn shell_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.active_shell()
            .is_some_and(|tab| tab.view.focus_handle(cx).is_focused(window))
    }
    pub(super) fn focus_shell(&self, window: &mut Window, cx: &mut App) {
        if let Some(tab) = self.active_shell() {
            window.focus(&tab.view.focus_handle(cx), cx);
        }
    }
    pub(super) fn sync_drawers(&mut self, cx: &mut Context<Self>) {
        let selected = self.view.chat();
        let archived: Vec<_> = self
            .drawers
            .keys()
            .copied()
            .filter(|id| {
                self.state
                    .threads
                    .get(id)
                    .is_none_or(|thread| thread.archived)
            })
            .collect();
        for thread in archived {
            self.engine.close_shells(thread);
            self.drawers.remove(&thread);
        }
        for (thread, drawer) in &self.drawers {
            for (index, tab) in drawer.tabs.iter().enumerate() {
                let visible = selected == Some(*thread) && drawer.open && index == drawer.active;
                tab.view
                    .update(cx, |view, cx| view.set_visible(visible, cx));
            }
        }
    }
    pub(super) fn toggle_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.view.chat() else {
            return;
        };
        if self.drawers.get(&thread).is_some_and(|drawer| drawer.open) {
            self.drawers.get_mut(&thread).unwrap().open = false;
            self.input.update(cx, |input, cx| input.focus(window, cx));
        } else {
            if self
                .drawers
                .get(&thread)
                .is_none_or(|drawer| drawer.tabs.is_empty())
            {
                self.new_shell(window, cx);
            }
            if let Some(drawer) = self.drawers.get_mut(&thread) {
                drawer.open = true;
            }
            if self.input.read(cx).value().is_empty() {
                self.focus_shell(window, cx);
            }
        }
        self.sync_drawers(cx);
        cx.notify();
    }
    pub(super) fn new_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_shell(true, window, cx);
    }
    fn add_shell(&mut self, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.view.chat() else {
            return;
        };
        let Some(row) = self.state.threads.get(&thread) else {
            return;
        };
        if self
            .drawers
            .get(&thread)
            .is_some_and(|drawer| drawer.tabs.len() >= SHELL_TAB_LIMIT)
        {
            return;
        }
        let cwd = row.cwd.clone();
        let theme = cx.global::<Theme>();
        let height =
            theme.space.0[8] + theme.text.terminal.line_height.to_pixels(window.rem_size()) * 12.;
        self.engine
            .set_terminal_colors(theme.terminal.engine_colors());
        let (handle, error) = match self.engine.open_shell(thread, cwd, PtySize::default()) {
            Ok(handle) => (Some(handle), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let view = cx.new(|cx| TerminalView::new(handle.clone(), window, cx));
        let drawer = self.drawers.entry(thread).or_insert_with(|| Drawer {
            open: false,
            height,
            active: 0,
            next: 1,
            tabs: Vec::new(),
        });
        let number = drawer.next;
        drawer.next += 1;
        let shell = veyra_engine::platform::user_shell();
        let name = shell.file_name().unwrap_or_default().to_string_lossy();
        drawer.tabs.push(Tab {
            number,
            label: format!("{name} {number}"),
            handle,
            view,
            error,
        });
        drawer.active = drawer.tabs.len() - 1;
        if drawer.open && focus {
            self.focus_shell(window, cx);
        }
        self.sync_drawers(cx);
        cx.notify();
    }
    pub(super) fn step_shell(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drawer) = self
            .view
            .chat()
            .and_then(|thread| self.drawers.get_mut(&thread))
        else {
            return;
        };
        if drawer.tabs.is_empty() {
            return;
        }
        drawer.active =
            (drawer.active + if forward { 1 } else { drawer.tabs.len() - 1 }) % drawer.tabs.len();
        self.focus_shell(window, cx);
        self.sync_drawers(cx);
        cx.notify();
    }
    fn tab_processes(&self, thread: ThreadId) -> Vec<String> {
        self.drawers
            .get(&thread)
            .into_iter()
            .flat_map(|drawer| &drawer.tabs)
            .flat_map(|tab| {
                tab.handle
                    .iter()
                    .flat_map(|handle| handle.running_processes())
                    .map(|path| {
                        format!(
                            "{}: {}",
                            tab.label,
                            path.file_name().unwrap_or_default().to_string_lossy()
                        )
                    })
            })
            .collect()
    }
    pub(super) fn running_shells(&self) -> Vec<String> {
        self.drawers
            .keys()
            .flat_map(|thread| {
                let title = self
                    .state
                    .threads
                    .get(thread)
                    .map(|row| self.thread_label(row))
                    .unwrap_or_else(|| "Thread".into());
                self.tab_processes(*thread)
                    .into_iter()
                    .map(move |process| format!("{title} · {process}"))
            })
            .collect()
    }
    pub(super) fn ask_close_shell(
        &mut self,
        thread: ThreadId,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self
            .drawers
            .get(&thread)
            .and_then(|drawer| drawer.tabs.iter().find(|tab| tab.number == number))
        else {
            return;
        };
        let processes: Vec<_> = tab
            .handle
            .iter()
            .flat_map(|handle| handle.running_processes())
            .map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        if processes.is_empty() {
            self.close_shell(thread, number, window, cx);
            return;
        }
        self.close_sheet = Some(CloseSheet {
            target: CloseTarget::Tab { thread, number },
            processes,
            drawer_focus: self.shell_has_focus(window, cx),
        });
        window.focus(&self.close_focus, cx);
        cx.notify();
    }
    pub(super) fn close_active_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.view.chat() else {
            return;
        };
        let Some(number) = self.active_shell().map(|tab| tab.number) else {
            return;
        };
        self.ask_close_shell(thread, number, window, cx);
    }
    fn close_shell(
        &mut self,
        thread: ThreadId,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(drawer) = self.drawers.get_mut(&thread)
            && let Some(index) = drawer.tabs.iter().position(|tab| tab.number == number)
        {
            let tab = drawer.tabs.remove(index);
            if let Some(handle) = tab.handle {
                self.engine.close_shell(&handle);
            }
            drawer.active = drawer.active.min(drawer.tabs.len().saturating_sub(1));
            if drawer.tabs.is_empty() {
                drawer.open = false;
                self.input.update(cx, |input, cx| input.focus(window, cx));
            } else {
                self.focus_shell(window, cx);
            }
        }
        self.sync_drawers(cx);
        cx.notify();
    }
    pub(super) fn confirm_close_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.close_sheet.take() else {
            return;
        };
        match sheet.target {
            CloseTarget::Tab { thread, number } => self.close_shell(thread, number, window, cx),
            CloseTarget::Archive {
                thread,
                keep_children,
                threads,
            } => {
                for id in threads {
                    self.engine.close_shells(id);
                    self.drawers.remove(&id);
                }
                self.send_command(
                    Command::ArchiveThread {
                        thread,
                        keep_children,
                    },
                    window,
                    cx,
                );
            }
        }
    }
    pub(super) fn cancel_close_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sheet) = self.close_sheet.take() {
            if sheet.drawer_focus {
                self.focus_shell(window, cx);
            } else {
                self.input.update(cx, |input, cx| input.focus(window, cx));
            }
            cx.notify();
        }
    }
    pub(super) fn archive_shells(
        &mut self,
        thread: ThreadId,
        keep_children: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let threads: Vec<_> = self
            .state
            .threads
            .values()
            .filter(|row| row.id == thread || !keep_children && row.root() == thread)
            .map(|row| row.id)
            .collect();
        let processes: Vec<_> = threads
            .iter()
            .flat_map(|id| self.tab_processes(*id))
            .collect();
        if processes.is_empty() {
            for id in threads {
                self.engine.close_shells(id);
                self.drawers.remove(&id);
            }
            false
        } else {
            self.close_sheet = Some(CloseSheet {
                target: CloseTarget::Archive {
                    thread,
                    keep_children,
                    threads,
                },
                processes,
                drawer_focus: self.shell_has_focus(window, cx),
            });
            window.focus(&self.close_focus, cx);
            cx.notify();
            true
        }
    }
    pub(super) fn restart_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.view.chat() else {
            return;
        };
        let focused = self.shell_has_focus(window, cx);
        let Some(tab) = self.active_shell() else {
            return;
        };
        if tab
            .handle
            .as_ref()
            .is_some_and(|handle| !handle.status().is_finished())
        {
            return;
        }
        let number = tab.number;
        let drawer = self.drawers.get_mut(&thread).unwrap();
        let index = drawer.active;
        if let Some(handle) = drawer.tabs.remove(index).handle {
            self.engine.close_shell(&handle);
        }
        self.add_shell(focused, window, cx);
        let drawer = self.drawers.get_mut(&thread).unwrap();
        let mut tab = drawer.tabs.pop().unwrap();
        tab.number = number;
        tab.label = format!(
            "{} {number}",
            veyra_engine::platform::user_shell()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        drawer.tabs.insert(index, tab);
        drawer.active = index;
        if focused {
            self.focus_shell(window, cx);
        } else {
            self.input.update(cx, |input, cx| input.focus(window, cx));
        }
        self.sync_drawers(cx);
    }
    pub(super) fn drawer_height_bounds(&self, window: &Window, cx: &App) -> (Pixels, Pixels) {
        let theme = cx.global::<Theme>();
        let min =
            theme.space.0[8] + theme.text.terminal.line_height.to_pixels(window.rem_size()) * 6.;
        let max = (window.viewport_size().height
            - theme.layout.header
            - self.composition_height
            - theme.text.read.line_height.to_pixels(window.rem_size()) * 3.)
            .max(min);
        (min, max)
    }
    pub(super) fn render_drawer(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        let thread = self.view.chat()?;
        let drawer = self.drawers.get(&thread).filter(|drawer| drawer.open)?;
        let theme = cx.global::<Theme>();
        let tabs_height = theme.space.0[8];
        let mut tabs = div()
            .h_flex()
            .h(tabs_height)
            .flex_none()
            .gap(theme.space.0[1]);
        for (index, tab) in drawer.tabs.iter().enumerate() {
            let number = tab.number;
            tabs = tabs.child(
                div()
                    .h_flex()
                    .min_w(theme.text.read.line_height.to_pixels(window.rem_size()) * 3.)
                    .bg(if drawer.active == index {
                        theme.colors.card
                    } else {
                        theme.colors.bg
                    })
                    .child(
                        labelled_button(("shell-tab", number), tab.label.clone(), cx).on_click(
                            cx.listener(move |shell, _, window, cx| {
                                shell.drawers.get_mut(&thread).unwrap().active = index;
                                shell.focus_shell(window, cx);
                                shell.sync_drawers(cx);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(labelled_button(("close-shell", number), "×", cx).on_click(
                        cx.listener(move |shell, _, window, cx| {
                            shell.ask_close_shell(thread, number, window, cx)
                        }),
                    )),
            );
        }
        tabs = tabs.child(
            labelled_button("new-shell-tab", "+", cx)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewShellTab), cx)),
        );
        let active = drawer.tabs.get(drawer.active)?;
        let message = active.error.clone().or_else(|| {
            active
                .handle
                .as_ref()
                .and_then(|handle| match handle.status() {
                    ShellStatus::Exited { code } => Some(format!(
                        "Shell exited with code {code}. Press Return to restart."
                    )),
                    ShellStatus::Closed => Some("Shell closed. Press Return to restart.".into()),
                    ShellStatus::Signalled => {
                        Some("Shell ended after a signal. Press Return to restart.".into())
                    }
                    ShellStatus::Failed { error } => {
                        Some(format!("{error}. Press Return to restart."))
                    }
                    _ => None,
                })
        });
        let (_, max) = self.drawer_height_bounds(window, cx);
        let rail = div()
            .id("drawer-resize")
            .h(theme.space.0[1])
            .w_full()
            .cursor(CursorStyle::ResizeUpDown)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |shell, event: &MouseDownEvent, _, cx| {
                    shell.drawer_resizing =
                        Some((thread, event.position.y, shell.drawers[&thread].height));
                    cx.stop_propagation();
                }),
            );
        #[cfg(debug_assertions)]
        let rail = rail.relative().child(
            canvas(
                {
                    let shell = cx.entity().downgrade();
                    move |bounds, _, cx| {
                        let _ =
                            shell.update(cx, |shell, _| shell.drawer_rail = Some((thread, bounds)));
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
        Some(
            div()
                .v_flex()
                .h(drawer.height.min(max))
                .flex_none()
                .min_h_0()
                .w_full()
                .debug_selector(|| "shell-drawer".into())
                .border_t(theme.layout.border)
                .border_color(theme.colors.border)
                .child(rail)
                .child(tabs)
                .children(
                    message.map(|message| div().text_color(theme.colors.text_muted).child(message)),
                )
                .child(active.view.clone())
                .into_any_element(),
        )
    }
    pub(super) fn render_shell_close_sheet(&self, cx: &App) -> Option<AnyElement> {
        let sheet = self.close_sheet.as_ref()?;
        let theme = cx.global::<Theme>();
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .track_focus(&self.close_focus)
                .key_context("VeyraShellClose")
                .child(
                    div()
                        .v_flex()
                        .w(theme.layout.transcript_min)
                        .p(theme.space.0[7])
                        .gap(theme.space.0[5])
                        .rounded(theme.radius.large)
                        .bg(theme.colors.card)
                        .border(theme.layout.border)
                        .border_color(theme.colors.border_strong)
                        .child("Stop these commands?")
                        .children(sheet.processes.iter().map(|name| div().child(name.clone())))
                        .child(
                            div()
                                .h_flex()
                                .justify_end()
                                .gap(theme.space.0[3])
                                .child(
                                    labelled_button("cancel-close-shell", "Cancel", cx).on_click(
                                        |_, window, cx| {
                                            window
                                                .dispatch_action(Box::new(CancelCloseShellTab), cx)
                                        },
                                    ),
                                )
                                .child(
                                    labelled_button("confirm-close-shell", "Close Tab", cx)
                                        .on_click(|_, window, cx| {
                                            window
                                                .dispatch_action(Box::new(ConfirmCloseShellTab), cx)
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
    #[cfg(debug_assertions)]
    pub(super) fn drawer_dump(&self, cx: &App) -> serde_json::Value {
        let Some(thread) = self.view.chat() else {
            return serde_json::Value::Null;
        };
        let Some(drawer) = self.drawers.get(&thread) else {
            return serde_json::json!({"open":false,"tabs":[]});
        };
        serde_json::json!({"open":drawer.open,"height":f32::from(drawer.height),
            "active_tab":drawer.tabs.get(drawer.active).map(|tab|tab.number),
            "tabs":drawer.tabs.iter().map(|tab| {
                let counts = tab.view.read(cx).frame_counts();
                let status = tab.handle.as_ref().map(TerminalHandle::status);
                let exit_code = match status { Some(ShellStatus::Exited { code }) => Some(code), _ => None };
                serde_json::json!({"number":tab.number,"label":tab.label,
                    "frame_requests":counts.0,"repaints":counts.1,
                    "pid":tab.handle.as_ref().and_then(TerminalHandle::pid),
                    "running":tab.handle.as_ref().map(TerminalHandle::running_processes).unwrap_or_default(),
                    "exit_code":exit_code,
                    "exited":status.as_ref().is_some_and(ShellStatus::is_finished) || tab.error.is_some(),
                    "error":tab.error.clone().or_else(|| match &status {
                        Some(ShellStatus::Failed { error }) => Some(error.clone()), _ => None })})
            }).collect::<Vec<_>>(),"terminal":self.active_shell().map(|tab|tab.view.read(cx).dump()),
            "close_sheet":self.close_sheet.as_ref().map(|sheet|&sheet.processes)})
    }
}
