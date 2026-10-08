pub mod actions;
mod new_thread;
mod sidebar;
pub mod theme;

use actions::*;
use gpui_kit::component::StyledExt;
use gpui_kit::*;
use std::path::PathBuf;
use theme::{Appearance, Theme};

pub struct Shell {
    focus: FocusHandle,
    sidebar_width: Pixels,
    resizing: bool,
    notice: Option<SharedString>,
}

impl Shell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            sidebar_width: cx.global::<Theme>().layout.sidebar_default,
            resizing: false,
            notice: None,
        }
    }

    fn notice(&mut self, message: &'static str, cx: &mut Context<Self>) {
        tracing::info!(message, "shell action");
        self.notice = Some(message.into());
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dump(&self, data_dir: &std::path::Path, cx: &App) -> serde_json::Value {
        serde_json::json!({
            "pid": std::process::id(), "build_rev": env!("VEYRA_BUILD_REV"),
            "data_dir": data_dir, "appearance": cx.global::<Theme>().appearance.name(),
            "view": "new_thread", "sidebar": { "width": f32::from(self.sidebar_width), "rows": [] },
            "composer_text": "", "notice": self.notice.as_ref().map(|s| s.as_ref())
        })
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>().clone();
        div()
            .h_flex()
            .size_full()
            .bg(t.colors.bg)
            .text_color(t.colors.text)
            .font_family(t.text.ui_font.clone())
            .text_size(t.text.ui.size)
            .line_height(t.text.ui.line_height)
            .track_focus(&self.focus)
            .key_context("Veyra")
            .on_action(cx.listener(|s, _: &AddProject, _, cx| {
                s.notice("Adding projects is not available in M0.", cx)
            }))
            .on_action(cx.listener(|s, _: &NewThread, _, cx| {
                s.notice = None;
                cx.notify();
            }))
            .on_action(cx.listener(|s, _: &PickProvider, _, cx| {
                s.notice("Provider selection is not available in M0.", cx)
            }))
            .on_action(cx.listener(|s, _: &PickModel, _, cx| {
                s.notice("Model selection is not available in M0.", cx)
            }))
            .on_action(cx.listener(|s, _: &PickEffort, _, cx| {
                s.notice("Effort selection is not available in M0.", cx)
            }))
            .on_action(cx.listener(|s, _: &PickPermission, _, cx| {
                s.notice("Permission selection is not available in M0.", cx)
            }))
            .on_action(cx.listener(|s, _: &Send, _, cx| {
                s.notice("The M0 composer is a shell. Sending is not available.", cx)
            }))
            .on_action(cx.listener(|s, _: &DismissNotice, _, cx| {
                s.notice = None;
                cx.notify();
            }))
            .on_action(cx.listener(|s, a: &SetSidebarWidth, _, cx| {
                let layout = &cx.global::<Theme>().layout;
                s.sidebar_width = px(a.width).clamp(layout.sidebar_min, layout.sidebar_max);
                cx.notify();
            }))
            .on_action(cx.listener(|s, _: &NarrowSidebar, w, cx| {
                w.dispatch_action(
                    Box::new(SetSidebarWidth {
                        width: f32::from(
                            s.sidebar_width - cx.global::<Theme>().layout.sidebar_step,
                        ),
                    }),
                    cx,
                );
            }))
            .on_action(cx.listener(|s, _: &WidenSidebar, w, cx| {
                w.dispatch_action(
                    Box::new(SetSidebarWidth {
                        width: f32::from(
                            s.sidebar_width + cx.global::<Theme>().layout.sidebar_step,
                        ),
                    }),
                    cx,
                );
            }))
            .on_action(cx.listener(|_, _: &ResetSidebar, w, cx| {
                w.dispatch_action(
                    Box::new(SetSidebarWidth {
                        width: f32::from(cx.global::<Theme>().layout.sidebar_default),
                    }),
                    cx,
                );
            }))
            .on_mouse_move(cx.listener(|s, e: &MouseMoveEvent, w, cx| {
                if s.resizing && e.pressed_button == Some(MouseButton::Left) {
                    w.dispatch_action(
                        Box::new(SetSidebarWidth {
                            width: f32::from(e.position.x),
                        }),
                        cx,
                    );
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.resizing = false),
            )
            .child(sidebar::render(self.sidebar_width, cx))
            .child(
                div()
                    .id("sidebar-resize")
                    .w(t.layout.border)
                    .h_full()
                    .flex_none()
                    .bg(t.colors.border)
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|s, _, _, cx| {
                            s.resizing = true;
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(new_thread::render(self.notice.clone(), cx))
    }
}

pub fn run(
    data_dir: PathBuf,
    #[cfg(debug_assertions)] requests: tokio::sync::mpsc::Receiver<crate::control::Request>,
) {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::register_fonts(cx).expect("bundled Geist fonts");
            actions::init(cx);
            let layout = Theme::new(Appearance::Dark).layout;
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    layout.window_default,
                    cx,
                ))),
                window_min_size: Some(layout.window_min),
                titlebar: Some(TitlebarOptions {
                    title: Some("Veyra".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(
                        layout.traffic_lights.width,
                        layout.traffic_lights.height,
                    )),
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                Theme::apply(Appearance::from_window(window.appearance()), window, cx);
                window
                    .observe_window_appearance(|window, cx| {
                        Theme::apply(Appearance::from_window(window.appearance()), window, cx);
                    })
                    .detach();
                let shell = cx.new(|cx| Shell::new(window, cx));
                #[cfg(debug_assertions)]
                crate::control::attach(&shell, requests, data_dir.clone(), window, cx);
                window.on_next_frame(move |_, _| {
                    let line = format!(
                        "veyra ready pid={} data_dir={}",
                        std::process::id(),
                        data_dir.display()
                    );
                    eprintln!("{line}");
                    tracing::info!("{line}");
                });
                shell
            })
            .expect("open Veyra window");
            cx.activate(true);
        });
}
