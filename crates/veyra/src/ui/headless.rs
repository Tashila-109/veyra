//! Renderer checks. Imported by the test target, never by a production build.
#![cfg(all(test, debug_assertions))]
#![allow(clippy::disallowed_types, dead_code)]

#[cfg(test)]
#[allow(unused_imports)] // The ordinary unit-test binary does not call this runner.
pub(crate) use checks::run;

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

    struct Mark {
        status: Status,
        phase: u8,
    }
    impl Render for Mark {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::new(Appearance::Dark);
            div()
                .size_full()
                .bg(theme.colors.bg)
                .flex()
                .items_center()
                .justify_center()
                .child(icons::status_mark(
                    &self.status,
                    self.phase,
                    Pixels::from(16.),
                    theme.colors.text,
                ))
        }
    }

    fn statuses() -> [Status; 7] {
        [
            Status::Working { since: 0 },
            Status::NeedsYou,
            Status::Done { took: 0 },
            Status::Failed,
            Status::Idle,
            Status::Held,
            Status::Interrupted,
        ]
    }

    fn status_marks_have_distinct_raster_shapes_and_working_steps() {
        let mut cx = context();
        let window = cx
            .open_window(size(Pixels::from(32.), Pixels::from(32.)), |_, cx| {
                cx.new(|_| Mark {
                    status: Status::Idle,
                    phase: 0,
                })
            })
            .unwrap();
        let cell = Bounds {
            origin: point(Pixels::from(8.), Pixels::from(8.)),
            size: size(Pixels::from(16.), Pixels::from(16.)),
        };
        let mut shapes = Vec::new();
        for status in statuses() {
            cx.update(|cx| {
                window.update(cx, |mark, _, cx| {
                    mark.status = status.clone();
                    cx.notify();
                })
            })
            .unwrap();
            draw(&mut cx, window.into());
            let shape = Frame::capture(&mut cx, window.into()).shape(cell);
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
                    mark.status = Status::Working { since: 0 };
                    mark.phase = phase;
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
    impl Paint {
        fn control(&self, name: &str) -> Bounds<Pixels> {
            self.controls[&format!("Name(\"{name}\")")]
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
            let warning = Rgba::from(t.colors.warning);
            let expected = [
                (warning.r * 255.).round() as u8,
                (warning.g * 255.).round() as u8,
                (warning.b * 255.).round() as u8,
            ];
            let frame = Frame::capture(&mut fixture.cx, fixture.window);
            let ink = frame
                .region(paint.control(&format!("allow-thread-{request}")))
                .iter()
                .filter(|p| p.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 5))
                .count();
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
                    let bounds = controls[&format!("Name(\"{name}-{request}\")")];
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
                    let (theme, scale, quads) = fixture
                        .cx
                        .update_window(fixture.window, |_, window, cx| {
                            (
                                cx.global::<Theme>().clone(),
                                window.scale_factor(),
                                window.painted_quads(),
                            )
                        })
                        .unwrap();
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
                        for plate in plates {
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
                    }
                    // Also exercise the renderer, rather than only layout/paint on TestAppContext.
                    let image = Frame::capture(&mut fixture.cx, fixture.window);
                    assert_eq!(image.width, (width * image.scale) as usize);
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
        for status in statuses() {
            fixture.update(|shell, _, cx| {
                shell.state.statuses.insert(child, status.clone());
                cx.notify();
            });
            fixture.frame();
            let bounds = fixture.cx.update(|cx| {
                let control = cx
                    .global::<PaintedControls>()
                    .0
                    .get(&format!("Name(\"child-open-{child}\")"))
                    .copied()
                    .expect("family child control painted");
                let t = cx.global::<Theme>();
                Bounds {
                    origin: point(
                        control.left() + t.space.0[3],
                        control.center().y - t.layout.status_glyph / 2.,
                    ),
                    size: size(t.layout.status_glyph, t.layout.status_glyph),
                }
            });
            let shape = Frame::capture(&mut fixture.cx, fixture.window).shape(bounds);
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
            let bg = Rgba::from(t.colors.bg);
            let image = Frame::capture(&mut fixture.cx, fixture.window);
            let expected = [
                (bg.r * 255.).round() as u8,
                (bg.g * 255.).round() as u8,
                (bg.b * 255.).round() as u8,
            ];
            let samples = image.region(band);
            assert!(
                samples
                    .iter()
                    .filter(|p| p.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 5))
                    .count()
                    * 10
                    >= samples.len() * 9,
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
            assert!(
                !Frame::capture(&mut fixture.cx, fixture.window)
                    .rgba
                    .is_empty()
            );
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

    pub(crate) fn run() {
        let tests: &[(&str, fn())] = &[
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
        let args: Vec<_> = std::env::args().skip(1).collect();
        let filters: Vec<_> = args.iter().filter(|s| !s.starts_with('-')).collect();
        let selected: Vec<_> = tests
            .iter()
            .filter(|(name, _)| {
                filters.is_empty()
                    || filters.iter().any(|f| {
                        if args.iter().any(|s| s == "--exact") {
                            name == *f
                        } else {
                            name.contains(f.as_str())
                        }
                    })
            })
            .collect();
        if args.iter().any(|s| s == "--list") {
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
