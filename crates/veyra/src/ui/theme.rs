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
            bg: rgb(0x0e0e10).into(),
            shell: rgb(0x131315).into(),
            card: rgb(0x161619).into(),
            overlay: rgb(0x1c1c20).into(),
            raised: rgb(0x26262b).into(),
            hover: rgba(0xffffff0f).into(),
            sel: rgba(0xffffff17).into(),
            bubble: rgba(0xffffff12).into(),
            code_wash: rgba(0xffffff12).into(),
            border: rgba(0xffffff14).into(),
            border_strong: rgba(0xffffff29).into(),
            text: rgb(0xe8e8ea).into(),
            text_muted: rgb(0xa3a3aa).into(),
            text_faint: rgb(0x8a8a92).into(),
            solid: rgb(0xececef).into(),
            on_solid: rgb(0x0e0e10).into(),
            accent: rgb(0x8b7cf6).into(),
            accent_selection: rgba(0x8b7cf652).into(),
            danger: rgb(0xf87171).into(),
            warning: rgb(0xfbbf24).into(),
            success: rgb(0x34d399).into(),
            diff_add: rgb(0x34d399).into(),
            diff_del: rgb(0xf87171).into(),
            diff_add_bg: rgba(0x34d3991a).into(),
            diff_del_bg: rgba(0xf871711a).into(),
            term_bg: rgb(0x0a0a0c).into(),
            term_fg: rgb(0xe8e8ea).into(),
            term_selection: rgba(0xffffff38).into(),
        }
    }
    pub fn light() -> Self {
        Self {
            bg: rgb(0xffffff).into(),
            shell: rgb(0xf5f5f7).into(),
            card: rgb(0xffffff).into(),
            overlay: rgb(0xffffff).into(),
            raised: rgb(0xffffff).into(),
            hover: rgba(0x0000000a).into(),
            sel: rgba(0x0000000f).into(),
            bubble: rgba(0x0000000a).into(),
            code_wash: rgba(0x0000000d).into(),
            border: rgba(0x0000001a).into(),
            border_strong: rgba(0x0000002e).into(),
            text: rgb(0x26262b).into(),
            text_muted: rgb(0x5f5f67).into(),
            text_faint: rgb(0x6b6b73).into(),
            solid: rgb(0x1f1f23).into(),
            on_solid: rgb(0xffffff).into(),
            accent: rgb(0x5b43e8).into(),
            accent_selection: rgba(0x5b43e838).into(),
            danger: rgb(0xdc2626).into(),
            warning: rgb(0xb45309).into(),
            success: rgb(0x15803d).into(),
            diff_add: rgb(0x15803d).into(),
            diff_del: rgb(0xb91c1c).into(),
            diff_add_bg: rgba(0x15803d14).into(),
            diff_del_bg: rgba(0xdc262614).into(),
            term_bg: rgb(0xfafafa).into(),
            term_fg: rgb(0x303035).into(),
            term_selection: rgba(0x00000029).into(),
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
    pub drawer_min: Pixels,
    pub drawer_default: Pixels,
    pub drawer_max_fraction: f32,
    pub drawer_tab: Pixels,
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

fn role(size: f32, line: f32, weight: FontWeight) -> TypeRole {
    TypeRole {
        size: rems(size / 16.),
        line_height: rems(line / 16.),
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
                0x242424, 0xf87171, 0x4ade80, 0xfacc15, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xd4d4d8,
                0x71717a, 0xfca5a5, 0x86efac, 0xfde047, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xfafafa,
            ]
        } else {
            [
                0x1f1f1f, 0xdc2626, 0x16a34a, 0xb45309, 0x2563eb, 0x9333ea, 0x0e7490, 0x3f3f46,
                0x71717a, 0xb91c1c, 0x15803d, 0x92400e, 0x1d4ed8, 0x7e22ce, 0x155e75, 0x18181b,
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
                ui_font: "Geist".into(),
                mono_font: "Geist Mono".into(),
                meta: role(11., 14., FontWeight::NORMAL),
                label: role(12., 16., FontWeight::NORMAL),
                ui: role(13., 18., FontWeight::NORMAL),
                read: role(14., 22., FontWeight::NORMAL),
                h3: role(15., 22., FontWeight::SEMIBOLD),
                h2: role(16., 24., FontWeight::MEDIUM),
                h1: role(19., 27., FontWeight::SEMIBOLD),
                code: role(12.5, 18., FontWeight::NORMAL),
                diff: role(12., 20., FontWeight::NORMAL),
                terminal: role(13., 18., FontWeight::NORMAL),
            },
            space: Spacing([2., 4., 6., 8., 12., 16., 20., 24., 32., 48.].map(px)),
            radius: Radii {
                inline: px(4.),
                control: px(6.),
                track: px(8.),
                card: px(10.),
                large: px(16.),
                full: px(9999.),
            },
            motion: Motion {
                hover: transition(150, [0.4, 0., 0.2, 1.]),
                press: transition(120, [0.22, 1., 0.36, 1.]),
                collapse: transition(180, [0., 0., 0.58, 1.]),
                view_switch: transition(150, [0.25, 0.1, 0.25, 1.]),
                menu: [
                    transition(140, [0.25, 0.1, 0.25, 1.]),
                    transition(100, [0.25, 0.1, 0.25, 1.]),
                ],
                resize: transition(200, [0., 0., 0.58, 1.]),
                stream_reveal: transition(320, [0.16, 1., 0.3, 1.]),
                stream_caret: size(px(2.), px(15.)),
                spinner: transition(750, [0., 0., 1., 1.]),
            },
            layout: Layout {
                window_default: size(px(1320.), px(880.)),
                window_min: size(px(900.), px(600.)),
                header: px(44.),
                sidebar_min: px(224.),
                sidebar_default: px(256.),
                sidebar_max: px(400.),
                project_row: px(28.),
                thread_row: px(30.),
                agent_row: px(28.),
                agent_margin: px(14.),
                agent_padding: px(8.),
                guide: px(1.),
                transcript_min: px(560.),
                transcript_default: px(736.),
                transcript_max: px(1200.),
                gutter: px(48.),
                composer_max: px(768.),
                composer_min_height: px(48.),
                composer_max_height: px(260.),
                composer_toolbar: px(40.),
                fade: px(24.),
                drawer_min: px(160.),
                drawer_default: px(280.),
                drawer_max_fraction: 0.55,
                drawer_tab: px(38.),
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
            kit.font_size = px(16.);
            kit.mono_font_size = px(12.5);
            kit.radius = theme.radius.control;
            kit.shadow = false;
        });
        cx.set_global(theme);
        cx.refresh_windows();
    }
}
pub fn register_fonts(cx: &mut App) -> anyhow::Result<()> {
    cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-Italic.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-Medium.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-SemiBold.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/GeistMono.ttf").as_slice()),
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
