use super::*;
#[cfg(debug_assertions)]
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::{
    Icon,
    command::{Command as KitCommand, CommandGroup, CommandItem, CommandState},
};

pub(super) struct Palette {
    state: Entity<CommandState>,
    origin: FocusHandle,
    contexts: Vec<KeyContext>,
    closing: bool,
}

#[derive(Clone)]
enum Target {
    Action(&'static str),
    Thread(ThreadId),
}
#[derive(Clone)]
struct Entry {
    label: SharedString,
    target: Target,
    icon: Icon,
    chord: Option<String>,
}
fn chord(binding: &Binding) -> Option<String> {
    actions::format_chord(binding.chord)
}

impl Shell {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        let origin = window.focused(cx).unwrap_or_else(|| self.focus.clone());
        let contexts = window.context_stack();
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |state, cx| state.focus(window, cx));
        self.palette = Some(Palette {
            state,
            origin,
            contexts,
            closing: false,
        });
        cx.notify();
    }
    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        window.focus(&palette.origin, cx);
        // gpui emits focus callbacks after drawing; actions need the restored state now.
        self.dock_focused = self.dock_focus.contains_focused(window, cx);
        self.composer_focused = self.input.focus_handle(cx).is_focused(window);
        if palette.closing {
            return;
        }
        let duration = cx.global::<Theme>().motion.menu[1].duration;
        if duration.is_zero() {
            self.palette = None;
        } else {
            palette.closing = true;
            let identity = palette.state.entity_id();
            cx.spawn_in(window, async move |owner, cx| {
                cx.background_executor().timer(duration).await;
                let _ = owner.update(cx, |shell, cx| {
                    if shell.palette.as_ref().is_some_and(|palette| {
                        palette.closing && palette.state.entity_id() == identity
                    }) {
                        shell.palette = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }
    fn palette_entries(&self, cx: &App) -> (Vec<Entry>, Vec<Entry>) {
        let Some(palette) = &self.palette else {
            return Default::default();
        };
        let query = palette.state.read(cx).query(cx);
        let query = query.trim();
        let actions_only = query.starts_with('>');
        let query = query
            .strip_prefix('>')
            .unwrap_or(query)
            .trim()
            .to_lowercase();
        let matches = |label: &str| label.to_lowercase().contains(&query);
        let actions = bindings()
            .into_iter()
            .filter_map(|binding| {
                let label = binding.palette_label()?;
                if !binding.matches_context(&palette.contexts) || !matches(label) {
                    return None;
                }
                Some(Entry {
                    label: label.into(),
                    target: Target::Action(binding.action),
                    icon: Icon::new(gpui_kit::assets::IconName::Command),
                    chord: chord(&binding),
                })
            })
            .collect();
        let threads = if actions_only {
            Vec::new()
        } else {
            sidebar::ordered_threads(&self.state)
                .into_iter()
                .filter_map(|id| {
                    let thread = self.state.threads.get(&id)?;
                    if thread.archived {
                        return None;
                    }
                    let label = match match thread.lineage {
                        Lineage::Child { root, .. } => self.state.threads.get(&root),
                        Lineage::Root => None,
                    } {
                        Some(root) => format!("{} / {}", root.title, thread.title),
                        None => thread.title.clone(),
                    };
                    if !matches(&label) {
                        return None;
                    }
                    Some(Entry {
                        label: label.into(),
                        target: Target::Thread(id),
                        icon: Icon::default().path(icons::status_path(&self.state.status(id))),
                        chord: None,
                    })
                })
                .collect()
        };
        (actions, threads)
    }
    fn run_palette_entry(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = &self.palette else { return };
        if palette.closing {
            return;
        }
        let contexts = palette.contexts.clone();
        self.close_palette(window, cx);
        match entry.target {
            Target::Action(name) => {
                if let Some(binding) = bindings()
                    .into_iter()
                    .find(|binding| binding.action == name)
                    && binding.palette_label().is_some()
                    && binding.matches_context(&contexts)
                    && binding.matches_context(&window.context_stack())
                {
                    window.dispatch_action((binding.build)(), cx);
                }
            }
            Target::Thread(thread) => {
                if self
                    .state
                    .threads
                    .get(&thread)
                    .is_some_and(|row| !row.archived)
                {
                    window.dispatch_action(
                        Box::new(SelectThread {
                            thread: Some(thread),
                        }),
                        cx,
                    );
                }
            }
        }
        cx.notify();
    }
    pub(super) fn render_palette(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let palette = self.palette.as_ref()?;
        let t = cx.global::<Theme>();
        let closing = palette.closing;
        let timing = if closing {
            &t.motion.menu[1]
        } else {
            &t.motion.menu[0]
        };
        let duration = timing.duration;
        let [a, b, c, d] = timing.curve;
        let (actions, threads) = self.palette_entries(cx);
        let entries = [actions.clone(), threads.clone()];
        let row = |entry: Entry| {
            let label = entry.label.clone();
            CommandItem::new().label(label).child(move |_, cx| {
                let t = cx.global::<Theme>();
                let content = div()
                    .h_flex()
                    .items_center()
                    .h(t.layout.thread_row)
                    .pl(t.layout.row_inset)
                    .pr(t.layout.row_inset)
                    .gap(t.layout.control_gap)
                    .child(
                        entry
                            .icon
                            .clone()
                            .size(t.layout.icon)
                            .text_color(t.colors.icon),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(t.text.ui.size)
                            .child(entry.label.clone()),
                    )
                    .children(entry.chord.clone().map(|chord| {
                        div()
                            .text_size(t.text.micro.size)
                            .line_height(t.text.micro.line_height)
                            .text_color(t.colors.text_muted)
                            .child(chord)
                    }));
                #[cfg(debug_assertions)]
                let content = {
                    let selector = format!("palette-row-{}", entry.label);
                    let observed = selector.clone();
                    content
                        .debug_selector(move || selector.clone())
                        .on_prepaint(move |bounds, window, cx| {
                            let visible = bounds.intersect(&window.content_mask().bounds);
                            if visible.size.width > gpui::Pixels::ZERO
                                && visible.size.height > gpui::Pixels::ZERO
                            {
                                cx.global_mut::<PaintedControls>().0.insert(
                                    format!("{:?}", ElementId::Name(observed.clone().into())),
                                    visible,
                                );
                            }
                        })
                };
                content
            })
        };
        let run = cx.entity().downgrade();
        let query = run.clone();
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .pl(self.sidebar_width + t.layout.border)
                .pt(t.layout.header + t.layout.section_gap)
                .flex()
                .justify_center()
                .items_start()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|shell, _, window, cx| shell.close_palette(window, cx)),
                )
                .child(
                    div()
                        .id("command-palette")
                        .debug_selector(|| "palette-card".into())
                        .occlude()
                        .w(t.layout.palette_width)
                        .max_w_full()
                        .max_h(t.layout.palette_max_height)
                        .p(t.layout.floating_inset)
                        .rounded(t.radius.floating)
                        .bg(t.colors.floating)
                        .shadow(t.shadows.floating.clone())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_action(cx.listener(
                            |shell, _: &gpui_kit::base::actions::Cancel, window, cx| {
                                shell.close_palette(window, cx);
                                cx.stop_propagation();
                            },
                        ))
                        .child(
                            KitCommand::new(&palette.state)
                                .bordered(false)
                                .filterable(false)
                                .max_h(t.layout.palette_max_height - t.layout.floating_inset * 2.)
                                .map(|command| {
                                    // Command::max_h caps the list only; bound its search/list flex root too.
                                    gpui::Styled::max_h(
                                        command,
                                        t.layout.palette_max_height - t.layout.floating_inset * 2.,
                                    )
                                })
                                .placeholder("Search actions and threads")
                                .group(
                                    CommandGroup::new()
                                        .label("Actions")
                                        .items(actions.into_iter().map(row)),
                                )
                                .group(
                                    CommandGroup::new()
                                        .label("Threads")
                                        .items(threads.into_iter().map(row)),
                                )
                                .on_query(move |_, _, cx| {
                                    let _ = query.update(cx, |_, cx| cx.notify());
                                })
                                .on_confirm(move |index, window, cx| {
                                    if let Some(entry) = entries
                                        .get(index.section)
                                        .and_then(|group| group.get(index.row))
                                        .cloned()
                                    {
                                        let _ = run.update(cx, |shell, cx| {
                                            shell.run_palette_entry(entry, window, cx)
                                        });
                                    }
                                }),
                        )
                        .with_animation(
                            ElementId::NamedInteger("palette-transition".into(), closing as u64),
                            Animation::new(duration)
                                .with_easing(gpui_kit::base::animation::cubic_bezier(a, b, c, d)),
                            move |panel, progress| {
                                panel.opacity(if closing { 1. - progress } else { progress })
                            },
                        ),
                )
                .into_any_element(),
        )
    }
    #[cfg(debug_assertions)]
    #[allow(clippy::disallowed_types)] // The driver exposes the rendered palette model as wire JSON.
    pub(super) fn palette_dump(&self, cx: &App) -> serde_json::Value {
        let (actions, threads) = self.palette_entries(cx);
        serde_json::json!({"open":self.palette.is_some(),
            "query":self.palette.as_ref().map(|p|p.state.read(cx).query(cx)),
            "actions":actions.iter().filter_map(|entry| match entry.target {
                Target::Action(name) => Some(serde_json::json!({
                    "action":name,"label":entry.label,"chord":entry.chord
                })),
                _ => None,
            }).collect::<Vec<_>>(),
            "threads":threads.iter().filter_map(|entry| match entry.target {
                Target::Thread(thread) => Some(serde_json::json!({"thread":thread,"title":entry.label})),
                _ => None,
            }).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::{bindings, chord};
    #[test]
    fn palette_chords_keep_the_shared_shifted_paste_spelling() {
        let mut binding = bindings()
            .into_iter()
            .find(|row| row.action == "PasteShell")
            .unwrap();
        binding.chord = "secondary-shift-v";
        assert_eq!(chord(&binding), Some("⌘⇧V".into()));
    }
}
