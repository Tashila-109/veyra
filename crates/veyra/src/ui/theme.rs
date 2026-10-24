use gpui_kit::component::{Theme as KitTheme, ThemeMode};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Background, BoxShadow, FontWeight, Global, Hsla, Pixels, Point, Rems, Rgba, SharedString,
    Size, Window, WindowAppearance, WindowBackgroundAppearance, linear_color_stop, linear_gradient,
    point, px, rems, rgb, rgba, size,
};
use std::{borrow::Cow, time::Duration};
use veyra_engine::terminal::{Color, NamedColor, Rgb};
use veyra_engine::{domain::WindowBackground, platform::DisplayOptions};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub requested: WindowBackground,
    pub options: DisplayOptions,
    #[cfg(debug_assertions)]
    pub overridden: bool,
}
impl Global for Look {}
impl Look {
    pub fn background(self) -> WindowBackground {
        if self.forced_by().is_some() {
            WindowBackground::Solid
        } else {
            self.requested
        }
    }
    pub fn forced_by(self) -> Option<&'static str> {
        match (
            self.options.reduce_transparency,
            self.options.increase_contrast,
        ) {
            (true, true) => Some("Reduce Transparency and Increase Contrast"),
            (true, false) => Some("Reduce Transparency"),
            (false, true) => Some("Increase Contrast"),
            _ => None,
        }
    }
    pub fn window_background(self) -> WindowBackgroundAppearance {
        match self.background() {
            WindowBackground::Translucent => WindowBackgroundAppearance::Blurred,
            WindowBackground::Solid => WindowBackgroundAppearance::Opaque,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub fn from_window(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
            _ => Self::Light,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

#[derive(Clone)]
pub struct Colors {
    pub bg: Hsla,
    pub shell: Hsla,
    pub card: Hsla,
    pub floating: Hsla,
    pub scrim: Hsla,
    pub overlay: Hsla,
    pub raised: Hsla,
    pub hover: Hsla,
    pub sel: Hsla,
    pub bubble: Hsla,
    pub code_wash: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub icon: Hsla,
    pub placeholder: Hsla,
    pub danger_wash: Hsla,
    pub warning_wash: Hsla,
    pub solid: Hsla,
    pub on_solid: Hsla,
    pub accent: Hsla,
    pub accent_selection: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub diff_add: Hsla,
    pub diff_del: Hsla,
    pub diff_add_bg: Hsla,
    pub diff_del_bg: Hsla,
    pub term_bg: Hsla,
    pub term_fg: Hsla,
    pub term_selection: Hsla,
}
impl Colors {
    pub fn dark() -> Self {
        Self {
            bg: rgb(0x0a0b0c).into(),
            shell: rgb(0x0f1011).into(),
            card: rgb(0x141416).into(),
            floating: rgb(0x1e1d20).into(),
            scrim: rgba(0x00000080).into(),
            overlay: rgb(0x1a1b1c).into(),
            raised: rgb(0x222224).into(),
            hover: rgba(0xffffff0b).into(),
            sel: rgba(0xffffff13).into(),
            bubble: rgba(0xffffff0f).into(),
            code_wash: rgba(0xffffff0f).into(),
            border: rgba(0xffffff11).into(),
            border_strong: rgba(0xffffff21).into(),
            text: rgb(0xe7e8ea).into(),
            text_muted: rgb(0xaaabad).into(),
            text_faint: rgb(0x909193).into(),
            icon: rgb(0xaaabad).into(),
            placeholder: rgb(0xaaabad).into(),
            danger_wash: wash(rgb(0xf88a82).into(), rgb(0x0a0b0c).into(), 19. / 255.),
            warning_wash: wash(rgb(0xe6ba60).into(), rgb(0x0a0b0c).into(), 19. / 255.),
            solid: rgb(0xf1f1f3).into(),
            on_solid: rgb(0x0a0b0c).into(),
            accent: rgb(0x8b7cf6).into(),
            accent_selection: rgba(0x8b7cf652).into(),
            danger: rgb(0xf88a82).into(),
            warning: rgb(0xe6ba60).into(),
            success: rgb(0x7bc897).into(),
            diff_add: rgb(0x7bc897).into(),
            diff_del: rgb(0xf88a82).into(),
            diff_add_bg: rgba(0x7bc89711).into(),
            diff_del_bg: rgba(0xf88a8211).into(),
            term_bg: rgb(0x060608).into(),
            term_fg: rgb(0xe7e8ea).into(),
            term_selection: rgba(0xffffff2d).into(),
        }
    }
    pub fn light() -> Self {
        Self {
            bg: rgb(0xffffff).into(),
            shell: rgb(0xf6f6f8).into(),
            card: rgb(0xffffff).into(),
            floating: rgb(0xffffff).into(),
            scrim: rgba(0x00000050).into(),
            overlay: rgb(0xffffff).into(),
            raised: rgb(0xffffff).into(),
            hover: rgba(0x00000008).into(),
            sel: rgba(0x0000000e).into(),
            bubble: rgba(0x0000000a).into(),
            code_wash: rgba(0x0000000b).into(),
            border: rgba(0x00000017).into(),
            border_strong: rgba(0x0000002b).into(),
            text: rgb(0x272729).into(),
            text_muted: rgb(0x5c5d5e).into(),
            text_faint: rgb(0x656667).into(),
            icon: rgb(0x5c5d5e).into(),
            placeholder: rgb(0x5c5d5e).into(),
            danger_wash: wash(rgb(0xa82d2e).into(), rgb(0xffffff).into(), 14. / 255.),
            warning_wash: wash(rgb(0x7f5d04).into(), rgb(0xffffff).into(), 14. / 255.),
            solid: rgb(0x1d1d1f).into(),
            on_solid: rgb(0xffffff).into(),
            accent: rgb(0x5b43e8).into(),
            accent_selection: rgba(0x5b43e838).into(),
            danger: rgb(0xa82d2e).into(),
            warning: rgb(0x7f5d04).into(),
            success: rgb(0x267147).into(),
            diff_add: rgb(0x267147).into(),
            diff_del: rgb(0xa82d2e).into(),
            diff_add_bg: rgba(0x2671470e).into(),
            diff_del_bg: rgba(0xa82d2e0e).into(),
            term_bg: rgb(0xfbfcfe).into(),
            term_fg: rgb(0x2f2f31).into(),
            term_selection: rgba(0x00000021).into(),
        }
    }
}

fn wash(foreground: Hsla, background: Hsla, alpha: f32) -> Hsla {
    let fg = foreground.to_rgb();
    let bg = background.to_rgb();
    Rgba {
        r: fg.r * alpha + bg.r * (1. - alpha),
        g: fg.g * alpha + bg.g * (1. - alpha),
        b: fg.b * alpha + bg.b * (1. - alpha),
        a: 1.,
    }
    .into()
}

#[derive(Clone)]
pub struct TerminalPalette {
    pub bg: Hsla,
    pub fg: Hsla,
    pub selection: Hsla,
    pub ansi: [Hsla; 16],
}
#[derive(Clone)]
pub struct Shadows {
    pub raised: Vec<BoxShadow>,
    pub floating: Vec<BoxShadow>,
}
#[derive(Clone, Copy)]
pub struct TypeRole {
    pub size: Rems,
    pub line_height: Rems,
    pub weight: FontWeight,
}
#[derive(Clone)]
pub struct TypeScale {
    pub base: Pixels,
    pub ui_font: SharedString,
    pub mono_font: SharedString,
    pub meta: TypeRole,
    pub micro: TypeRole,
    pub label: TypeRole,
    pub ui: TypeRole,
    pub read: TypeRole,
    pub h3: TypeRole,
    pub h2: TypeRole,
    pub h1: TypeRole,
    pub code: TypeRole,
    pub diff: TypeRole,
    pub terminal: TypeRole,
}
#[derive(Clone)]
pub struct Spacing(pub [Pixels; 10]);
#[derive(Clone)]
pub struct Radii {
    pub inline: Pixels,
    pub control: Pixels,
    pub track: Pixels,
    pub card: Pixels,
    pub floating: Pixels,
    pub large: Pixels,
    pub full: Pixels,
}
#[derive(Clone)]
pub struct Transition {
    pub duration: Duration,
    pub curve: [f32; 4],
}
#[derive(Clone)]
pub struct Motion {
    pub hover: Transition,
    pub press: Transition,
    pub collapse: Transition,
    pub drawer: Transition,
    pub menu: [Transition; 2],
    pub resize: Transition,
    pub stream_reveal: Transition,
    pub spinner: Transition,
}
#[derive(Clone)]
pub struct Layout {
    pub window_default: Size<Pixels>,
    pub window_min: Size<Pixels>,
    pub header: Pixels,
    pub sidebar_min: Pixels,
    pub sidebar_default: Pixels,
    pub sidebar_max: Pixels,
    pub sidebar_step: Pixels,
    pub traffic_lights: Point<Pixels>,
    pub border: Pixels,
    pub project_row: Pixels,
    pub thread_row: Pixels,
    pub agent_row: Pixels,
    pub agent_margin: Pixels,
    pub agent_padding: Pixels,
    pub guide: Pixels,
    pub transcript_min: Pixels,
    pub transcript_default: Pixels,
    pub transcript_max: Pixels,
    pub gutter: Pixels,
    pub composer_max: Pixels,
    pub composer_min_height: Pixels,
    pub composer_max_height: Pixels,
    pub composer_toolbar: Pixels,
    pub composer_input_shrink: f32,
    pub dock_question_min_height: Pixels,
    pub fade: Pixels,
    pub terminal_padding: Size<Pixels>,
    pub terminal_hint: Pixels,
    pub segment_height: Pixels,
    pub segment_button: Pixels,
    pub segment_padding: Pixels,
    pub icon_button: Pixels,
    pub small_icon_button: Pixels,
    pub icon: Pixels,
    pub small_icon: Pixels,
    pub icon_stroke: f32,
    pub focus_ring: Pixels,
    pub focus_inset: Pixels,
    pub row_gap: Pixels,
    pub group_gap: Pixels,
    pub resize_band: Pixels,
    pub palette_width: Pixels,
    pub palette_max_height: Pixels,
    pub toast_width: Pixels,
    pub toast_inset: Pixels,
    pub toasts_shown: usize,
    pub row_inset: Pixels,
    pub control_gap: Pixels,
    pub section_gap: Pixels,
    pub floating_inset: Pixels,
    pub list_gap: Pixels,
    pub status_glyph: Pixels,
    pub provider_mark: Pixels,
    pub drawer_tabs: Pixels,
    pub drawer_min: Pixels,
    pub drawer_default: Pixels,
    pub drawer_tab: Pixels,
}
#[derive(Clone)]
pub struct Theme {
    pub appearance: Appearance,
    pub colors: Colors,
    pub terminal: TerminalPalette,
    pub shadows: Shadows,
    pub text: TypeScale,
    pub space: Spacing,
    pub radius: Radii,
    pub layout: Layout,
    pub motion: Motion,
    pub look: Look,
    pub glass_coverage: f32,
}
impl Global for Theme {}

const TYPE_BASE: f32 = 16.;

fn role(size: f32, line: f32, weight: FontWeight) -> TypeRole {
    TypeRole {
        size: rems(size / TYPE_BASE),
        line_height: rems(line / TYPE_BASE),
        weight,
    }
}
fn transition(ms: u64, curve: [f32; 4]) -> Transition {
    Transition {
        duration: Duration::from_millis(ms),
        curve,
    }
}
fn shadow(y: f32, blur: f32, color: u32) -> BoxShadow {
    BoxShadow {
        color: rgba(color).into(),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}
impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        let dark = appearance == Appearance::Dark;
        let colors = if dark {
            Colors::dark()
        } else {
            Colors::light()
        };
        let ansi = if dark {
            [
                0x28292a, 0xf08f87, 0x70cb92, 0xe6ba60, 0x69a9ed, 0xd497df, 0x3ccdde, 0xd2d2d4,
                0x828385, 0xfebab3, 0x91ecb2, 0xffdd9c, 0x97c9fe, 0xf3b9fe, 0x69edff, 0xf1f2f4,
            ]
        } else {
            [
                0x202122, 0x954440, 0x1a7244, 0x7f5d04, 0x266099, 0x7c4986, 0x056d78, 0x373839,
                0x717173, 0x833431, 0x056136, 0x6c4e04, 0x145088, 0x6c3a75, 0x045c66, 0x161718,
            ]
        };
        Self {
            appearance,
            terminal: TerminalPalette {
                bg: colors.term_bg,
                fg: colors.term_fg,
                selection: colors.term_selection,
                ansi: ansi.map(|c| rgb(c).into()),
            },
            colors,
            shadows: Shadows {
                raised: if dark {
                    vec![shadow(4., 12., 0x00000044)]
                } else {
                    vec![shadow(1., 2., 0x0000001a)]
                },
                floating: if dark {
                    vec![shadow(8., 24., 0x00000066)]
                } else {
                    vec![shadow(8., 24., 0x0000001f), shadow(1., 2., 0x00000014)]
                },
            },
            text: TypeScale {
                base: px(TYPE_BASE),
                ui_font: "Geist".into(),
                mono_font: "Geist Mono".into(),
                meta: role(11., 16., FontWeight::NORMAL),
                micro: role(10., 12., FontWeight::MEDIUM),
                label: role(12., 16., FontWeight::NORMAL),
                ui: role(13., 20., FontWeight::NORMAL),
                read: role(14., 24., FontWeight::NORMAL),
                h3: role(16., 24., FontWeight::SEMIBOLD),
                h2: role(18., 28., FontWeight::SEMIBOLD),
                h1: role(20., 28., FontWeight::SEMIBOLD),
                code: role(12., 20., FontWeight::NORMAL),
                diff: role(12., 20., FontWeight::NORMAL),
                terminal: role(13., 17., FontWeight::NORMAL),
            },
            space: Spacing([2., 4., 6., 8., 12., 16., 20., 24., 32., 40.].map(px)),
            radius: Radii {
                inline: px(4.),
                control: px(6.),
                track: px(8.),
                card: px(8.),
                floating: px(12.),
                large: px(16.),
                full: px(9999.),
            },
            motion: Motion {
                hover: transition(100, [0.25, 0.1, 0.25, 1.]),
                press: transition(100, [0.25, 0.1, 0.25, 1.]),
                collapse: transition(200, [0., 0., 0.58, 1.]),
                drawer: transition(200, [0., 0., 0.58, 1.]),
                menu: [
                    transition(200, [0., 0., 0.58, 1.]),
                    transition(100, [0.42, 0., 1., 1.]),
                ],
                resize: transition(200, [0., 0., 0.58, 1.]),
                stream_reveal: transition(300, [0., 0., 0.58, 1.]),
                spinner: transition(1000, [0., 0., 1., 1.]),
            },
            layout: Layout {
                window_default: size(px(1280.), px(800.)),
                window_min: size(px(760.), px(560.)),
                header: px(44.),
                sidebar_min: px(200.),
                sidebar_default: px(240.),
                sidebar_max: px(448.),
                sidebar_step: px(16.),
                traffic_lights: point(px(16.), px(16.)),
                border: px(1.),
                project_row: px(28.),
                thread_row: px(32.),
                agent_row: px(28.),
                agent_margin: px(14.),
                agent_padding: px(8.),
                guide: px(1.),
                transcript_min: px(480.),
                transcript_default: px(752.),
                transcript_max: px(752.),
                gutter: px(40.),
                composer_max: px(784.),
                composer_min_height: px(48.),
                composer_max_height: px(240.),
                composer_toolbar: px(40.),
                composer_input_shrink: 1000.,
                dock_question_min_height: px(44.),
                fade: px(24.),
                terminal_padding: size(px(14.), px(20.)),
                terminal_hint: px(30.),
                segment_height: px(28.),
                segment_button: px(24.),
                segment_padding: px(2.),
                icon_button: px(28.),
                small_icon_button: px(24.),
                icon: px(16.),
                small_icon: px(14.),
                icon_stroke: 2. / 24.,
                focus_ring: px(2.),
                focus_inset: px(1.),
                row_gap: px(2.),
                group_gap: px(12.),
                resize_band: px(8.),
                palette_width: px(560.),
                palette_max_height: px(344.),
                toast_width: px(360.),
                toast_inset: px(16.),
                toasts_shown: 3,
                row_inset: px(6.),
                control_gap: px(8.),
                section_gap: px(24.),
                floating_inset: px(4.),
                list_gap: px(2.),
                status_glyph: px(16.),
                provider_mark: px(14.),
                drawer_tabs: px(32.),
                drawer_min: px(134.),
                drawer_default: px(236.),
                drawer_tab: px(72.),
            },
            look: Look::default(),
            glass_coverage: if dark { 0.96 } else { 0.98 },
        }
    }
    pub fn apply(appearance: Appearance, window: &mut Window, cx: &mut App) {
        KitTheme::change(
            match appearance {
                Appearance::Dark => ThemeMode::Dark,
                Appearance::Light => ThemeMode::Light,
            },
            Some(window),
            cx,
        );
        let look = cx.try_global::<Look>().copied().unwrap_or_default();
        let theme = Self::with_look(appearance, look);
        window.set_background_appearance(look.window_background());
        cx.set_reduce_motion(look.options.reduce_motion);
        KitTheme::update(cx, |kit| {
            let c = &theme.colors;
            kit.background = match look.background() {
                WindowBackground::Translucent => c.bg.opacity(0.),
                WindowBackground::Solid => c.bg,
            };
            kit.foreground = c.text;
            kit.muted_foreground = c.text_muted;
            kit.sidebar = c.shell;
            kit.sidebar_foreground = c.text;
            kit.popover = c.floating;
            kit.popover_foreground = c.text;
            kit.secondary = c.raised;
            kit.secondary_foreground = c.text;
            kit.muted = c.card;
            kit.input = c.card;
            kit.border = c.border;
            kit.accent = c.hover;
            kit.accent_foreground = c.text;
            kit.primary = c.solid;
            kit.primary_foreground = c.on_solid;
            kit.primary_hover = c.solid;
            kit.primary_active = c.solid;
            kit.ring = c.accent;
            kit.caret = c.accent;
            kit.selection = c.accent_selection;
            kit.link = c.text;
            kit.danger = c.danger;
            kit.warning = c.warning;
            kit.success = c.success;
            kit.font_family = theme.text.ui_font.clone();
            kit.mono_font_family = theme.text.mono_font.clone();
            kit.font_size = theme.text.base;
            kit.mono_font_size = theme.text.code.size.to_pixels(theme.text.base);
            kit.radius = theme.radius.floating;
            kit.shadow = true;
        });
        cx.set_global(theme);
        cx.refresh_windows();
    }

    pub fn with_look(appearance: Appearance, look: Look) -> Self {
        let mut theme = Self::new(appearance);
        theme.look = look;
        if look.options.increase_contrast {
            theme.colors.border = match appearance {
                Appearance::Dark => rgb(0x737477).into(),
                Appearance::Light => rgb(0x777779).into(),
            };
            theme.colors.border_strong = theme.colors.border;
            theme.colors.text_muted = theme.colors.text;
            theme.colors.text_faint = theme.colors.text;
            theme.colors.icon = theme.colors.text_muted;
            theme.colors.placeholder = theme.colors.text_muted;
        }
        if look.options.reduce_motion {
            for token in theme.motion.tokens_mut() {
                token.duration = Duration::ZERO;
            }
        }
        theme
    }

    pub fn sidebar_fill(&self) -> Hsla {
        let mut color = self.colors.shell;
        if self.look.background() == WindowBackground::Translucent {
            color.a = self.glass_coverage;
        }
        color
    }

    pub fn focus_ring(&self) -> impl IntoElement {
        gpui_kit::div()
            .debug_selector(|| "focus-ring".into())
            .absolute()
            .inset(self.layout.focus_inset)
            .border(self.layout.focus_ring)
            .border_color(self.colors.accent)
            .rounded(self.radius.control - self.layout.focus_inset)
    }

    pub fn plate(&self, fill: Hsla) -> Background {
        let wash = self.colors.hover.to_rgb();
        let base = fill.to_rgb();
        let top: Hsla = Rgba {
            r: wash.r * wash.a + base.r * (1. - wash.a),
            g: wash.g * wash.a + base.g * (1. - wash.a),
            b: wash.b * wash.a + base.b * (1. - wash.a),
            a: 1.,
        }
        .into();
        linear_gradient(
            180.,
            linear_color_stop(top, 0.),
            linear_color_stop(fill, 0.5),
        )
    }

    pub fn plate_highlight(&self) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: self.colors.hover,
            offset: point(Pixels::ZERO, self.layout.border),
            blur_radius: Pixels::ZERO,
            spread_radius: Pixels::ZERO,
            inset: true,
        }]
    }

    pub fn composer_fade(&self) -> Background {
        linear_gradient(
            180.,
            linear_color_stop(self.colors.bg.opacity(0.), 0.),
            linear_color_stop(self.colors.bg, 1.),
        )
    }
}
impl Motion {
    fn tokens_mut(&mut self) -> [&mut Transition; 9] {
        let Self {
            hover,
            press,
            collapse,
            drawer,
            menu: [menu_in, menu_out],
            resize,
            stream_reveal,
            spinner,
        } = self;
        [
            hover,
            press,
            collapse,
            drawer,
            menu_in,
            menu_out,
            resize,
            stream_reveal,
            spinner,
        ]
    }
}
pub fn register_fonts(cx: &mut App) -> anyhow::Result<()> {
    cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-Italic.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-Medium.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-SemiBold.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/GeistMono-Regular.ttf").as_slice()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Rgba;
    fn over(fg: Rgba, bg: Rgba) -> Rgba {
        Rgba {
            r: fg.r * fg.a + bg.r * (1. - fg.a),
            g: fg.g * fg.a + bg.g * (1. - fg.a),
            b: fg.b * fg.a + bg.b * (1. - fg.a),
            a: 1.,
        }
    }
    fn luminance(c: Rgba) -> f32 {
        let linear = |v: f32| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        linear(c.r) * 0.2126 + linear(c.g) * 0.7152 + linear(c.b) * 0.0722
    }
    fn contrast(fg: Hsla, bg: Hsla, base: Hsla) -> f32 {
        let bg = over(bg.to_rgb(), base.to_rgb());
        let f = luminance(over(fg.to_rgb(), bg));
        let b = luminance(bg);
        (f.max(b) + 0.05) / (f.min(b) + 0.05)
    }
    fn token_call_count(source: &str) -> usize {
        let bytes = source.as_bytes();
        let mut index = 0;
        let mut block = 0;
        let mut line = false;
        let mut quoted = false;
        let mut count = 0;
        while index < bytes.len() {
            let rest = &bytes[index..];
            if line {
                line = bytes[index] != b'\n';
            } else if block > 0 {
                if rest.starts_with(b"/*") {
                    block += 1;
                    index += 1;
                } else if rest.starts_with(b"*/") {
                    block -= 1;
                    index += 1;
                }
            } else if quoted && bytes[index] == b'\\' {
                index += 1;
            } else if bytes[index] == b'"' {
                quoted = !quoted;
            } else if !quoted && rest.starts_with(b"//") {
                line = true;
                index += 1;
            } else if !quoted && rest.starts_with(b"/*") {
                block = 1;
                index += 1;
            } else if [b"px(".as_slice(), b"rgb(", b"hsla("]
                .iter()
                .any(|needle| rest.starts_with(needle))
            {
                count += 1;
            }
            index += 1;
        }
        count
    }
    #[test]
    fn theme_tokens_only() {
        use std::{fs, path::PathBuf};
        let mut folders = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut count = 0;
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    && path.file_name().unwrap() != "theme.rs"
                {
                    count += token_call_count(&fs::read_to_string(path).unwrap());
                }
            }
        }
        const INITIAL_TOKEN_CALLS: usize = 7;
        assert!(
            count <= INITIAL_TOKEN_CALLS,
            "theme token calls increased from the starting count of 7 to {count}"
        );
        assert_eq!(
            token_call_count("px(1) // rgb(2)\n/* hsla(3) /* px(4) */ */ rgb(5)"),
            2
        );
        assert_eq!(token_call_count("\"//\" px(1)"), 1);
    }
    #[test]
    fn display_flags_force_solid() {
        for requested in [WindowBackground::Translucent, WindowBackground::Solid] {
            for reduce_transparency in [false, true] {
                for increase_contrast in [false, true] {
                    let look = Look {
                        requested,
                        options: DisplayOptions {
                            reduce_transparency,
                            increase_contrast,
                            reduce_motion: true,
                        },
                        ..Default::default()
                    };
                    assert_eq!(
                        look.background(),
                        if reduce_transparency || increase_contrast {
                            WindowBackground::Solid
                        } else {
                            requested
                        }
                    );
                    assert_eq!(
                        look.forced_by().is_some(),
                        reduce_transparency || increase_contrast
                    );
                    assert_eq!(look.requested, requested);
                }
            }
        }
    }

    #[gpui::test]
    fn reduce_motion_snaps_every_token(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            gpui_kit::init(cx);
            for appearance in [Appearance::Dark, Appearance::Light] {
                let mut normal = Theme::new(appearance);
                assert!(
                    normal
                        .motion
                        .tokens_mut()
                        .iter()
                        .all(|token| !token.duration.is_zero())
                );
                let mut reduced = Theme::with_look(
                    appearance,
                    Look {
                        options: DisplayOptions {
                            reduce_motion: true,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
                assert!(
                    reduced
                        .motion
                        .tokens_mut()
                        .iter()
                        .all(|token| token.duration.is_zero())
                );
                assert_eq!(reduced.motion.spinner.duration, Duration::ZERO);
                for reduce_motion in [false, true, false] {
                    cx.set_global(Look {
                        options: DisplayOptions {
                            reduce_motion,
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                    Theme::apply(appearance, window, cx);
                    assert_eq!(cx.reduce_motion(), reduce_motion);
                }
            }
        });
    }

    #[gpui::test]
    fn theme_apply_keeps_blurred_in_translucent(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            gpui_kit::init(cx);
            let look = Look::default();
            cx.set_global(look);
            for appearance in [Appearance::Dark, Appearance::Light, Appearance::Dark] {
                Theme::apply(appearance, window, cx);
                assert_eq!(
                    cx.global::<Theme>().look.window_background(),
                    WindowBackgroundAppearance::Blurred
                );
                assert_eq!(cx.global::<Theme>().appearance, appearance);
                assert_eq!(KitTheme::global(cx).background.a, 0.);
                assert!(KitTheme::global(cx).shadow);
                assert_eq!(
                    KitTheme::global(cx).popover,
                    cx.global::<Theme>().colors.floating
                );
            }
            for look in [
                Look {
                    requested: WindowBackground::Solid,
                    ..Default::default()
                },
                Look {
                    options: DisplayOptions {
                        reduce_transparency: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Look {
                    options: DisplayOptions {
                        increase_contrast: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ] {
                cx.set_global(look);
                for appearance in [Appearance::Dark, Appearance::Light] {
                    Theme::apply(appearance, window, cx);
                    assert_eq!(KitTheme::global(cx).background.a, 1.);
                    assert_eq!(
                        cx.global::<Theme>().look.window_background(),
                        WindowBackgroundAppearance::Opaque
                    );
                }
            }
        });
    }

    fn sidebar_contrast_passes(theme: &Theme, coverage: f32) -> bool {
        let c = &theme.colors;
        let desktop = match theme.appearance {
            Appearance::Dark => rgb(0xffffff),
            Appearance::Light => rgb(0x000000),
        };
        let mut painted = theme.clone();
        painted.glass_coverage = coverage;
        let shell: Hsla = over(painted.sidebar_fill().to_rgb(), desktop).into();
        [
            shell,
            Hsla::from(over(c.hover.to_rgb(), shell.to_rgb())),
            Hsla::from(over(c.sel.to_rgb(), shell.to_rgb())),
        ]
        .into_iter()
        .all(|background| {
            [
                (c.text, 4.5),
                (c.text_muted, 4.5),
                (c.text_faint, 4.5),
                (c.accent, 3.),
                (c.danger, 3.),
                (c.success, 3.),
                (c.warning, 3.),
            ]
            .into_iter()
            .all(|(foreground, floor)| contrast(foreground, background, shell) >= floor)
        })
    }

    #[test]
    fn theme_contrast() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let t = Theme::new(appearance);
            let coverage = (35..=50)
                .map(|step| step as f32 * 0.02)
                .find(|coverage| sidebar_contrast_passes(&t, *coverage))
                .expect("no passing glass tint");
            println!("{} glass_coverage={coverage:.2}", appearance.name());
            assert!(
                (t.glass_coverage - coverage).abs() < 0.001,
                "{appearance:?} stored glass coverage {} should be {coverage:.2}",
                t.glass_coverage
            );
            let high = Theme::with_look(
                appearance,
                Look {
                    options: DisplayOptions {
                        increase_contrast: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            for background in [
                high.colors.bg,
                high.colors.card,
                high.colors.shell,
                high.colors.floating,
            ] {
                assert!(contrast(high.colors.border, background, background) >= 3.);
                for foreground in [high.colors.text_muted, high.colors.text_faint] {
                    assert!(contrast(foreground, background, background) >= 4.5);
                }
            }
            let c = &t.colors;
            for theme in [&t, &high] {
                let c = &theme.colors;
                assert_eq!(c.icon, c.text_muted);
                assert_eq!(c.placeholder, c.text_muted);
                for background in [c.bg, c.shell, c.card, c.floating] {
                    assert!(contrast(c.icon, background, background) >= 3.);
                }
                for background in [c.danger_wash, c.warning_wash] {
                    for foreground in [c.text, c.text_muted] {
                        assert!(contrast(foreground, background, c.bg) >= 4.5);
                    }
                }
            }
            let pairs = [
                ("text/bg", c.text, c.bg, c.bg, 4.5),
                ("text/shell", c.text, c.shell, c.bg, 4.5),
                ("text/card", c.text, c.card, c.bg, 4.5),
                ("muted/bg", c.text_muted, c.bg, c.bg, 4.5),
                ("muted/shell", c.text_muted, c.shell, c.bg, 4.5),
                ("muted/card", c.text_muted, c.card, c.bg, 4.5),
                ("faint/bg", c.text_faint, c.bg, c.bg, 4.5),
                ("faint/shell", c.text_faint, c.shell, c.bg, 4.5),
                ("faint/card", c.text_faint, c.card, c.bg, 4.5),
                ("muted/selected", c.text_muted, c.sel, c.shell, 4.5),
                ("on_solid/solid", c.on_solid, c.solid, c.bg, 4.5),
                ("terminal", c.term_fg, c.term_bg, c.bg, 4.5),
                ("ansi8", t.terminal.ansi[8], c.term_bg, c.bg, 3.),
                ("accent/bg", c.accent, c.bg, c.bg, 3.),
                ("accent/shell", c.accent, c.shell, c.bg, 3.),
                ("warning", c.warning, c.shell, c.bg, 3.),
                ("success", c.success, c.shell, c.bg, 3.),
                ("danger", c.danger, c.shell, c.bg, 3.),
                ("diff_add/card", c.diff_add, c.card, c.bg, 4.5),
                ("diff_del/card", c.diff_del, c.card, c.bg, 4.5),
                ("diff_add/tint", c.diff_add, c.diff_add_bg, c.card, 4.5),
                ("diff_del/tint", c.diff_del, c.diff_del_bg, c.card, 4.5),
                ("muted/add", c.text_muted, c.diff_add_bg, c.card, 4.5),
                ("muted/del", c.text_muted, c.diff_del_bg, c.card, 4.5),
            ];
            for (name, fg, bg, base, min) in pairs {
                let ratio = contrast(fg, bg, base);
                assert!(ratio >= min, "{appearance:?} {name} {ratio:.2} < {min}");
            }
        }
    }
}

impl TerminalPalette {
    pub(super) fn resolve(&self, value: Color, palette: &[Option<Rgb>]) -> Hsla {
        let index = match value {
            Color::Spec(value) => {
                return rgb(value.r as u32 * 65536 + value.g as u32 * 256 + value.b as u32).into();
            }
            Color::Indexed(index) => index as usize,
            Color::Named(name) => name as usize,
        };
        if let Some(Some(value)) = palette.get(index) {
            return rgb(value.r as u32 * 65536 + value.g as u32 * 256 + value.b as u32).into();
        }
        match index {
            0..16 => self.ansi[index],
            16..256 => {
                let color = veyra_engine::terminal::xterm_color(index as u8);
                rgb(color.r as u32 * 65536 + color.g as u32 * 256 + color.b as u32).into()
            }
            n if n == NamedColor::Background as usize => self.bg,
            n if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&n) => {
                self.ansi[n - NamedColor::DimBlack as usize]
            }
            _ => self.fg,
        }
    }

    pub(super) fn engine_colors(&self) -> veyra_engine::terminal::Colors {
        let convert = |color: Hsla| {
            let color = Rgba::from(color);
            Rgb {
                r: (color.r * 255.).round() as u8,
                g: (color.g * 255.).round() as u8,
                b: (color.b * 255.).round() as u8,
            }
        };
        veyra_engine::terminal::Colors {
            foreground: convert(self.fg),
            background: convert(self.bg),
        }
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::{Appearance, Theme};
    use veyra_engine::terminal::{Color, NamedColor, Rgb};
    #[test]
    fn terminal_theme_answers_and_paints_light_and_dark() {
        for (appearance, foreground, background) in [
            (
                Appearance::Dark,
                Rgb {
                    r: 231,
                    g: 232,
                    b: 234,
                },
                Rgb { r: 6, g: 6, b: 8 },
            ),
            (
                Appearance::Light,
                Rgb {
                    r: 47,
                    g: 47,
                    b: 49,
                },
                Rgb {
                    r: 251,
                    g: 252,
                    b: 254,
                },
            ),
        ] {
            let theme = Theme::new(appearance);
            let colors = theme.terminal.engine_colors();
            assert_eq!(colors.foreground, foreground);
            assert_eq!(colors.background, background);
            assert_eq!(
                theme
                    .terminal
                    .resolve(Color::Named(NamedColor::Foreground), &[]),
                theme.terminal.fg
            );
            assert_eq!(
                theme.terminal.resolve(Color::Indexed(3), &[]),
                theme.terminal.ansi[3]
            );
            assert_eq!(
                theme
                    .terminal
                    .resolve(Color::Spec(Rgb { r: 0, g: 255, b: 0 }), &[]),
                super::rgb(0x00ff00).into()
            );
            assert_eq!(
                theme.terminal.resolve(Color::Indexed(196), &[]),
                super::rgb(0xff0000).into()
            );
        }
    }
}
