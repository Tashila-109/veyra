//! Renderer checks. Imported by the test target, never by a production build.
#![cfg(debug_assertions)]
#![allow(clippy::disallowed_types)] // Checks inspect debug dumps and provider wire JSON.
#![allow(dead_code)] // Only the separate main-thread target calls these checks, not the unit-test harness.

#[allow(unused_imports)] // The ordinary unit-test binary does not call this runner.
pub(crate) use checks::run;

// Keep this boundary: composer's dock scan reads each source only up to its first test module.
#[cfg(test)]
mod checks {
    use super::super::*;
    use ::gpui::{AnyWindowHandle, HeadlessAppContext};
    use std::{sync::Arc, time::Duration};

    thread_local! {
        // The runner calls us on its main thread. No native window or application
        // event loop is created; only this text system is given to TestPlatform.
        static TEXT: Arc<dyn gpui::PlatformTextSystem> =
            gpui_platform::current_platform(true).text_system();
    }

    fn context() -> HeadlessAppContext {
        let mut cx = HeadlessAppContext::with_platform(
            TEXT.with(Clone::clone),
            Arc::new(icons::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
            theme::register_fonts(cx).unwrap();
            let look = theme::Look {
                requested: WindowBackground::Solid,
                options: veyra_engine::platform::DisplayOptions {
                    reduce_motion: true,
                    ..Default::default()
                },
                overridden: true,
            };
            cx.set_global(look);
        });
        cx
    }

    struct Frame {
        width: usize,
        height: usize,
        scale: f32,
        rgba: Vec<u8>,
    }
    impl Frame {
        fn capture(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Self {
            let scale = cx
                .update_window(window, |_, w, _| w.scale_factor())
                .unwrap();
            let image = cx
                .capture_screenshot(window)
                .expect("headless Metal readback");
            Self {
                width: image.width() as usize,
                height: image.height() as usize,
                scale,
                rgba: image.into_raw(),
            }
        }
        fn region(&self, bounds: Bounds<Pixels>) -> Vec<[u8; 3]> {
            let x = |v: Pixels| (f32::from(v) * self.scale).round() as usize;
            let (left, right, top, bottom) = (
                x(bounds.left()),
                x(bounds.right()),
                x(bounds.top()),
                x(bounds.bottom()),
            );
            assert!(left < right && top < bottom && right <= self.width && bottom <= self.height);
            (top..bottom)
                .flat_map(|y| (left..right).map(move |x| y * self.width + x))
                .map(|i| self.rgba[i * 4..i * 4 + 3].try_into().unwrap())
                .collect()
        }
        fn color_pixels(&self, bounds: Bounds<Pixels>, color: Hsla) -> usize {
            let rgba = Rgba::from(color);
            let expected = [rgba.r, rgba.g, rgba.b].map(|v| (v * 255.).round() as u8);
            self.region(bounds)
                .iter()
                .filter(|p| p.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 5))
                .count()
        }
        fn shape(&self, bounds: Bounds<Pixels>) -> Vec<bool> {
            let pixels = self.region(bounds);
            let bg = pixels[0];
            let contrasts: Vec<_> = pixels
                .iter()
                .map(|p| p.iter().zip(bg).map(|(a, b)| a.abs_diff(b)).max().unwrap())
                .collect();
            let peak = *contrasts.iter().max().unwrap();
            assert!(peak > 24, "mark has no foreground contrast");
            let shape: Vec<_> = contrasts.into_iter().map(|v| v > peak / 2).collect();
            let ink = shape.iter().filter(|v| **v).count();
            assert!(
                ink > 3 && ink < shape.len() * 3 / 4,
                "empty/contaminated mark"
            );
            shape
        }
    }

    fn draw(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    enum MarkGlyph {
        Status { status: Status, phase: u8 },
        Asset(&'static str),
    }
    struct Mark {
        glyph: MarkGlyph,
        background: Hsla,
        ink: Hsla,
    }
    impl Render for Mark {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let glyph = match &self.glyph {
                MarkGlyph::Asset(asset) => {
                    // The reference bypasses status_path and status_mark's mapping.
                    div()
                        .size(Pixels::from(16.))
                        .flex_none()
                        .child(
                            gpui_kit::component::Icon::default()
                                .path(*asset)
                                .size(Pixels::from(16.))
                                .text_color(self.ink),
                        )
                        .into_any_element()
                }
                MarkGlyph::Status { status, phase } => {
                    icons::status_mark(status, *phase, Pixels::from(16.), self.ink)
                        .into_any_element()
                }
            };
            div()
                .size_full()
                .bg(self.background)
                .flex()
                .items_center()
                .justify_center()
                .child(glyph)
        }
    }

    fn statuses() -> [(Status, &'static str); 7] {
        [
            (Status::Working { since: 0 }, "marks/working.svg"),
            (Status::NeedsYou, "marks/needs-you.svg"),
            (Status::Done { took: 0 }, "marks/done.svg"),
            (Status::Failed, "marks/failed.svg"),
            (Status::Idle, "marks/idle.svg"),
            (Status::Held, "marks/held.svg"),
            (Status::Interrupted, "marks/interrupted.svg"),
        ]
    }

    fn mark_cell() -> Bounds<Pixels> {
        Bounds {
            origin: point(Pixels::from(8.), Pixels::from(8.)),
            size: size(Pixels::from(16.), Pixels::from(16.)),
        }
    }

    fn reference_shape(asset: &'static str, background: Hsla, ink: Hsla) -> Vec<bool> {
        let mut cx = context();
        let window = cx
            .open_window(size(Pixels::from(32.), Pixels::from(32.)), |_, cx| {
                cx.new(|_| Mark {
                    glyph: MarkGlyph::Asset(asset),
                    background,
                    ink,
                })
            })
            .unwrap();
        draw(&mut cx, window.into());
        Frame::capture(&mut cx, window.into()).shape(mark_cell())
    }

    fn status_marks_have_distinct_raster_shapes_and_working_steps() {
        let mut cx = context();
        let theme = Theme::new(Appearance::Dark);
        let window = cx
            .open_window(size(Pixels::from(32.), Pixels::from(32.)), |_, cx| {
                cx.new(|_| Mark {
                    glyph: MarkGlyph::Status {
                        status: Status::Idle,
                        phase: 0,
                    },
                    background: theme.colors.bg,
                    ink: theme.colors.text,
                })
            })
            .unwrap();
        let cell = mark_cell();
        let mut shapes = Vec::new();
        for (status, asset) in statuses() {
            cx.update(|cx| {
                window.update(cx, |mark, _, cx| {
                    mark.glyph = MarkGlyph::Status {
                        status: status.clone(),
                        phase: 0,
                    };
                    cx.notify();
                })
            })
            .unwrap();
            draw(&mut cx, window.into());
            let shape = Frame::capture(&mut cx, window.into()).shape(cell);
            assert!(
                shape == reference_shape(asset, theme.colors.bg, theme.colors.text),
                "status raster does not match its asset: {status:?} {asset}"
            );
            assert!(
                !shapes.contains(&shape),
                "status raster shape repeated: {status:?}"
            );
            shapes.push(shape);
        }
        let mut steps = Vec::new();
        for phase in 0..icons::WORKING_STEPS_PER_TURN as u8 {
            cx.update(|cx| {
                window.update(cx, |mark, _, cx| {
                    mark.glyph = MarkGlyph::Status {
                        status: Status::Working { since: 0 },
                        phase,
                    };
                    cx.notify();
                })
            })
            .unwrap();
            draw(&mut cx, window.into());
            let shape = Frame::capture(&mut cx, window.into()).shape(cell);
            assert!(
                !steps.contains(&shape),
                "Working raster step repeated: {phase}"
            );
            steps.push(shape);
        }
        assert_eq!(steps.len(), 12);
    }

    struct Paint {
        theme: Theme,
        scale: f32,
        quads: Vec<::gpui::Quad>,
        controls: BTreeMap<String, Bounds<Pixels>>,
        active: bool,
    }
    fn control(controls: &BTreeMap<String, Bounds<Pixels>>, name: &str) -> Bounds<Pixels> {
        controls
            .get(&format!("Name(\"{name}\")"))
            .copied()
            .unwrap_or_else(|| panic!("control was not painted: {name}"))
    }

    impl Paint {
        fn control(&self, name: &str) -> Bounds<Pixels> {
            control(&self.controls, name)
        }
    }

    struct Fixture {
        cx: HeadlessAppContext,
        window: AnyWindowHandle,
        shell: Entity<Shell>,
        sender: tokio::sync::mpsc::UnboundedSender<UiEvent>,
        _runtime: tokio::runtime::Runtime,
        _scratch: crate::tests::Scratch,
        thread: ThreadId,
        turn: TurnId,
    }
    impl Fixture {
        fn new(width: f32, height: f32) -> Self {
            let scratch = crate::tests::Scratch(
                std::path::PathBuf::from("/tmp").join(format!("veyra-vtests-{}", ThreadId::new())),
            );
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let engine = Engine::start_with_env(
                runtime.handle(),
                veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
                veyra_engine::provider::env::ChildEnv::isolated(),
            )
            .unwrap();
            runtime.block_on(engine.shutdown_wait());
            let (sender, events) = tokio::sync::mpsc::unbounded_channel();
            let mut cx = context();
            let thread = ThreadId::new();
            let turn = TurnId::new();
            let (window, shell) = cx
                .update(|cx| {
                    gpui_kit::open_window(
                        WindowOptions {
                            window_bounds: Some(WindowBounds::Windowed(Bounds {
                                origin: Default::default(),
                                size: size(Pixels::from(width), Pixels::from(height)),
                            })),
                            focus: false,
                            show: false,
                            ..Default::default()
                        },
                        cx,
                        |window, cx| {
                            Theme::apply(Appearance::Dark, window, cx);
                            cx.new(|cx| {
                                let mut shell = Shell::new(
                                    engine,
                                    runtime.handle().clone(),
                                    events,
                                    scratch.0.clone(),
                                    window,
                                    cx,
                                );
                                shell.probes = Default::default();
                                shell.providers = vec![Probe {
                                    provider: ProviderKind::Claude,
                                    path: None,
                                    version: None,
                                    readiness: Readiness::Ready,
                                    models: vec![],
                                    account: None,
                                }];
                                let project = ProjectId::new();
                                shell.state.projects.insert(
                                    project,
                                    Project {
                                        id: project,
                                        path: "/tmp".into(),
                                        name: "Headless project".into(),
                                    },
                                );
                                shell.state.threads.insert(
                                    thread,
                                    Thread {
                                        id: thread,
                                        project,
                                        title: "Headless thread".into(),
                                        lineage: Lineage::Root,
                                        provider: ProviderKind::Claude,
                                        config: shell.config.clone(),
                                        cwd: "/tmp".into(),
                                        native: None,
                                        cli_version: None,
                                        hold: None,
                                        archived: false,
                                    },
                                );
                                shell.state.turns.insert(
                                    turn,
                                    Turn {
                                        id: turn,
                                        thread,
                                        ordinal: 1,
                                        origin: TurnOrigin::Provider,
                                        state: TurnState::Running { since: 0 },
                                    },
                                );
                                shell.view = MainView::Chat(thread);
                                shell.project = Some(project);
                                shell
                            })
                        },
                    )
                })
                .unwrap();
            let mut this = Self {
                cx,
                window,
                shell,
                sender,
                _runtime: runtime,
                _scratch: scratch,
                thread,
                turn,
            };
            this.frame();
            this.cx
                .update_window(this.window, |_, window, _| window.activate_window())
                .unwrap();
            this.frame();
            this
        }
        fn update<R>(
            &mut self,
            f: impl FnOnce(&mut Shell, &mut Window, &mut Context<Shell>) -> R,
        ) -> R {
            let shell = self.shell.clone();
            self.cx
                .update_window(self.window, |_, window, cx| {
                    shell.update(cx, |shell, cx| f(shell, window, cx))
                })
                .unwrap()
        }
        fn frame(&mut self) {
            draw(&mut self.cx, self.window);
        }
        fn dump(&mut self) -> serde_json::Value {
            self.cx
                .update(|cx| self.shell.read(cx).dump(std::path::Path::new("/tmp"), cx))
        }
        fn action(&mut self, action: impl gpui::Action) {
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.dispatch_action(Box::new(action), cx)
                })
                .unwrap();
            self.frame();
        }
        fn resize(&mut self, width: f32, height: f32) {
            self.cx
                .update_window(self.window, |_, window, _| {
                    window.resize(size(Pixels::from(width), Pixels::from(height)))
                })
                .unwrap();
            self.frame();
        }
        fn appearance(&mut self, appearance: Appearance) {
            self.cx
                .update_window(self.window, |_, w, cx| Theme::apply(appearance, w, cx))
                .unwrap();
            self.frame();
        }
        fn paint(&mut self) -> Paint {
            self.cx
                .update_window(self.window, |_, w, cx| Paint {
                    theme: cx.global::<Theme>().clone(),
                    scale: w.scale_factor(),
                    quads: w.painted_quads(),
                    controls: cx.global::<PaintedControls>().0.clone(),
                    active: w.is_window_active(),
                })
                .unwrap()
        }
        fn question(&mut self) -> RequestId {
            let id = RequestId::new();
            let thread = self.thread;
            let turn = self.turn;
            self.update(|shell, _, cx| {
                shell.state.requests = vec![Request {
                    id,
                    thread,
                    turn,
                    native: NativeRequestRef("headless-question".into()),
                    state: RequestState::Pending,
                    kind: RequestKind::Question {
                        questions: vec![Question {
                            text: "Choose a route".into(),
                            options: vec!["Short".into(), "Long".into()],
                        }],
                    },
                }];
                cx.notify();
            });
            self.frame();
            id
        }
        fn approval(&mut self) -> RequestId {
            let id = self.question();
            self.update(|shell, _, cx| {
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction::new(
                        "Bash".into(),
                        ApprovalTarget::Command {
                            command: "echo ok".into(),
                            cwd: CommandCwd::ProjectFolder,
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    )
                    .with_grant(Some(GrantKey {
                        tool: "Bash".into(),
                        command: "echo ok".into(),
                    })),
                };
                cx.notify();
            });
            self.frame();
            id
        }
    }

    fn buttons_light_only_explicit_plates() {
        fn check(fixture: &mut Fixture, name: &str, fill: Option<Hsla>) {
            let paint = fixture.paint();
            let bounds = paint.control(name).scale(paint.scale);
            let gradients: Vec<_> = paint
                .quads
                .iter()
                .filter(|q| q.bounds == bounds && q.background.as_solid().is_none())
                .collect();
            match fill {
                Some(fill) => {
                    assert_eq!(gradients.len(), 1, "{name} must paint one lit plate");
                    assert_eq!(
                        gradients[0].background,
                        paint.theme.plate(fill),
                        "{name} fill"
                    );
                    assert!(
                        paint.quads.iter().any(|quad| quad.bounds == bounds
                            && quad.border_color == paint.theme.colors.border
                            && quad.border_widths.top
                                == paint.theme.layout.border.scale(paint.scale)),
                        "{name} must paint the one-pixel rim"
                    );
                }
                None => assert!(gradients.is_empty(), "{name} must paint flat"),
            }
        }
        for appearance in [Appearance::Dark, Appearance::Light] {
            let mut fixture = Fixture::new(960., 740.);
            fixture.appearance(appearance);
            let request = fixture.approval();
            let solid = fixture.paint().theme.colors.solid;
            check(&mut fixture, &format!("allow-once-{request}"), Some(solid));
            check(&mut fixture, &format!("deny-{request}"), None);
            check(&mut fixture, &format!("allow-thread-{request}"), None);
            fixture.update(|shell, _, cx| {
                shell.state.requests.clear();
                cx.notify();
            });
            fixture.frame();
            let stop = format!("stop-{}-{}", fixture.thread, fixture.turn);
            check(&mut fixture, &stop, Some(solid));
            fixture.update(|shell, window, cx| {
                shell
                    .input
                    .update(cx, |input, cx| input.set_value("A draft", window, cx));
                cx.notify();
            });
            fixture.frame();
            check(&mut fixture, "send", Some(solid));
            fixture.update(|shell, _, cx| {
                shell.quit_sheet = true;
                cx.notify();
            });
            fixture.frame();
            check(&mut fixture, "confirm-quit", None);
            let thread = fixture.thread;
            fixture.update(|shell, _, cx| {
                shell.quit_sheet = false;
                shell.close_sheet = Some(drawer::CloseSheet {
                    target: drawer::CloseTarget::Tab { thread, number: 1 },
                    processes: vec!["Owned shell: sleep".into()],
                    drawer_focus: false,
                });
                cx.notify();
            });
            fixture.frame();
            check(&mut fixture, "confirm-close-shell", None);
        }
    }

    fn reply_code_unwrapped_sideways_scroll_probe() {
        struct Probe;
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                use gpui_kit::component::text::{TextView, TextViewStyle};
                let t = cx.global::<Theme>();
                let mut code = StyleRefinement::default()
                    .whitespace_nowrap()
                    .font_family(t.text.mono_font.clone())
                    .text_size(t.text.code.size)
                    .line_height(t.text.code.line_height)
                    .pt(Pixels::from(48.));
                code.overflow.x = Some(Overflow::Scroll);
                div().size_full().bg(t.colors.bg).child(
                    TextView::markdown(
                        "wrap-prerequisite",
                        format!("```rust\nLEFT_{}RIGHT\n```", "abcdefghij".repeat(50)),
                    )
                    .style(TextViewStyle::default().code_block(code))
                    .code_block_actions(|_, _, _| div().child("Probe")),
                )
            }
        }
        let mut cx = context();
        let window = cx
            .open_window(
                size(Pixels::from(360.), Pixels::from(160.)),
                |window, cx| {
                    Theme::apply(Appearance::Dark, window, cx);
                    cx.new(|_| Probe)
                },
            )
            .unwrap();
        draw(&mut cx, window.into());
        let region = Bounds {
            origin: point(Pixels::from(20.), Pixels::from(48.)),
            size: size(Pixels::from(320.), Pixels::from(20.)),
        };
        let before = Frame::capture(&mut cx, window.into()).region(region);
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                    position: point(Pixels::from(180.), Pixels::from(58.)),
                    delta: gpui::ScrollDelta::Pixels(point(Pixels::from(-144.), Pixels::ZERO)),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
        draw(&mut cx, window.into());
        let after = Frame::capture(&mut cx, window.into()).region(region);
        let changed = before
            .iter()
            .zip(&after)
            .filter(|(left, right)| left != right)
            .count();
        println!("WRAP_CHECK unwrapped=true horizontal_wheel=-144 changed_code_pixels={changed}");
        assert!(
            changed > 100,
            "gpui-kit 0.7.3 unwrapped reply code did not scroll sideways"
        );
    }

    fn approval_command_plates_and_decision_controls() {
        let mut fixture = Fixture::new(960., 740.);
        let request = fixture.approval();
        for appearance in [Appearance::Dark, Appearance::Light] {
            fixture.appearance(appearance);
            let paint = fixture.paint();
            let t = &paint.theme;
            let scale = paint.scale;
            for prefix in ["deny", "allow-thread"] {
                let bounds = paint.control(&format!("{prefix}-{request}")).scale(scale);
                assert!(
                    paint.quads.iter().any(|q| q.bounds == bounds
                        && q.border_color == t.colors.border
                        && q.border_widths.top > Pixels::from(0.).scale(scale)),
                    "approval {prefix} outline {appearance:?}"
                );
            }
            let once = paint.control(&format!("allow-once-{request}")).scale(scale);
            assert!(
                paint
                    .quads
                    .iter()
                    .any(|q| q.bounds == once && q.background == t.plate(t.colors.solid)),
                "approval primary solid fill {appearance:?}"
            );
            let dock = fixture
                .update(|shell, _, _| shell.composer_dock_bounds.get())
                .scale(scale);
            assert!(
                paint
                    .quads
                    .iter()
                    .any(|q| q.background == t.colors.code_wash.into()
                        && q.bounds.size.width > dock.size.width * 0.8
                        && dock.contains(&q.bounds.origin)
                        && dock.contains(&q.bounds.bottom_right())),
                "approval command wash {appearance:?}"
            );
            let frame = Frame::capture(&mut fixture.cx, fixture.window);
            let ink = frame.color_pixels(
                paint.control(&format!("allow-thread-{request}")),
                t.colors.warning,
            );
            assert!(
                ink >= (2. * frame.scale * frame.scale) as usize,
                "approval grant warning ink {appearance:?}"
            );
        }
    }

    fn semantic_approval_projection_and_control_bounds() {
        let mut fixture = Fixture::new(760., 600.);
        let request = fixture.approval();
        for width in [760., 960., 1950.] {
            fixture.resize(width, 740.);
            let dump = fixture.dump();
            let dock = &dump["composer"]["dock"];
            assert_eq!(dock["action_text"], "echo ok");
            assert_eq!(dock["grant_caption"], "Allow `echo ok` in this thread");
            assert_eq!(
                dock["request"]["kind"]["Approval"]["action"]["target"]["command"],
                "echo ok"
            );
            assert_eq!(dock["allow_once_enabled"], true);
            assert_eq!(dock["allow_in_thread_enabled"], true);
            fixture.cx.update(|cx| {
                let controls = &cx.global::<PaintedControls>().0;
                for name in ["deny", "allow-once", "allow-thread"] {
                    let bounds = control(controls, &format!("{name}-{request}"));
                    assert!(
                        bounds.left() >= Pixels::from(0.)
                            && bounds.top() >= Pixels::from(0.)
                            && bounds.right() <= Pixels::from(width)
                            && bounds.bottom() <= Pixels::from(740.),
                        "approval control escaped viewport: {name}"
                    );
                }
            });
        }
    }

    fn option_cursor_paints_only_the_current_row() {
        let mut fixture = Fixture::new(760., 740.);
        let request = fixture.question();
        for appearance in [Appearance::Dark, Appearance::Light] {
            fixture.appearance(appearance);
            for width in [760., 960., 1950.] {
                fixture.resize(width, 740.);
                fixture.action(FocusDock);
                fixture.action(MoveOptionUp);
                for cursor in 0..2 {
                    if cursor == 1 {
                        fixture.action(MoveOptionDown);
                    }
                    let state = fixture.update(|shell, _, _| {
                        (
                            shell.dock_focused,
                            shell.question_scroll.cursor,
                            shell.state.requests[0].id,
                        )
                    });
                    assert_eq!(state, (true, cursor, request));
                    let viewport =
                        fixture.update(|shell, _, _| shell.question_scroll.options.bounds());
                    let paint = fixture.paint();
                    let theme = &paint.theme;
                    let scale = paint.scale;
                    let quads = &paint.quads;
                    let frame = Frame::capture(&mut fixture.cx, fixture.window);
                    let viewport = viewport.scale(scale);
                    let mut rows = quads
                        .iter()
                        .filter(|q| {
                            q.background == theme.colors.card.into()
                                && q.bounds.left() >= viewport.left()
                                && q.bounds.right() <= viewport.right()
                                && q.bounds.top() >= viewport.top()
                                && q.bounds.bottom() <= viewport.bottom()
                        })
                        .collect::<Vec<_>>();
                    rows.sort_by_key(|q| q.bounds.top());
                    assert_eq!(rows.len(), 2);
                    for (option, row) in rows.into_iter().enumerate() {
                        let row = row.bounds.map(|v| Pixels::from(v.as_f32() / scale));
                        let bounds = row.inset(theme.layout.focus_inset).scale(scale);
                        let plates: Vec<_> = quads
                            .iter()
                            .filter(|q| {
                                q.border_color == theme.colors.accent
                                    && row.scale(scale).contains(&q.bounds.origin)
                                    && row.scale(scale).contains(&q.bounds.bottom_right())
                            })
                            .collect();
                        assert_eq!(
                            plates.len(),
                            if option == cursor { 4 } else { 0 },
                            "painted option cursor count {appearance:?} width={width} cursor={cursor} option={option}"
                        );
                        for plate in &plates {
                            assert_eq!(plate.bounds, bounds);
                            assert!(plate.background.is_transparent());
                            let stroke = theme.layout.focus_ring.scale(scale);
                            assert_eq!(plate.border_widths, gpui::Edges::all(stroke));
                            let radius =
                                (theme.radius.control - theme.layout.focus_inset).scale(scale);
                            assert_eq!(plate.corner_radii, gpui::Corners::all(radius));
                            for point in [bounds.origin, bounds.bottom_right()] {
                                // Rounded corners are clipped intentionally; their rectangle
                                // remains within the option's viewport content mask.
                                assert!(viewport.contains(&point));
                            }
                        }
                        if option == cursor {
                            let half = theme.layout.focus_ring.scale(scale) / 2.;
                            for point in [
                                point(bounds.center().x, bounds.top() + half),
                                point(bounds.right() - half, bounds.center().y),
                                point(bounds.center().x, bounds.bottom() - half),
                                point(bounds.left() + half, bounds.center().y),
                            ] {
                                assert!(
                                    plates
                                        .iter()
                                        .any(|quad| quad.content_mask.bounds.contains(&point)),
                                    "option cursor side midpoint clipped: {point:?} {appearance:?} width={width}"
                                );
                            }
                        }
                        let ink = frame.color_pixels(
                            bounds.map(|v| Pixels::from(v.as_f32() / scale)),
                            theme.colors.accent,
                        );
                        if option == cursor {
                            assert!(
                                ink >= (8. * scale * scale) as usize,
                                "cursor ring pixels missing"
                            );
                        } else {
                            assert_eq!(ink, 0, "cursor ring pixels on unselected option");
                        }
                    }
                }
            }
        }
    }

    fn family_status_marks_follow_child_status() {
        let mut fixture = Fixture::new(960., 740.);
        let child = ThreadId::new();
        let root = fixture.thread;
        fixture.update(|shell, _, cx| {
            let mut row = shell.state.threads[&root].clone();
            row.id = child;
            row.title = "Mark".into();
            row.lineage = Lineage::Child {
                parent: root,
                root,
                relationship: Relationship::Subagent,
            };
            shell.state.threads.insert(child, row);
            shell.scroller.update(cx, |scroller, cx| {
                scroller.reset(shell.state.transcript_rows(root), cx)
            });
            cx.notify();
        });
        let mut shapes = Vec::new();
        for (status, asset) in statuses() {
            fixture.update(|shell, _, cx| {
                shell.state.statuses.insert(child, status.clone());
                cx.notify();
            });
            fixture.frame();
            let paint = fixture.paint();
            let control = paint.control(&format!("child-open-{child}"));
            let t = &paint.theme;
            let bounds = Bounds {
                origin: point(
                    control.left(),
                    control.center().y - t.layout.status_glyph / 2.,
                ),
                size: size(t.layout.status_glyph, t.layout.status_glyph),
            };
            let shape = Frame::capture(&mut fixture.cx, fixture.window).shape(bounds);
            assert!(
                shape == reference_shape(asset, t.colors.card, sidebar::glyph_color(&status, t)),
                "family status raster does not match its asset: {status:?} {asset}"
            );
            assert!(
                !shapes.contains(&shape),
                "family card repeated a status raster: {status:?}"
            );
            shapes.push(shape);
        }
    }

    fn composer_accent_and_unscrolled_header_rule() {
        let mut fixture = Fixture::new(960., 740.);
        for appearance in [Appearance::Dark, Appearance::Light] {
            fixture.appearance(appearance);
            fixture.update(|shell, window, cx| {
                shell.input.update(cx, |input, cx| input.focus(window, cx))
            });
            fixture.frame();
            let surface = fixture.update(|shell, _, _| shell.composer_surface_bounds.get());
            let paint = fixture.paint();
            let t = &paint.theme;
            assert!(paint.active);
            assert!(
                paint
                    .quads
                    .iter()
                    .any(|q| q.bounds == surface.scale(paint.scale)
                        && q.border_color == t.colors.accent),
                "focused composer accent edge {appearance:?}"
            );
            let band = Bounds {
                origin: point(Pixels::from(320.), t.layout.header - Pixels::from(1.)),
                size: size(Pixels::from(500.), Pixels::from(1.)),
            };
            let image = Frame::capture(&mut fixture.cx, fixture.window);
            assert!(
                image.color_pixels(band, t.colors.bg) * 10 >= image.region(band).len() * 9,
                "unscrolled header painted a rule {appearance:?}"
            );
        }
    }

    fn settings_card_tracks_and_lit_thumbs() {
        let mut fixture = Fixture::new(960., 740.);
        fixture.update(|shell, _, cx| {
            shell.view = MainView::Settings { back: None };
            cx.notify();
        });
        for appearance in [Appearance::Dark, Appearance::Light] {
            fixture.appearance(appearance);
            let paint = fixture.paint();
            let t = &paint.theme;
            let scale = paint.scale;
            let quads = &paint.quads;
            assert!(
                quads.iter().any(|q| q.background == t.colors.card.into()
                    && q.bounds.size.width > Pixels::from(250.).scale(scale)
                    && q.bounds.size.height > Pixels::from(100.).scale(scale)),
                "settings group card background {appearance:?}"
            );
            let tracks: Vec<_> = quads
                .iter()
                .filter(|q| {
                    q.background == t.colors.card.into()
                        && q.bounds.size.height == t.layout.segment_height.scale(scale)
                        && q.corner_radii.top_left == t.radius.track.scale(scale)
                })
                .collect();
            assert_eq!(tracks.len(), 2, "settings card tracks {appearance:?}");
            for track in tracks {
                let thumbs: Vec<_> = quads
                    .iter()
                    .filter(|q| {
                        q.background == t.plate(t.colors.raised)
                            && q.bounds.left() > track.bounds.left()
                            && q.bounds.right() < track.bounds.right()
                            && q.bounds.top() >= track.bounds.top()
                            && q.bounds.bottom() <= track.bounds.bottom()
                    })
                    .collect();
                assert_eq!(thumbs.len(), 1, "settings raised lit thumb {appearance:?}");
            }
        }
    }

    fn semantic_placeholders_and_input_clipping() {
        let mut cx = ::gpui::TestAppContext::build_with_text_system(
            ::gpui::TestDispatcher::new(0),
            Some("semantic_placeholders_and_input_clipping"),
            TEXT.with(Clone::clone),
        );
        cx.update(|cx| theme::register_fonts(cx).unwrap());
        super::super::tests::with_docked_shell(&mut cx, |shell, cx, _, _| {
            let thread = shell.update(cx, |shell, _| {
                let (thread, _) = super::super::tests::configure_dock_thread(
                    shell,
                    ProviderKind::Claude,
                    false,
                    false,
                );
                shell.state.requests.clear();
                thread
            });
            for appearance in [Appearance::Dark, Appearance::Light] {
                cx.update(|window, cx| Theme::apply(appearance, window, cx));
                for (width, height) in [(760., 600.), (1950., 1385.)] {
                    cx.simulate_resize(size(Pixels::from(width), Pixels::from(height)));
                    for (status, hold, expected) in [
                        (Status::Idle, false, "Reply to Claude Code"),
                        (Status::Working { since: 0 }, false, "Queue a follow-up"),
                        (Status::NeedsYou, false, "Queue a follow-up"),
                        (Status::Held, true, "Write now, it waits for Continue"),
                    ] {
                        shell.update(cx, |shell, cx| {
                            shell.state.statuses.insert(thread, status);
                            shell.state.threads.get_mut(&thread).unwrap().hold =
                                hold.then_some(HoldReason::ChildReport);
                            cx.notify();
                        });
                        super::super::tests::draw(cx);
                        shell.read_with(cx, |shell, cx| {
                            let dump = shell.dump(std::path::Path::new("/tmp"), cx);
                            assert_eq!(dump["composer"]["placeholder"], expected);
                            assert_eq!(
                                shell.input.read(cx).presentation().placeholder().as_ref(),
                                expected,
                                "rendered input placeholder"
                            );
                        });
                        let input = cx
                            .debug_bounds("composer-input")
                            .expect("composer input laid out");
                        assert!(
                            input.left() >= Pixels::from(0.)
                                && input.top() >= Pixels::from(0.)
                                && input.right() <= Pixels::from(width)
                                && input.bottom() <= Pixels::from(height),
                            "composer input escaped viewport {appearance:?} {input:?}"
                        );
                    }
                }
            }
        });
    }

    fn headless_stream_render_proxy() {
        let mut fixture = Fixture::new(960., 740.);
        let thread = fixture.thread;
        let turn = fixture.turn;
        let item = ItemId::new();
        fixture.update(|shell, _, cx| {
            shell
                .state
                .statuses
                .insert(thread, Status::Working { since: 0 });
            shell.state.apply(UiEvent::Change {
                counter: None,
                event: ChangeEvent::ItemAdded(Item {
                    id: item,
                    thread,
                    turn,
                    seq: 1,
                    body: ItemBody::AssistantText {
                        text: String::new(),
                        done: false,
                    },
                }),
            });
            shell.scroller.update(cx, |state, cx| {
                state.reset(shell.state.transcript_rows(thread), cx)
            });
            cx.notify();
        });
        fixture.frame();
        let before = fixture.update(|shell, _, _| shell.repaints);
        let mut quads = Vec::new();
        let began = Instant::now();
        for batch in 0..20 {
            fixture
                .sender
                .send(UiEvent::TextAppended {
                    thread,
                    item,
                    counter: batch + 1,
                    text: format!("batch {batch}\n"),
                    delta_count: 1,
                })
                .unwrap();
            fixture.cx.advance_clock(Duration::from_millis(16));
            fixture.frame();
            quads.push(
                fixture
                    .cx
                    .update_window(fixture.window, |_, w, _| w.painted_quads().len())
                    .unwrap(),
            );
        }
        let renders = fixture.update(|shell, _, _| shell.repaints) - before;
        let final_dump = fixture.dump();
        assert_eq!(final_dump["selected_thread"]["status"], "Working");
        let expected = (0..20)
            .map(|batch| format!("batch {batch}\n"))
            .collect::<String>();
        assert!(
            final_dump["transcript"]["items"]
                .to_string()
                .contains(&serde_json::to_string(&expected).unwrap()),
            "proxy lost or reordered streamed text"
        );
        assert!(renders >= 20 && quads.iter().all(|n| *n > 0));
        println!(
            concat!(
                "HEADLESS_PROXY batches=20 renders={} renders_per_batch={} ",
                "quads_per_frame={:?} advisory_us_per_frame={}"
            ),
            renders,
            renders as f64 / 20.,
            quads,
            began.elapsed().as_micros() as f64 / renders as f64
        );
    }

    fn reference_icon_shape(
        asset: &'static str,
        extent: Pixels,
        background: Hsla,
        ink: Hsla,
    ) -> Vec<bool> {
        struct IconReference {
            asset: &'static str,
            extent: Pixels,
            background: Hsla,
            ink: Hsla,
        }
        impl Render for IconReference {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .bg(self.background)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui_kit::component::Icon::default()
                            .path(self.asset)
                            .size(self.extent)
                            .text_color(self.ink),
                    )
            }
        }
        let mut cx = context();
        let window = cx
            .open_window(size(Pixels::from(32.), Pixels::from(32.)), |_, cx| {
                cx.new(|_| IconReference {
                    asset,
                    extent,
                    background,
                    ink,
                })
            })
            .unwrap();
        draw(&mut cx, window.into());
        Frame::capture(&mut cx, window.into()).shape(Bounds {
            origin: point(
                (Pixels::from(32.) - extent) / 2.,
                (Pixels::from(32.) - extent) / 2.,
            ),
            size: size(extent, extent),
        })
    }
    fn reference_text(
        text: &'static str,
        role: theme::TypeRole,
        t: &Theme,
        ink: Hsla,
        background: Hsla,
        bounds: Bounds<Pixels>,
    ) -> Vec<[u8; 3]> {
        struct Label {
            text: &'static str,
            role: theme::TypeRole,
            theme: Theme,
            ink: Hsla,
            background: Hsla,
            bounds: Bounds<Pixels>,
        }
        impl Render for Label {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().relative().bg(self.theme.colors.bg).child(
                    div()
                        .absolute()
                        .left(self.bounds.left())
                        .top(self.bounds.top())
                        .w(self.bounds.size.width)
                        .h(self.bounds.size.height)
                        .bg(self.background)
                        .font_family(self.theme.text.ui_font.clone())
                        .text_size(self.role.size)
                        .line_height(self.role.line_height)
                        .font_weight(self.role.weight)
                        .text_color(self.ink)
                        .child(self.text),
                )
            }
        }
        let mut cx = context();
        let window = cx
            .open_window(
                size(
                    bounds.right() + Pixels::from(4.),
                    bounds.bottom() + Pixels::from(4.),
                ),
                |window, cx| {
                    Theme::apply(t.appearance, window, cx);
                    cx.new(|_| Label {
                        text,
                        role,
                        theme: t.clone(),
                        ink,
                        background,
                        bounds,
                    })
                },
            )
            .unwrap();
        draw(&mut cx, window.into());
        Frame::capture(&mut cx, window.into()).region(bounds)
    }
    fn code_block_corner_paints_language_copy_and_wrap() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let mut fixture = Fixture::new(960., 740.);
            fixture.appearance(appearance);
            let thread = fixture.thread;
            let turn = fixture.turn;
            let item = ItemId::new();
            fixture.update(|shell, _, cx| {
                shell.state.apply(UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::ItemAdded(Item {
                        id: item,
                        thread,
                        turn,
                        seq: 1,
                        body: ItemBody::AssistantText {
                            text: "```rust\nlet value = 42;\n```".into(),
                            done: true,
                        },
                    }),
                });
                shell.scroller.update(cx, |state, cx| {
                    state.reset(shell.state.transcript_rows(thread), cx)
                });
                cx.notify();
            });
            fixture.frame();
            let blocks = fixture.update(|shell, _, _| shell.code_blocks_dump(item));
            let start = blocks[0]["start"].as_u64().unwrap();
            let paint = fixture.paint();
            let language = paint.control(&format!("code-language-{item}-{start}"));
            let copy = paint.control(&format!("code-copy-{item}-{start}"));
            let wrap = paint.control(&format!("code-wrap-caption-{item}-{start}"));
            let frame = Frame::capture(&mut fixture.cx, fixture.window);
            for (text, role, ink, bounds) in [
                (
                    "rust",
                    paint.theme.text.micro,
                    paint.theme.colors.text_muted,
                    language,
                ),
                (
                    "Wrap",
                    paint.theme.text.label,
                    paint.theme.colors.text,
                    wrap,
                ),
            ] {
                let actual = frame.region(bounds);
                let [r, g, b] = actual[0];
                let background = Rgba {
                    r: f32::from(r) / 255.,
                    g: f32::from(g) / 255.,
                    b: f32::from(b) / 255.,
                    a: 1.,
                }
                .into();
                let expected = reference_text(text, role, &paint.theme, ink, background, bounds);
                assert_eq!(actual.len(), expected.len());
                let levels = |pixels: &[[u8; 3]]| {
                    let bg = pixels[0];
                    let contrast: Vec<_> = pixels
                        .iter()
                        .map(|p| p.iter().zip(bg).map(|(a, b)| a.abs_diff(b)).max().unwrap())
                        .collect();
                    let peak = *contrast.iter().max().unwrap();
                    assert!(peak > 24, "caption has no painted ink");
                    (
                        peak,
                        contrast
                            .into_iter()
                            .map(|value| f32::from(value) / f32::from(peak))
                            .collect::<Vec<_>>(),
                    )
                };
                let (actual_peak, actual) = levels(&actual);
                let (expected_peak, expected) = levels(&expected);
                assert!(
                    actual_peak.abs_diff(expected_peak) <= 5,
                    "{text} caption ink {appearance:?}"
                );
                // Subpixel compositing can move an edge pixel across half coverage. Every
                // definite foreground/background pixel must still match the independent text.
                assert!(
                    actual
                        .iter()
                        .zip(expected)
                        .all(|(actual, expected)| !(*actual > 0.6 && expected < 0.4
                            || *actual < 0.4 && expected > 0.6)),
                    "{text} raster {appearance:?}"
                );
            }
            let glyph = Bounds {
                origin: copy.center()
                    - point(
                        paint.theme.layout.small_icon / 2.,
                        paint.theme.layout.small_icon / 2.,
                    ),
                size: size(paint.theme.layout.small_icon, paint.theme.layout.small_icon),
            };
            assert!(
                frame.shape(glyph)
                    == reference_icon_shape(
                        "icons/copy.svg",
                        paint.theme.layout.small_icon,
                        paint.theme.colors.code_wash,
                        paint.theme.colors.icon
                    ),
                "Copy glyph raster {appearance:?}"
            );
            assert!(language.right() <= copy.left() && copy.right() <= wrap.left());
        }
    }
    fn toast_marks_paint_their_status_shapes() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let mut fixture = Fixture::new(960., 740.);
            fixture.appearance(appearance);
            let thread = fixture.thread;
            for (status, asset) in statuses().into_iter().filter(|(status, _)| {
                matches!(
                    status,
                    Status::NeedsYou | Status::Done { .. } | Status::Failed
                )
            }) {
                fixture.update(|shell, _, cx| {
                    shell.toasts.attention(toasts::Attention {
                        root: thread,
                        thread,
                        title: "Other work".into(),
                        status: status.clone(),
                    });
                    cx.notify();
                });
                fixture.frame();
                let paint = fixture.paint();
                let bounds = paint.control(&format!("toast-mark-{thread}"));
                assert!(
                    Frame::capture(&mut fixture.cx, fixture.window).shape(bounds)
                        == reference_shape(
                            asset,
                            paint.theme.colors.floating,
                            sidebar::glyph_color(&status, &paint.theme)
                        ),
                    "toast mark {status:?} {appearance:?}"
                );
            }
            fixture.update(|shell, _, cx| {
                shell.toasts.attention.clear();
                shell.notice("A notice", cx);
            });
            fixture.frame();
            let paint = fixture.paint();
            assert!(
                Frame::capture(&mut fixture.cx, fixture.window)
                    .shape(paint.control("toast-mark-notice"))
                    == reference_shape(
                        "marks/idle.svg",
                        paint.theme.colors.floating,
                        paint.theme.colors.text_muted
                    )
            );
        }
    }
    fn storage_card_matches_kit_toast_frame_and_stack_cap() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let mut fixture = Fixture::new(960., 740.);
            fixture.appearance(appearance);
            fixture.update(|shell, _, cx| shell.notice("Storage is blocked", cx));
            fixture.frame();
            let kit = fixture.paint();
            let width = kit.theme.layout.toast_width.scale(kit.scale);
            let kit_frame = *kit
                .quads
                .iter()
                .find(|q| {
                    q.bounds.size.width == width && q.background == kit.theme.colors.floating.into()
                })
                .expect("kit toast frame painted");
            let thread = fixture.thread;
            fixture.update(|shell, _, cx| {
                shell.toasts.blocked = Some("Storage is blocked".into());
                for index in 0..4 {
                    let id = ThreadId::new();
                    let mut row = shell.state.threads[&thread].clone();
                    row.id = id;
                    shell.state.threads.insert(id, row);
                    shell.toasts.attention(toasts::Attention {
                        root: id,
                        thread: id,
                        title: format!("Attention {index}"),
                        status: Status::NeedsYou,
                    });
                }
                cx.notify();
            });
            fixture.frame();
            fixture.frame();
            let storage = fixture.paint();
            let bounds = storage.control("storage-toast").scale(storage.scale);
            let card = storage
                .quads
                .iter()
                .find(|q| {
                    q.bounds == bounds && q.background == storage.theme.colors.floating.into()
                })
                .expect("storage card painted");
            assert_eq!(
                card.bounds.size, kit_frame.bounds.size,
                "kit/storage frame size {appearance:?}"
            );
            assert_eq!(card.corner_radii, kit_frame.corner_radii);
            assert_eq!(card.background, kit_frame.background);
            assert!(storage.quads.iter().any(|q| q.bounds == bounds
                && q.border_color == storage.theme.colors.border
                && q.border_widths.top == storage.theme.layout.border.scale(storage.scale)));
            let marks = storage
                .controls
                .keys()
                .filter(|name| name.starts_with("Name(\"toast-mark-"))
                .count();
            assert_eq!(
                marks, 3,
                "storage card must reserve one of three shown slots"
            );
            let frame = Frame::capture(&mut fixture.cx, fixture.window);
            assert!(
                frame.shape(storage.control("toast-mark-storage"))
                    == reference_shape(
                        "marks/failed.svg",
                        storage.theme.colors.floating,
                        storage.theme.colors.danger
                    )
            );
            let copy = storage.control("copy-notice");
            assert!(
                copy.top().scale(storage.scale) >= bounds.top()
                    && copy.bottom().scale(storage.scale) <= bounds.bottom()
            );
            let dump = fixture.dump();
            assert_eq!(dump["notice"], dump["toasts"][0]["text"]);
        }
    }
    fn frame_guard_reads_palette_and_toasts() {
        use std::io::Write;
        let mut fixture = Fixture::new(960., 740.);
        fixture.update(|shell, _, cx| shell.notice("At rest", cx));
        fixture.frame();
        let dump = fixture.dump();
        let script = concat!(
            "import json,sys; from frame_scene import require_part,idle_variant; ",
            "d=json.load(sys.stdin); assert require_part(d,'6d')=='6d'; idle_variant(d,'6d')"
        );
        let mut child = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .env(
                "PYTHONPATH",
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../.agents/skills/verify-veyra/bin"),
            )
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&dump).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "frame_scene rejected actual dump: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[derive(Default)]
    struct Options {
        filters: Vec<String>,
        skips: Vec<String>,
        exact: bool,
        ignored: bool,
        list: bool,
    }
    impl Options {
        fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
            let mut options = Self::default();
            let mut args = args;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--list" => options.list = true,
                    "--exact" => options.exact = true,
                    "--ignored" => options.ignored = true,
                    "--nocapture" | "--show-output" => (), // This sequential runner always prints output.
                    "--skip" => options
                        .skips
                        .push(args.next().ok_or("--skip requires a filter")?),
                    "--" => {
                        options.filters.extend(args);
                        break;
                    }
                    _ if arg.starts_with("--skip=") => options.skips.push(arg[7..].to_owned()),
                    _ if arg.starts_with('-') => {
                        return Err(format!("unsupported test argument: {arg}"));
                    }
                    _ => options.filters.push(arg),
                }
            }
            Ok(options)
        }
        fn matches(&self, name: &str) -> bool {
            let matches = |filter: &String| {
                if self.exact {
                    name == filter
                } else {
                    name.contains(filter)
                }
            };
            !self.ignored
                && (self.filters.is_empty() || self.filters.iter().any(matches))
                && !self.skips.iter().any(matches)
        }
    }

    pub(crate) fn run() {
        let tests: &[(&str, fn())] = &[
            (
                "code_block_corner_paints_language_copy_and_wrap",
                code_block_corner_paints_language_copy_and_wrap,
            ),
            (
                "toast_marks_paint_their_status_shapes",
                toast_marks_paint_their_status_shapes,
            ),
            (
                "storage_card_matches_kit_toast_frame_and_stack_cap",
                storage_card_matches_kit_toast_frame_and_stack_cap,
            ),
            (
                "frame_guard_reads_palette_and_toasts",
                frame_guard_reads_palette_and_toasts,
            ),
            (
                "reply_code_unwrapped_sideways_scroll_probe",
                reply_code_unwrapped_sideways_scroll_probe,
            ),
            (
                "buttons_light_only_explicit_plates",
                buttons_light_only_explicit_plates,
            ),
            (
                "status_marks_have_distinct_raster_shapes_and_working_steps",
                status_marks_have_distinct_raster_shapes_and_working_steps,
            ),
            (
                "family_status_marks_follow_child_status",
                family_status_marks_follow_child_status,
            ),
            (
                "option_cursor_paints_only_the_current_row",
                option_cursor_paints_only_the_current_row,
            ),
            (
                "composer_accent_and_unscrolled_header_rule",
                composer_accent_and_unscrolled_header_rule,
            ),
            (
                "settings_card_tracks_and_lit_thumbs",
                settings_card_tracks_and_lit_thumbs,
            ),
            (
                "semantic_placeholders_and_input_clipping",
                semantic_placeholders_and_input_clipping,
            ),
            (
                "approval_command_plates_and_decision_controls",
                approval_command_plates_and_decision_controls,
            ),
            (
                "semantic_approval_projection_and_control_bounds",
                semantic_approval_projection_and_control_bounds,
            ),
            ("headless_stream_render_proxy", headless_stream_render_proxy),
        ];
        let options = Options::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
            eprintln!("headless: {error}");
            std::process::exit(2);
        });
        let selected: Vec<_> = tests
            .iter()
            .filter(|(name, _)| options.matches(name))
            .collect();
        if options.list {
            for (name, _) in selected {
                println!("{name}: test");
            }
            return;
        }
        let mut failed = 0;
        for (name, test) in &selected {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(*test));
            println!(
                "test {name} ... {}",
                if result.is_ok() { "ok" } else { "FAILED" }
            );
            failed += usize::from(result.is_err());
        }
        println!(
            "test result: {}. {} passed; {failed} failed; {} filtered out",
            if failed == 0 { "ok" } else { "FAILED" },
            selected.len() - failed,
            tests.len() - selected.len()
        );
        if failed != 0 {
            std::process::exit(1);
        }
    }
}
