use gpui_kit::component::{Theme as KitTheme, ThemeMode};
use gpui_kit::{
    App, BoxShadow, FontWeight, Global, Hsla, Pixels, Rems, SharedString, Size, Window,
    WindowAppearance, point, px, rems, rgb, rgba, size,
};
use std::{borrow::Cow, time::Duration};

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
    pub view_switch: Transition,
    pub menu: [Transition; 2],
    pub resize: Transition,
    pub stream_reveal: Transition,
    pub stream_caret: Size<Pixels>,
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
    pub traffic_lights: Size<Pixels>,
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
    pub icon_stroke: Pixels,
    pub status_glyph: Pixels,
    pub provider_mark: Pixels,
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
                    vec![]
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
                label: role(12., 16., FontWeight::NORMAL),
                ui: role(13., 20., FontWeight::NORMAL),
                read: role(14., 24., FontWeight::NORMAL),
                h3: role(16., 24., FontWeight::SEMIBOLD),
                h2: role(18., 28., FontWeight::MEDIUM),
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
                large: px(16.),
                full: px(9999.),
            },
            motion: Motion {
                hover: transition(100, [0.25, 0.1, 0.25, 1.]),
                press: transition(100, [0.25, 0.1, 0.25, 1.]),
                collapse: transition(200, [0., 0., 0.58, 1.]),
                view_switch: transition(200, [0.25, 0.1, 0.25, 1.]),
                menu: [
                    transition(200, [0., 0., 0.58, 1.]),
                    transition(100, [0.42, 0., 1., 1.]),
                ],
                resize: transition(200, [0., 0., 0.58, 1.]),
                stream_reveal: transition(300, [0., 0., 0.58, 1.]),
                stream_caret: size(px(2.), px(15.)),
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
                traffic_lights: size(px(16.), px(16.)),
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
                icon_stroke: px(1.5),
                status_glyph: px(12.),
                provider_mark: px(14.),
            },
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
        let theme = Self::new(appearance);
        KitTheme::update(cx, |kit| {
            let c = &theme.colors;
            kit.background = c.bg;
            kit.foreground = c.text;
            kit.muted_foreground = c.text_muted;
            kit.sidebar = c.shell;
            kit.sidebar_foreground = c.text;
            kit.popover = c.overlay;
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
            kit.radius = theme.radius.control;
            kit.shadow = false;
        });
        cx.set_global(theme);
        cx.refresh_windows();
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
    #[test]
    fn theme_contrast() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let t = Theme::new(appearance);
            let c = &t.colors;
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
