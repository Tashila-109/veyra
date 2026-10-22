#![allow(clippy::disallowed_types)] // Debug terminal dump methods serialize grid and perf wire JSON.
use super::{actions::*, theme::Theme};
use gpui_kit::component::StyledExt;
use gpui_kit::{prelude::*, *};
#[cfg(debug_assertions)]
use std::time::Instant;
use veyra_engine::{
    provider::TerminalHandle,
    terminal::{
        Cell, Column, CursorShape, Dimensions, Flags, Line, NamedColor, PtySize, Rgb, TermMode,
    },
};

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Default)]
struct DebugTerminalGrid(Option<(u16, u16)>);
#[cfg(debug_assertions)]
impl Global for DebugTerminalGrid {}
#[cfg(debug_assertions)]
impl DebugTerminalGrid {
    fn parse(value: Option<&str>) -> Option<(u16, u16)> {
        let (cols, rows) = value?.split_once('x')?;
        if !cols.bytes().all(|byte| byte.is_ascii_digit())
            || !rows.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let cols = cols.parse::<u16>().ok()?;
        let rows = rows.parse::<u16>().ok()?;
        ((2..=1000).contains(&cols) && (1..=1000).contains(&rows)).then_some((cols, rows))
    }
}
#[cfg(debug_assertions)]
pub(super) fn init(cx: &mut App) {
    let value = std::env::var("VEYRA_DEBUG_TERMINAL_GRID").ok();
    cx.set_global(DebugTerminalGrid(DebugTerminalGrid::parse(
        value.as_deref(),
    )));
}

pub fn terminal_key_bytes(key: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = key.modifiers;
    let name = key.key.as_str();
    if m.platform {
        return match (name, m.shift, m.alt, m.control) {
            ("left", false, false, false) => Some(vec![1]),
            ("right", false, false, false) => Some(vec![5]),
            _ => None,
        };
    }
    if m.alt && !m.control && !m.shift && matches!(name, "left" | "right") {
        return Some(vec![27, if name == "left" { b'b' } else { b'f' }]);
    }
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    if modifier > 1 {
        let suffix = match name {
            "up" => Some('A'),
            "down" => Some('B'),
            "right" => Some('C'),
            "left" => Some('D'),
            "home" => Some('H'),
            "end" => Some('F'),
            "f1" => Some('P'),
            "f2" => Some('Q'),
            "f3" => Some('R'),
            "f4" => Some('S'),
            _ => None,
        };
        if let Some(suffix) = suffix {
            return Some(format!("\x1b[1;{modifier}{suffix}").into_bytes());
        }
        let number = match name {
            "insert" => Some(2),
            "delete" => Some(3),
            "pageup" => Some(5),
            "pagedown" => Some(6),
            "f5" => Some(15),
            "f6" => Some(17),
            "f7" => Some(18),
            "f8" => Some(19),
            "f9" => Some(20),
            "f10" => Some(21),
            "f11" => Some(23),
            "f12" => Some(24),
            _ => None,
        };
        if let Some(number) = number {
            return Some(format!("\x1b[{number};{modifier}~").into_bytes());
        }
    }
    if m.control {
        let byte = match name {
            "enter" => 13,
            "tab" => 9,
            "backspace" => 8,
            "escape" => 27,
            "space" | "@" | "2" => 0,
            "[" | "3" => 27,
            "\\" | "4" => 28,
            "]" | "5" => 29,
            "^" | "6" => 30,
            "_" | "-" | "7" => 31,
            "?" | "8" => 127,
            letter if letter.len() == 1 && letter.as_bytes()[0].is_ascii_alphabetic() => {
                letter.as_bytes()[0].to_ascii_lowercase() - b'a' + 1
            }
            _ => return None,
        };
        return Some(if m.alt { vec![27, byte] } else { vec![byte] });
    }
    let bytes: &[u8] = match name {
        "enter" if m.shift => b"\x1b\r",
        "enter" => b"\r",
        "tab" if m.shift => b"\x1b[Z",
        "tab" => b"\t",
        "backspace" => b"\x7f",
        "escape" => b"\x1b",
        "up" if app_cursor => b"\x1bOA",
        "down" if app_cursor => b"\x1bOB",
        "right" if app_cursor => b"\x1bOC",
        "left" if app_cursor => b"\x1bOD",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" if app_cursor => b"\x1bOH",
        "end" if app_cursor => b"\x1bOF",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "insert" => b"\x1b[2~",
        "delete" => b"\x1b[3~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "f1" => b"\x1bOP",
        "f2" => b"\x1bOQ",
        "f3" => b"\x1bOR",
        "f4" => b"\x1bOS",
        "f5" => b"\x1b[15~",
        "f6" => b"\x1b[17~",
        "f7" => b"\x1b[18~",
        "f8" => b"\x1b[19~",
        "f9" => b"\x1b[20~",
        "f10" => b"\x1b[21~",
        "f11" => b"\x1b[23~",
        "f12" => b"\x1b[24~",
        "space" => b" ",
        _ => {
            let text = key.key_char.as_deref().unwrap_or(name);
            if text.chars().count() != 1 {
                return None;
            }
            let mut bytes = if m.shift && key.key_char.is_none() {
                text.to_uppercase().into_bytes()
            } else {
                text.as_bytes().to_vec()
            };
            if m.alt {
                bytes.insert(0, 27);
            }
            return Some(bytes);
        }
    };
    let mut bytes = bytes.to_vec();
    if m.alt {
        bytes.insert(0, 27);
    }
    Some(bytes)
}

struct Grid {
    app_cursor: bool,
    rows: Vec<Vec<Cell>>,
    palette: Vec<Option<Rgb>>,
    cursor: Option<(usize, usize, CursorShape)>,
    #[cfg(debug_assertions)]
    flood: veyra_engine::terminal::FloodStamps,
}
fn snapshot(handle: &TerminalHandle) -> Option<Grid> {
    let mutex = handle.emulator()?;
    let _lease = mutex.lease();
    let emulator = mutex.lock_unfair();
    let term = &emulator.term;
    let cursor = term.renderable_content().cursor;
    Some(Grid {
        app_cursor: term.mode().contains(TermMode::APP_CURSOR),
        rows: (0..term.screen_lines())
            .map(|row| {
                (0..term.columns())
                    .map(|col| {
                        let point = veyra_engine::terminal::Point::new(
                            Line(row as i32 - term.grid().display_offset() as i32),
                            Column(col),
                        );
                        let mut cell = term.grid()[point].clone();
                        if term
                            .renderable_content()
                            .selection
                            .is_some_and(|selection| selection.contains(point))
                        {
                            cell.flags.toggle(Flags::INVERSE);
                        }
                        cell
                    })
                    .collect()
            })
            .collect(),
        palette: (0..=NamedColor::DimForeground as usize)
            .map(|i| term.colors()[i])
            .collect(),
        cursor: (term.grid().display_offset() == 0
            && cursor.shape != CursorShape::Hidden
            && cursor.point.line.0 >= 0)
            .then_some((
                cursor.point.line.0 as usize,
                cursor.point.column.0,
                cursor.shape,
            )),
        #[cfg(debug_assertions)]
        flood: emulator.flood,
    })
}
fn segments(row: &[Cell]) -> Vec<(usize, String, &Cell)> {
    let mut result = Vec::new();
    let mut column = 0;
    while column < row.len() {
        let cell = &row[column];
        let start = column;
        column += 1;
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let mut text = String::from(if cell.flags.contains(Flags::HIDDEN) {
            ' '
        } else {
            cell.c
        });
        if let Some(marks) = cell.zerowidth() {
            text.extend(marks);
        }
        if cell.c.is_ascii() && cell.zerowidth().is_none() {
            while let Some(next) = row.get(column) {
                if !next.c.is_ascii()
                    || next.flags != cell.flags
                    || next.fg != cell.fg
                    || next.bg != cell.bg
                    || next.zerowidth().is_some()
                {
                    break;
                }
                text.push(if next.flags.contains(Flags::HIDDEN) {
                    ' '
                } else {
                    next.c
                });
                column += 1;
            }
        }
        result.push((start, text, cell));
    }
    result
}
struct Frame {
    backgrounds: Vec<PaintQuad>,
    text: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    line_height: Pixels,
    #[cfg(debug_assertions)]
    flood: veyra_engine::terminal::FloodStamps,
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct FloodPerf {
    start: Option<veyra_engine::terminal::ReaderStamp>,
    done: Option<veyra_engine::terminal::ReaderStamp>,
    frames: Vec<Instant>,
    repaints: u64,
    frame_requests: u64,
}
#[cfg(debug_assertions)]
impl FloodPerf {
    fn paint(&mut self, stamps: veyra_engine::terminal::FloodStamps, at: Instant) {
        self.repaints += 1;
        if self.start.map(|s| s.at) != stamps.start.map(|s| s.at) {
            self.start = stamps.start;
            self.frames.clear();
        }
        self.done = stamps.done;
        if self.start.is_some_and(|start| at >= start.at)
            && self.done.is_none_or(|done| at <= done.at)
        {
            self.frames.push(at);
        }
    }
    fn dump(&self) -> Option<serde_json::Value> {
        let (start, done) = (self.start?, self.done?);
        let frames: Vec<_> = self
            .frames
            .iter()
            .copied()
            .filter(|at| *at >= start.at && *at <= done.at)
            .collect();
        let mut gaps: Vec<_> = frames
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64() * 1000.)
            .collect();
        gaps.sort_by(f64::total_cmp);
        let median = match gaps.len() {
            0 => 0.,
            n if n % 2 == 1 => gaps[n / 2],
            n => (gaps[n / 2 - 1] + gaps[n / 2]) / 2.,
        };
        Some(serde_json::json!({
            "pty_bytes":done.bytes.saturating_sub(start.bytes + b"\x1b]0;FLOOD_DONE\x07".len() as u64),
            "elapsed_ms":done.at.duration_since(start.at).as_secs_f64() * 1000.,
            "repaints":frames.len(), "median_repaint_gap_ms":median,
            "max_repaint_gap_ms":gaps.last().copied().unwrap_or_default()
        }))
    }
}

pub(super) struct TerminalView {
    pub handle: Option<TerminalHandle>,
    focus: FocusHandle,
    resize: Option<(u16, u16)>,
    pending_frame: bool,
    visible: bool,
    geometry: Option<(Bounds<Pixels>, Pixels, Pixels)>,
    selecting: bool,
    scrolled: bool,
    app_cursor: bool,
    error: Option<SharedString>,
    #[cfg(debug_assertions)]
    perf: FloodPerf,
    #[cfg(debug_assertions)]
    focused: bool,
}
impl TerminalView {
    pub fn new(
        handle: Option<TerminalHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(mut wake) = handle.as_ref().and_then(TerminalHandle::subscribe) {
            cx.spawn_in(window, async move |this, cx| {
                while wake.changed().await.is_ok() {
                    if this
                        .update_in(cx, |terminal, window, cx| {
                            terminal.queue_frame(window, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |view, _, cx| {
            #[cfg(debug_assertions)]
            {
                view.focused = true;
            }
            #[cfg(not(debug_assertions))]
            let _ = view;
            cx.notify();
        })
        .detach();
        cx.on_focus_out(&focus, window, |view, _, _, cx| {
            #[cfg(debug_assertions)]
            {
                view.focused = false;
            }
            #[cfg(not(debug_assertions))]
            let _ = view;
            cx.notify();
        })
        .detach();
        Self {
            handle,
            focus,
            resize: None,
            pending_frame: false,
            visible: false,
            geometry: None,
            selecting: false,
            scrolled: false,
            app_cursor: false,
            error: None,
            #[cfg(debug_assertions)]
            perf: FloodPerf::default(),
            #[cfg(debug_assertions)]
            focused: false,
        }
    }
    fn queue_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible && !self.pending_frame {
            #[cfg(debug_assertions)]
            {
                self.perf.frame_requests += 1;
            }
            self.pending_frame = true;
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |terminal, cx| {
                    terminal.pending_frame = false;
                    if terminal.visible {
                        cx.notify();
                    }
                });
            });
        }
    }
    #[cfg(debug_assertions)]
    pub fn frame_counts(&self) -> (u64, u64) {
        (self.perf.frame_requests, self.perf.repaints)
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            if visible {
                cx.notify();
            }
        }
    }
    #[cfg(debug_assertions)]
    pub fn focused(&self) -> bool {
        self.visible && self.focused
    }
    fn scroll(&mut self, scroll: veyra_engine::terminal::Scroll, cx: &mut Context<Self>) {
        if matches!(scroll, veyra_engine::terminal::Scroll::Bottom) && !self.scrolled {
            return;
        }
        if let Some(emulator) = self.handle.as_ref().and_then(TerminalHandle::emulator) {
            let mut emulator = emulator.lock();
            emulator.term.scroll_display(scroll);
            self.scrolled = emulator.term.grid().display_offset() != 0;
            cx.notify();
        }
    }
    pub fn send_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.scroll(veyra_engine::terminal::Scroll::Bottom, cx);
        if let Some(handle) = &self.handle
            && let Err(error) = handle.write(bytes)
        {
            self.error = Some(error.to_string().into());
            cx.notify();
        }
    }
    fn select(&mut self, position: Point<Pixels>, start: bool, cx: &mut Context<Self>) {
        use veyra_engine::terminal::{Point as CellPoint, Selection, SelectionType, Side};
        let Some((bounds, width, height)) = self.geometry else {
            return;
        };
        let Some(emulator) = self.handle.as_ref().and_then(TerminalHandle::emulator) else {
            return;
        };
        let mut emulator = emulator.lock();
        let term = &mut emulator.term;
        let row = (((position.y - bounds.origin.y) / height).floor() as i32)
            .clamp(0, term.screen_lines() as i32 - 1)
            - term.grid().display_offset() as i32;
        let col =
            (((position.x - bounds.origin.x) / width).floor() as usize).min(term.columns() - 1);
        let point = CellPoint::new(Line(row), Column(col));
        if start {
            term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
        } else if let Some(selection) = &mut term.selection {
            selection.update(point, Side::Right);
        }
        cx.notify();
    }
    fn paste(&mut self, _: &PasteShell, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let bracketed = self
                .handle
                .as_ref()
                .and_then(TerminalHandle::emulator)
                .is_some_and(|emulator| {
                    emulator
                        .lock()
                        .term
                        .mode()
                        .contains(TermMode::BRACKETED_PASTE)
                });
            self.send_input(&paste_bytes(&text, bracketed), cx);
        }
        cx.stop_propagation();
    }
    fn copy(&mut self, _: &CopyShell, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self
            .handle
            .as_ref()
            .and_then(TerminalHandle::emulator)
            .and_then(|emulator| emulator.lock().term.selection_to_string())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.stop_propagation();
    }
    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if let Some(bytes) = terminal_key_bytes(key, self.app_cursor) {
            self.send_input(&bytes, cx);
            cx.stop_propagation();
        }
    }
    #[cfg(debug_assertions)]
    pub fn dump(&self) -> serde_json::Value {
        let grid = self
            .handle
            .as_ref()
            .and_then(|handle| handle.emulator())
            .map(|emulator| emulator.lock().text())
            .unwrap_or_default();
        serde_json::json!({"grid":grid,"focused":self.focused,"columns":self.resize.map(|s|s.0),"rows\
            ":self.resize.map(|s|s.1),"repaints":self.perf.repaints})
    }
    #[cfg(debug_assertions)]
    pub fn flood(&self) -> Option<serde_json::Value> {
        self.perf.dump()
    }
    fn frame(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        let theme = cx.global::<Theme>();
        let mut font = gpui::font(theme.text.mono_font.clone());
        font.features = FontFeatures(
            ["calt", "liga", "dlig"]
                .map(|tag| (tag.to_owned(), 0))
                .to_vec()
                .into(),
        );
        let font_size = theme.text.terminal.size.to_pixels(window.rem_size());
        let line_height = theme.text.terminal.line_height.to_pixels(window.rem_size());
        let cell_width = window
            .text_system()
            .em_advance(window.text_system().resolve_font(&font), font_size)
            .expect("terminal font advance");
        let padding = theme.layout.terminal_padding;
        let origin = bounds.origin + point(padding.width, padding.height);
        self.geometry = Some((Bounds::new(origin, bounds.size), cell_width, line_height));
        let cols = (((bounds.size.width - padding.width * 2.) / cell_width).floor() as u16).max(2);
        let rows =
            (((bounds.size.height - padding.height * 2.) / line_height).floor() as u16).max(1);
        #[cfg(debug_assertions)]
        let (cols, rows) = cx
            .try_global::<DebugTerminalGrid>()
            .and_then(|grid| grid.0)
            .unwrap_or((cols, rows));
        if self.resize != Some((cols, rows))
            && let Some(handle) = &self.handle
        {
            match handle.resize(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                Ok(()) => self.resize = Some((cols, rows)),
                Err(error) if error.kind() == std::io::ErrorKind::NotConnected => {}
                Err(error) => self.error = Some(error.to_string().into()),
            }
        }
        let mut frame = Frame {
            backgrounds: Vec::new(),
            text: Vec::new(),
            cursor: None,
            line_height,
            #[cfg(debug_assertions)]
            flood: Default::default(),
        };
        let Some(grid) = self.handle.as_ref().and_then(snapshot) else {
            return frame;
        };
        self.app_cursor = grid.app_cursor;
        #[cfg(debug_assertions)]
        {
            frame.flood = grid.flood;
        }
        for (row, cells) in grid.rows.iter().enumerate() {
            let y = origin.y + line_height * row as f32;
            let mut column = 0;
            while column < cells.len() {
                let start = column;
                let background = |cell: &Cell| {
                    theme.terminal.resolve(
                        if cell.flags.contains(Flags::INVERSE) {
                            cell.fg
                        } else {
                            cell.bg
                        },
                        &grid.palette,
                    )
                };
                let bg = background(&cells[column]);
                column += 1;
                while column < cells.len() && background(&cells[column]) == bg {
                    column += 1;
                }
                if bg != theme.terminal.bg {
                    frame.backgrounds.push(fill(
                        Bounds::new(
                            point(origin.x + cell_width * start as f32, y),
                            size(cell_width * (column - start) as f32, line_height),
                        ),
                        bg,
                    ));
                }
            }
            for (column, text, cell) in segments(cells) {
                let mut color = theme.terminal.resolve(
                    if cell.flags.contains(Flags::INVERSE) {
                        cell.bg
                    } else {
                        cell.fg
                    },
                    &grid.palette,
                );
                if cell.flags.contains(Flags::DIM) {
                    color.a *= 0.5;
                }
                let mut font = font.clone();
                if cell.flags.contains(Flags::BOLD) {
                    font.weight = FontWeight::BOLD;
                }
                if cell.flags.contains(Flags::ITALIC) {
                    font.style = FontStyle::Italic;
                }
                let run = TextRun {
                    len: text.len(),
                    font,
                    color,
                    background_color: None,
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES).then_some(
                        UnderlineStyle {
                            color: Some(color),
                            thickness: theme.layout.border,
                            wavy: cell.flags.contains(Flags::UNDERCURL),
                        },
                    ),
                    strikethrough: cell.flags.contains(Flags::STRIKEOUT).then_some(
                        StrikethroughStyle {
                            color: Some(color),
                            thickness: theme.layout.border,
                        },
                    ),
                };
                let line = window
                    .text_system()
                    .shape_line(text.into(), font_size, &[run], None);
                frame
                    .text
                    .push((point(origin.x + cell_width * column as f32, y), line));
            }
        }
        if let Some((row, col, shape)) = grid.cursor {
            let mut bounds = Bounds::new(
                origin + point(cell_width * col as f32, line_height * row as f32),
                size(cell_width, line_height),
            );
            match shape {
                CursorShape::Beam => bounds.size.width = theme.layout.border,
                CursorShape::Underline => {
                    bounds.origin.y += line_height - theme.layout.border;
                    bounds.size.height = theme.layout.border;
                }
                _ => {}
            }
            frame.cursor = Some(if self.focus.is_focused(window) {
                fill(bounds, theme.terminal.selection)
            } else {
                outline(bounds, theme.colors.text_muted, BorderStyle::Solid)
            });
        }
        frame
    }
}
impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(debug_assertions)]
        {
            self.focused = self.focus.is_focused(window);
        }
        #[cfg(not(debug_assertions))]
        let _ = window;
        let theme = cx.global::<Theme>();
        let view = cx.entity();
        #[cfg(debug_assertions)]
        let painted = view.clone();
        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus)
            .key_context("VeyraTerminal")
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(|terminal, _: &ScrollShellHistoryUp, _, cx| {
                terminal.scroll(veyra_engine::terminal::Scroll::PageUp, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|terminal, _: &ScrollShellHistoryDown, _, cx| {
                terminal.scroll(veyra_engine::terminal::Scroll::PageDown, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(Self::key))
            .on_scroll_wheel(
                cx.listener(|terminal, event: &ScrollWheelEvent, window, cx| {
                    let height = cx
                        .global::<Theme>()
                        .text
                        .terminal
                        .line_height
                        .to_pixels(window.rem_size());
                    let rows = (event.delta.pixel_delta(height).y / height).round() as i32;
                    terminal.scroll(veyra_engine::terminal::Scroll::Delta(rows), cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|terminal, event: &MouseMoveEvent, _, cx| {
                if terminal.selecting {
                    terminal.select(event.position, false, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|terminal, _, _, _| terminal.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|terminal, _, _, _| terminal.selecting = false),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|terminal, event: &MouseDownEvent, window, cx| {
                    window.focus(&terminal.focus, cx);
                    terminal.selecting = true;
                    terminal.select(event.position, true, cx);
                }),
            )
            .child(
                div()
                    .debug_selector(|| "terminal-grid".into())
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(theme.terminal.bg)
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                view.update(cx, |view, cx| view.frame(bounds, window, cx))
                            },
                            move |bounds, frame, window, cx| {
                                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                    for quad in frame.backgrounds {
                                        window.paint_quad(quad);
                                    }
                                    for (origin, line) in frame.text {
                                        if let Err(error) = line.paint(
                                            origin,
                                            frame.line_height,
                                            TextAlign::Left,
                                            None,
                                            window,
                                            cx,
                                        ) {
                                            tracing::warn!(%error,"terminal text paint failed");
                                        }
                                    }
                                    if let Some(cursor) = frame.cursor {
                                        window.paint_quad(cursor);
                                    }
                                });
                                #[cfg(debug_assertions)]
                                painted.update(cx, |view, _| {
                                    view.perf.paint(frame.flood, Instant::now())
                                });
                            },
                        )
                        .size_full(),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::terminal_key_bytes;
    use gpui_kit::Keystroke;
    #[cfg(debug_assertions)]
    #[test]
    fn terminal_debug_grid_parser() {
        use super::DebugTerminalGrid;
        assert_eq!(DebugTerminalGrid::parse(None), None);
        for (value, expected) in [
            ("127x33", (127, 33)),
            ("2x1", (2, 1)),
            ("1000x1000", (1000, 1000)),
        ] {
            assert_eq!(DebugTerminalGrid::parse(Some(value)), Some(expected));
        }
        for value in [
            "",
            "127",
            "127X33",
            "127x",
            "x33",
            "0x33",
            "1x33",
            "127x0",
            "-2x33",
            "+127x33",
            "127x+33",
            "127x-1",
            "127x33x1",
            " 127x33",
            "127x33 ",
            "1001x33",
            "127x1001",
            "65535x65535",
            "65536x33",
            "127x65536",
            "１２７x33",
        ] {
            assert_eq!(DebugTerminalGrid::parse(Some(value)), None, "{value}");
        }
    }
    #[test]
    fn terminal_key_bytes_per_chord() {
        for (key, expected) in [
            ("shift-enter", &b"\x1b\r"[..]),
            ("alt-left", b"\x1bb"),
            ("alt-right", b"\x1bf"),
            ("secondary-left", b"\x01"),
            ("secondary-right", b"\x05"),
            ("ctrl-l", b"\x0c"),
            ("k", b"k"),
        ] {
            assert_eq!(
                terminal_key_bytes(&Keystroke::parse(key).unwrap(), false).as_deref(),
                Some(expected),
                "{key}"
            );
        }
        assert_eq!(
            terminal_key_bytes(&Keystroke::parse("secondary-j").unwrap(), false),
            None
        );
        assert_eq!(
            terminal_key_bytes(&Keystroke::parse("up").unwrap(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(&Keystroke::parse("ctrl-c").unwrap(), false),
            Some(vec![3])
        );
    }
    #[test]
    fn terminal_ordinary_modified_keys_use_legacy_sequences() {
        for (key, expected) in [
            ("shift-left", &b"\x1b[1;2D"[..]),
            ("ctrl-left", b"\x1b[1;5D"),
            ("alt-up", b"\x1b[1;3A"),
            ("shift-pageup", b"\x1b[5;2~"),
            ("ctrl-f1", b"\x1b[1;5P"),
            ("shift-k", b"K"),
        ] {
            assert_eq!(
                terminal_key_bytes(&Keystroke::parse(key).unwrap(), false).as_deref(),
                Some(expected),
                "{key}"
            );
        }
    }
    #[test]
    fn terminal_segments_keep_ascii_together_and_pin_other_cells() {
        use veyra_engine::terminal::{Cell, Flags};
        let mut cells = vec![Cell::default(); 6];
        for (cell, character) in cells.iter_mut().zip(['a', 'b', '界', ' ', '-', 'z']) {
            cell.c = character;
        }
        cells[2].flags = Flags::WIDE_CHAR;
        cells[3].flags = Flags::WIDE_CHAR_SPACER;
        let segments: Vec<_> = super::segments(&cells)
            .into_iter()
            .map(|(column, text, _)| (column, text))
            .collect();
        assert_eq!(
            segments,
            [(0, "ab".into()), (2, "界".into()), (4, "-z".into())]
        );
    }
    #[cfg(debug_assertions)]
    #[test]
    fn terminal_flood_counts_only_marker_interval() {
        use std::time::{Duration, Instant};
        use veyra_engine::terminal::{FloodStamps, ReaderStamp};
        let start = Instant::now();
        let first = ReaderStamp {
            at: start,
            bytes: 16,
        };
        let mut perf = super::FloodPerf::default();
        for ms in [20, 40, 90, 110] {
            perf.paint(
                FloodStamps {
                    start: Some(first),
                    done: None,
                },
                start + Duration::from_millis(ms),
            );
        }
        perf.paint(
            FloodStamps {
                start: Some(first),
                done: Some(ReaderStamp {
                    at: start + Duration::from_millis(100),
                    bytes: 16 + 50000020 + 15,
                }),
            },
            start + Duration::from_millis(120),
        );
        assert_eq!(
            perf.dump().unwrap(),
            serde_json::json!({"pty_bytes":50000020_u64,"elapsed_ms":100.,"repaints":3,"median_repaint_gap_ms\
                ":35.,"max_repaint_gap_ms":50.})
        );
    }
}

#[cfg(all(test, debug_assertions))]
mod window_tests {
    use super::{TerminalView, terminal_key_bytes};
    use crate::ui::{
        actions,
        theme::{Appearance, Theme},
    };
    use gpui::{
        AppContext, Focusable, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    };
    use std::{
        cell::Cell,
        rc::Rc,
        time::{Duration, Instant},
    };
    use veyra_engine::{Engine, domain::*, provider::env::ChildEnv};

    use veyra_engine::terminal::Dimensions;

    struct TerminalFixture {
        engine: Engine,
        runtime: tokio::runtime::Runtime,
        _scratch: crate::tests::Scratch,
        handle: veyra_engine::provider::TerminalHandle,
    }
    impl Drop for TerminalFixture {
        fn drop(&mut self) {
            self.runtime.block_on(self.engine.shutdown_wait());
        }
    }
    impl TerminalFixture {
        fn new() -> Self {
            let scratch = crate::tests::Scratch(
                std::path::Path::new("/tmp").join(format!("veyra-term-ui-{}", ThreadId::new())),
            );
            std::fs::create_dir_all(scratch.0.join("fixtures")).unwrap();
            std::fs::create_dir_all(scratch.0.join("project")).unwrap();
            let binary = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("veyra-test-cli");
            let shell = scratch.0.join("shell");
            std::fs::write(
                &shell,
                format!(
                    "#!/bin/sh\nprintf '\\033[?2004h'\nexec '{}' shell\n",
                    binary.display()
                ),
            )
            .unwrap();
            assert!(
                std::process::Command::new("/bin/chmod")
                    .arg("700")
                    .arg(&shell)
                    .status()
                    .unwrap()
                    .success()
            );
            let env = ChildEnv::isolated().with_variable("SHELL", shell.as_os_str());
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let engine = Engine::start_with_env(
                runtime.handle(),
                veyra_engine::data_dir::DataDir::at(scratch.0.join("data")).unwrap(),
                env,
            )
            .unwrap();
            let handle = engine
                .open_shell(
                    ThreadId::new(),
                    scratch.0.join("project"),
                    veyra_engine::terminal::PtySize::default(),
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !handle
                .emulator()
                .unwrap()
                .lock()
                .text()
                .iter()
                .any(|line| line == "VEYRA_TUI_READY")
            {
                assert!(Instant::now() < deadline, "shell fixture did not start");
                std::thread::sleep(Duration::from_millis(5));
            }
            Self {
                engine,
                runtime,
                _scratch: scratch,
                handle,
            }
        }
    }
    struct KeyWindow {
        terminal: gpui::Entity<TerminalView>,
        switches: Rc<Cell<usize>>,
        new_threads: Rc<Cell<usize>>,
    }
    impl Render for KeyWindow {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let switches = self.switches.clone();
            let new_threads = self.new_threads.clone();
            gpui::div()
                .size_full()
                .key_context("Veyra")
                .on_action(move |_: &actions::ToggleShellDrawer, _, _| {
                    switches.set(switches.get() + 1)
                })
                .on_action({
                    let counter = self.switches.clone();
                    move |_: &actions::NewShellTab, _, _| counter.set(counter.get() + 1)
                })
                .on_action({
                    let counter = self.switches.clone();
                    move |_: &actions::CloseShellTab, _, _| counter.set(counter.get() + 1)
                })
                .on_action(move |_: &actions::NewThread, _, _| {
                    new_threads.set(new_threads.get() + 1)
                })
                .on_action(|_: &actions::DismissNotice, _, _| {})
                .on_action(|_: &actions::ScrollTranscriptUp, _, _| {})
                .on_action(|_: &actions::ScrollTranscriptDown, _, _| {})
                .child(self.terminal.clone())
        }
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }
    #[gpui::test]
    fn terminal_debug_grid_overrides_layout(cx: &mut gpui::TestAppContext) {
        use veyra_engine::terminal::Dimensions;
        let fixture = TerminalFixture::new();
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
            cx.set_global(super::DebugTerminalGrid(Some((127, 33))));
        });
        let (root, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(Appearance::Dark, window, cx);
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new(None, window, cx);
                terminal.handle = Some(fixture.handle.clone());
                terminal.visible = true;
                terminal
            });
            window.focus(&terminal.focus_handle(cx), cx);
            KeyWindow {
                terminal,
                switches: Rc::new(Cell::new(0)),
                new_threads: Rc::new(Cell::new(0)),
            }
        });
        let view = root.read_with(cx, |root, _| root.terminal.clone());
        let theme = cx.update(|_, cx| cx.global::<Theme>().clone());
        for bounds in [theme.layout.window_min, theme.layout.window_default] {
            cx.simulate_resize(bounds);
            draw(cx);
            assert_eq!(view.read_with(cx, |view, _| view.resize), Some((127, 33)));
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                draw(cx);
                let actual = {
                    let emulator = fixture.handle.emulator().unwrap();
                    let emulator = emulator.lock();
                    (emulator.term.columns(), emulator.term.screen_lines())
                };
                if actual == (127, 33) {
                    break;
                }
                assert!(Instant::now() < deadline, "emulator grid: {actual:?}");
                std::thread::sleep(Duration::from_millis(5));
            }
            let dump = view.read_with(cx, |view, _| view.dump());
            assert_eq!(dump["columns"], 127);
            assert_eq!(dump["rows"], 33);
            assert_eq!(dump["grid"].as_array().unwrap().len(), 33);
        }
    }
    #[gpui::test]
    fn drawer_keys_win_over_terminal_element(cx: &mut gpui::TestAppContext) {
        let fixture = TerminalFixture::new();
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
        });
        let switches = Rc::new(Cell::new(0));
        let (root, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(Appearance::Dark, window, cx);
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new(None, window, cx);
                terminal.handle = Some(fixture.handle.clone());
                terminal.visible = true;
                terminal
            });
            window.focus(&terminal.focus_handle(cx), cx);
            KeyWindow {
                terminal,
                switches: switches.clone(),
                new_threads: Rc::new(Cell::new(0)),
            }
        });
        let view = root.read_with(cx, |root, _| root.terminal.clone());
        let theme = cx.update(|_, cx| cx.global::<Theme>().clone());
        cx.simulate_resize(theme.layout.window_default);
        draw(cx);
        let wide = view.read_with(cx, |view, _| view.resize.unwrap());
        cx.simulate_resize(theme.layout.window_min);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            draw(cx);
            if fixture.handle.emulator().unwrap().lock().text().len()
                == view.read_with(cx, |view, _| view.resize.unwrap().1 as usize)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let narrow = view.read_with(cx, |view, _| view.resize.unwrap());
        assert!(narrow.0 < wide.0 && narrow.1 < wide.1);
        assert!(view.read_with(cx, |view, _| view.focused));
        for keystroke in crate::control::text_keystrokes("Qé") {
            cx.update(|window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                        keystroke,
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
            });
        }
        for key in [
            "shift-enter",
            "alt-left",
            "ctrl-l",
            "escape",
            "pageup",
            "pagedown",
            "tab",
            "ctrl-c",
        ] {
            cx.simulate_keystrokes(key);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            draw(cx);
            let grid = fixture.handle.emulator().unwrap().lock().text().join(" ");
            if grid.contains("51 c3 a9 1b 0d 1b 62 0c 1b 1b 5b 35 7e 1b 5b 36 7e 09 03") {
                break;
            }
            assert!(Instant::now() < deadline, "{grid}");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(view.read_with(cx, |view, _| view.focused));
        cx.simulate_keystrokes("secondary-n");
        draw(cx);
        assert_eq!(root.read_with(cx, |root, _| root.new_threads.get()), 1);
        let grid = fixture.handle.emulator().unwrap().lock().text();
        cx.simulate_keystrokes("secondary-j");
        draw(cx);
        assert_eq!(switches.get(), 1, "window binding must beat terminal input");
        cx.simulate_keystrokes("secondary-t secondary-w");
        draw(cx);
        assert_eq!(
            switches.get(),
            3,
            "drawer bindings must beat terminal input"
        );
        assert_eq!(fixture.handle.emulator().unwrap().lock().text(), grid);
        assert_eq!(
            terminal_key_bytes(&gpui::Keystroke::parse("secondary-j").unwrap(), false),
            None
        );
        assert!(view.read_with(cx, |view, _| view.perf.repaints) > 0);
    }
    #[gpui::test]
    fn terminal_keydown_does_not_wait_for_emulator(cx: &mut gpui::TestAppContext) {
        let fixture = TerminalFixture::new();
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
        });
        let (root, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(Appearance::Dark, window, cx);
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new(None, window, cx);
                terminal.handle = Some(fixture.handle.clone());
                terminal.visible = true;
                terminal
            });
            window.focus(&terminal.focus_handle(cx), cx);
            KeyWindow {
                terminal,
                switches: Rc::new(Cell::new(0)),
                new_threads: Rc::new(Cell::new(0)),
            }
        });
        draw(cx);
        let view = root.read_with(cx, |root, _| root.terminal.clone());
        let handle = fixture.handle.clone();
        let (ready, reached) = std::sync::mpsc::sync_channel(1);
        let (release, released) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let _lock = handle.emulator().unwrap().lock();
            ready.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_millis(300));
        });
        reached.recv_timeout(Duration::from_secs(2)).unwrap();
        let elapsed = cx.update(|window, cx| {
            let start = Instant::now();
            view.update(cx, |view, cx| {
                view.key(
                    &gpui::KeyDownEvent {
                        keystroke: gpui::Keystroke::parse("up").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                )
            });
            start.elapsed()
        });
        let _ = release.send(());
        reader.join().unwrap();
        assert!(
            elapsed < Duration::from_millis(150),
            "keydown waited for emulator: {elapsed:?}"
        );
    }
    #[gpui::test]
    fn terminal_clipboard_and_history_use_bound_actions(cx: &mut gpui::TestAppContext) {
        for (name, chord) in [
            ("RestartShellTab", "secondary-enter"),
            ("PasteShell", "secondary-v"),
            ("CopyShell", "secondary-c"),
            ("ScrollShellHistoryUp", "shift-pageup"),
            ("ScrollShellHistoryDown", "shift-pagedown"),
        ] {
            assert!(
                actions::bindings()
                    .iter()
                    .any(|binding| binding.action == name
                        && binding.chord == chord
                        && binding.context == Some("VeyraTerminal")),
                "{name}"
            );
        }
        let fixture = TerminalFixture::new();
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(Appearance::Dark, window, cx);
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new(None, window, cx);
                terminal.handle = Some(fixture.handle.clone());
                terminal.visible = true;
                terminal
            });
            window.focus(&terminal.focus_handle(cx), cx);
            KeyWindow {
                terminal,
                switches: Rc::new(Cell::new(0)),
                new_threads: Rc::new(Cell::new(0)),
            }
        });
        draw(cx);
        cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("Q".into())));
        cx.simulate_keystrokes("secondary-v");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let text = fixture.handle.emulator().unwrap().lock().text().join(" ");
            if text.contains("51") {
                break;
            }
            assert!(Instant::now() < deadline, "paste did not reach PTY: {text}");
            std::thread::sleep(Duration::from_millis(5));
        }
        let expected = {
            use veyra_engine::terminal::{Column, Line, Point, Selection, SelectionType, Side};
            let mut emulator = fixture.handle.emulator().unwrap().lock();
            let mut selection = Selection::new(
                SelectionType::Simple,
                Point::new(Line(0), Column(0)),
                Side::Left,
            );
            selection.update(Point::new(Line(0), Column(4)), Side::Right);
            emulator.term.selection = Some(selection);
            emulator.term.selection_to_string().unwrap()
        };
        assert!(!expected.is_empty());
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()))
        });
        cx.simulate_keystrokes("secondary-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(expected)
        );
    }

    #[gpui::test]
    fn paste_key_observes_bracketed_mode(cx: &mut gpui::TestAppContext) {
        let fixture = TerminalFixture::new();
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(Appearance::Dark, window, cx);
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new(None, window, cx);
                terminal.handle = Some(fixture.handle.clone());
                terminal.visible = true;
                terminal
            });
            window.focus(&terminal.focus_handle(cx), cx);
            KeyWindow {
                terminal,
                switches: Rc::new(Cell::new(0)),
                new_threads: Rc::new(Cell::new(0)),
            }
        });
        draw(cx);
        assert!(
            fixture
                .handle
                .emulator()
                .unwrap()
                .lock()
                .term
                .mode()
                .contains(veyra_engine::terminal::TermMode::BRACKETED_PASTE)
        );
        cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("Q".into())));
        cx.simulate_keystrokes("secondary-v");
        let expected = [
            "1b", "5b", "32", "30", "30", "7e", "51", "1b", "5b", "32", "30", "31", "7e",
        ];
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let lines = fixture.handle.emulator().unwrap().lock().text();
            let hex: Vec<_> = lines
                .iter()
                .map(|line| line.trim())
                .filter(|line| line.len() == 2)
                .collect();
            if hex.ends_with(&expected) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "bracketed paste bytes missing: {lines:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[gpui::test]
    fn scrollback_returns_on_key(cx: &mut gpui::TestAppContext) {
        let fixture = TerminalFixture::new();
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(Appearance::Dark, window, cx);
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new(None, window, cx);
                terminal.handle = Some(fixture.handle.clone());
                terminal.visible = true;
                terminal
            });
            window.focus(&terminal.focus_handle(cx), cx);
            KeyWindow {
                terminal,
                switches: Rc::new(Cell::new(0)),
                new_threads: Rc::new(Cell::new(0)),
            }
        });
        draw(cx);
        fixture.handle.write(&[b'x'; 200]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while fixture
            .handle
            .emulator()
            .unwrap()
            .lock()
            .term
            .grid()
            .history_size()
            == 0
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        cx.simulate_keystrokes("shift-pageup");
        assert!(
            fixture
                .handle
                .emulator()
                .unwrap()
                .lock()
                .term
                .grid()
                .display_offset()
                > 0
        );
        cx.simulate_keystrokes("shift-pagedown");
        assert_eq!(
            fixture
                .handle
                .emulator()
                .unwrap()
                .lock()
                .term
                .grid()
                .display_offset(),
            0
        );
        cx.simulate_keystrokes("shift-pageup");
        assert!(
            fixture
                .handle
                .emulator()
                .unwrap()
                .lock()
                .term
                .grid()
                .display_offset()
                > 0
        );
        cx.simulate_keystrokes("up");
        assert_eq!(
            fixture
                .handle
                .emulator()
                .unwrap()
                .lock()
                .term
                .grid()
                .display_offset(),
            0
        );
    }
    #[gpui::test]
    fn hidden_tab_wakes_no_repaint(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            actions::init(cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            Theme::apply(Appearance::Dark, window, cx);
            TerminalView::new(None, window, cx)
        });
        draw(cx);
        let before = view.read_with(cx, |view, _| view.frame_counts());
        view.update_in(cx, |view, window, cx| {
            view.set_visible(false, cx);
            view.queue_frame(window, cx);
            assert!(!view.pending_frame);
        });
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.frame_counts()), before);
        view.update_in(cx, |view, window, cx| {
            view.set_visible(true, cx);
            view.queue_frame(window, cx);
            assert!(view.pending_frame);
        });
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.frame_counts()).0 > before.0);
        assert!(view.read_with(cx, |view, _| view.frame_counts()).1 > before.1);
    }
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    bytes.extend(
        text.bytes()
            .filter(|byte| *byte >= 0x20 || matches!(*byte, b'\t' | b'\n' | b'\r')),
    );
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    bytes
}

#[cfg(test)]
mod paste_tests {
    #[test]
    fn paste_strips_controls_and_embedded_bracket_markers() {
        let text = "a\x1b[201~b\x1b[200~c\0\x03\x07\x08\x0b\x0c\x1f\t\n\r";
        for bracketed in [false, true] {
            let bytes = super::paste_bytes(text, bracketed);
            let expected = b"a[201~b[200~c\t\n\r";
            if bracketed {
                assert_eq!(
                    bytes,
                    [b"\x1b[200~".as_slice(), expected, b"\x1b[201~"].concat()
                );
                assert_eq!(
                    bytes
                        .windows(6)
                        .filter(|part| *part == b"\x1b[200~")
                        .count(),
                    1
                );
                assert_eq!(
                    bytes
                        .windows(6)
                        .filter(|part| *part == b"\x1b[201~")
                        .count(),
                    1
                );
            } else {
                assert_eq!(bytes, expected);
            }
        }
    }

    #[test]
    fn paste_is_bracketed_when_asked() {
        assert_eq!(super::paste_bytes("one\ntwo", false), b"one\ntwo");
        assert_eq!(
            super::paste_bytes("one\ntwo", true),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
    }
}
