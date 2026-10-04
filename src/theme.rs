//! Themes: a few anchor colors and a paired syntax theme, every other slot derived from them.

// This file is a color table; 6-digit `0xRRGGBB` literals read better grouped as one value.
#![allow(clippy::unreadable_literal)]

use ratatui::style::Color;
use two_face::theme::EmbeddedThemeName;

/// The default theme name; the fallback for an unset CLI value.
pub const DEFAULT: &str = "catppuccin";

/// A theme's intrinsic cast, which sets the derivation direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// A palette's syntax theme: a vendored `.tmTheme`, or one from `two-face`.
#[derive(Clone, Copy, Debug)]
pub enum SyntaxChoice {
    Bundled(&'static [u8]),
    Embedded(EmbeddedThemeName),
}

/// A resolved theme: its name, the chrome `Palette`, and its paired syntax theme.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub palette: Palette,
    pub syntax: SyntaxChoice,
}

/// The resolved colors every UI element paints — one source for chrome and diff fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The background anchor: never painted, but what the modal scrim blends toward.
    pub base: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface2: Color,
    pub dim2: Color,
    pub dim1: Color,
    pub dim0: Color,
    pub text: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub orange: Color,
    pub purple: Color,
    pub blue: Color,
    pub del_bg: Color,
    pub ins_bg: Color,
    pub emph_del_bg: Color,
    pub emph_ins_bg: Color,
    /// The warm fill behind a search match, legible on any row.
    pub match_hl: Color,
    /// The selection fill, a hue apart from the cursor-row fills.
    pub sel_bg: Color,
}

impl Palette {
    /// The cursor-row fill, one surface stronger in the focused pane.
    pub fn cursor_bg(&self, focused: bool) -> Color {
        if focused { self.surface2 } else { self.surface1 }
    }

    /// A color on a selection fill: `dim2` would vanish there, so it rises to `dim0`.
    pub fn on_fill(&self, color: Color) -> Color {
        if color == self.dim2 { self.dim0 } else { color }
    }

    /// A changed block's bar: green when it only gained lines, else amber, lifted to read on `base`.
    #[must_use]
    pub fn bar_color(&self, bar: crate::diff::Bar) -> Color {
        use crate::diff::Bar;
        let hue = match bar {
            Bar::Added => self.green,
            Bar::Modified => self.yellow,
        };
        lift(hue, self.base, self.text, MIN_MARK_CONTRAST)
    }

    /// A marker row's color: red when removed, amber when silent, lifted like a bar.
    #[must_use]
    pub fn marker_color(&self, kind: crate::diff::MarkerKind) -> Color {
        use crate::diff::MarkerKind;
        let hue = match kind {
            MarkerKind::Removed => self.red,
            MarkerKind::Unrendered => self.yellow,
        };
        lift(hue, self.base, self.text, MIN_MARK_CONTRAST)
    }

    /// Recede a color halfway to `base` behind a modal; terminal defaults pass through.
    pub fn scrim(&self, color: Color) -> Color {
        match color {
            Color::Rgb(..) => blend(color, self.base, 0.5),
            other => other,
        }
    }
}

/// Resolve a theme name; anything unknown falls back to the default, logged.
pub fn resolve(name: Option<&str>) -> Theme {
    match name {
        None => catppuccin(),
        Some(n) => build(n).unwrap_or_else(|| {
            logln!("unknown theme {n:?}; using {DEFAULT}");
            catppuccin()
        }),
    }
}

/// Whether `name` is a built-in theme, as config validation asks.
pub fn is_known(name: &str) -> bool {
    build(name).is_some()
}

/// The theme named `name`, by herdr's own theme names.
fn build(name: &str) -> Option<Theme> {
    use Appearance::{Dark, Light};
    use EmbeddedThemeName as E;
    Some(match name {
        "catppuccin" => catppuccin(),
        "catppuccin-latte" => catppuccin_latte(),
        "dracula" => derived("dracula", Dark, E::Dracula, DRACULA),
        "nord" => derived("nord", Dark, E::Nord, NORD),
        "gruvbox" => derived("gruvbox", Dark, E::GruvboxDark, GRUVBOX),
        "gruvbox-light" => derived("gruvbox-light", Light, E::GruvboxLight, GRUVBOX_LIGHT),
        "one-dark" => derived("one-dark", Dark, E::TwoDark, ONE_DARK),
        "one-light" => derived("one-light", Light, E::OneHalfLight, ONE_LIGHT),
        "solarized" => derived("solarized", Dark, E::SolarizedDark, SOLARIZED),
        "solarized-light" => derived("solarized-light", Light, E::SolarizedLight, SOLARIZED_LIGHT),
        // Popular themes beyond herdr's set, whose syntax `two-face` already provides.
        "catppuccin-frappe" => derived("catppuccin-frappe", Dark, E::CatppuccinFrappe, FRAPPE),
        "catppuccin-macchiato" => {
            derived("catppuccin-macchiato", Dark, E::CatppuccinMacchiato, MACCHIATO)
        }
        "github-light" => derived("github-light", Light, E::Github, GITHUB_LIGHT),
        "monokai" => derived("monokai", Dark, E::MonokaiExtended, MONOKAI),
        // herdr names whose syntax `two-face` lacks, paired with a vendored `.tmTheme`.
        "tokyo-night" => bundled("tokyo-night", Dark, TOKYO_NIGHT_TM, TOKYO_NIGHT),
        "tokyo-night-day" => bundled("tokyo-night-day", Light, TOKYO_NIGHT_DAY_TM, TOKYO_NIGHT_DAY),
        "rose-pine" => bundled("rose-pine", Dark, ROSE_PINE_TM, ROSE_PINE),
        "rose-pine-dawn" => bundled("rose-pine-dawn", Light, ROSE_PINE_DAWN_TM, ROSE_PINE_DAWN),
        // Beyond herdr's set, paired with a vendored `.tmTheme`.
        "ayu" => bundled("ayu", Dark, AYU_TM, AYU),
        "everforest" => everforest(),
        _ => return None,
    })
}

/// A derived theme: its palette is computed from `anchors`, paired with a `two-face` syntax theme.
fn derived(
    name: &'static str,
    appearance: Appearance,
    syntax: EmbeddedThemeName,
    anchors: Anchors,
) -> Theme {
    Theme { name, palette: derive(anchors, appearance), syntax: SyntaxChoice::Embedded(syntax) }
}

/// The anchor colors a derived theme lists; the rest of its palette is computed from these.
#[derive(Clone, Copy, Debug)]
struct Anchors {
    base: Color,
    text: Color,
    red: Color,
    green: Color,
    yellow: Color,
    orange: Color,
    purple: Color,
    blue: Color,
}

/// Catppuccin Mocha, pinned to its canonical values.
fn catppuccin() -> Theme {
    Theme {
        name: "catppuccin",
        palette: Palette {
            base: Color::Rgb(0x1e, 0x1e, 0x2e),
            surface0: Color::Rgb(0x31, 0x32, 0x44),
            surface1: Color::Rgb(0x45, 0x47, 0x5a),
            surface2: Color::Rgb(0x58, 0x5b, 0x70),
            dim2: Color::Rgb(0x6c, 0x70, 0x86),
            dim1: Color::Rgb(0x7f, 0x84, 0x9c),
            dim0: Color::Rgb(0xa6, 0xad, 0xc8),
            text: Color::Rgb(0xcd, 0xd6, 0xf4),
            red: Color::Rgb(0xf3, 0x8b, 0xa8),
            green: Color::Rgb(0xa6, 0xe3, 0xa1),
            yellow: Color::Rgb(0xf9, 0xe2, 0xaf),
            orange: Color::Rgb(0xfa, 0xb3, 0x87),
            purple: Color::Rgb(0xcb, 0xa6, 0xf7),
            blue: Color::Rgb(0xb4, 0xbe, 0xfe),
            del_bg: Color::Rgb(0x45, 0x23, 0x2f),
            ins_bg: Color::Rgb(0x1f, 0x3a, 0x2a),
            emph_del_bg: Color::Rgb(0x6e, 0x34, 0x46),
            emph_ins_bg: Color::Rgb(0x30, 0x55, 0x3f),
            match_hl: Color::Rgb(0x5c, 0x51, 0x2b),
            sel_bg: Color::Rgb(0x35, 0x3d, 0x7d),
        },
        syntax: SyntaxChoice::Bundled(MOCHA_TM),
    }
}

/// A theme whose palette is derived from `anchors`, paired with a bundled `.tmTheme`'s bytes.
fn bundled(
    name: &'static str,
    appearance: Appearance,
    syntax: &'static [u8],
    anchors: Anchors,
) -> Theme {
    Theme { name, palette: derive(anchors, appearance), syntax: SyntaxChoice::Bundled(syntax) }
}

/// Vendored `.tmTheme`s `two-face` lacks; licenses in the README.
const MOCHA_TM: &[u8] = include_bytes!("../assets/Catppuccin Mocha.tmTheme");
const TOKYO_NIGHT_TM: &[u8] = include_bytes!("../assets/tokyo-night.tmTheme");
const TOKYO_NIGHT_DAY_TM: &[u8] = include_bytes!("../assets/tokyo-night-day.tmTheme");
const ROSE_PINE_TM: &[u8] = include_bytes!("../assets/rose-pine.tmTheme");
const ROSE_PINE_DAWN_TM: &[u8] = include_bytes!("../assets/rose-pine-dawn.tmTheme");
const AYU_TM: &[u8] = include_bytes!("../assets/ayu-dark.tmTheme");
const EVERFOREST_TM: &[u8] = include_bytes!("../assets/everforest.tmTheme");

/// Everforest dark hard: derived, except the diff row fills upstream ships its own of.
fn everforest() -> Theme {
    let derived = bundled("everforest", Appearance::Dark, EVERFOREST_TM, EVERFOREST);
    Theme {
        palette: Palette { ins_bg: hex(0x3c4841), del_bg: hex(0x493b40), ..derived.palette },
        ..derived
    }
}

/// Catppuccin Latte, a derived light theme.
fn catppuccin_latte() -> Theme {
    derived(
        "catppuccin-latte",
        Appearance::Light,
        EmbeddedThemeName::CatppuccinLatte,
        CATPPUCCIN_LATTE,
    )
}

const CATPPUCCIN_LATTE: Anchors =
    anchors(0xeff1f5, 0x4c4f69, 0xd20f39, 0x40a02b, 0xdf8e1d, 0xfe640b, 0x8839ef, 0x7287fd);

/// Anchors: base, text, then red, green, yellow, orange, purple, blue.
const DRACULA: Anchors =
    anchors(0x282a36, 0xf8f8f2, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xffb86c, 0xbd93f9, 0x8be9fd);
const NORD: Anchors =
    anchors(0x2e3440, 0xd8dee9, 0xbf616a, 0xa3be8c, 0xebcb8b, 0xd08770, 0xb48ead, 0x81a1c1);
const GRUVBOX: Anchors =
    anchors(0x282828, 0xebdbb2, 0xfb4934, 0xb8bb26, 0xfabd2f, 0xfe8019, 0xd3869b, 0x83a598);
const GRUVBOX_LIGHT: Anchors =
    anchors(0xfbf1c7, 0x3c3836, 0x9d0006, 0x79740e, 0xb57614, 0xaf3a03, 0x8f3f71, 0x076678);
const ONE_DARK: Anchors =
    anchors(0x282c34, 0xabb2bf, 0xe06c75, 0x98c379, 0xe5c07b, 0xd19a66, 0xc678dd, 0x61afef);
const ONE_LIGHT: Anchors =
    anchors(0xfafafa, 0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x986801, 0xa626a4, 0x4078f2);
const SOLARIZED: Anchors =
    anchors(0x002b36, 0x93a1a1, 0xdc322f, 0x859900, 0xb58900, 0xcb4b16, 0x6c71c4, 0x268bd2);
const SOLARIZED_LIGHT: Anchors =
    anchors(0xfdf6e3, 0x586e75, 0xdc322f, 0x859900, 0xb58900, 0xcb4b16, 0x6c71c4, 0x268bd2);
const FRAPPE: Anchors =
    anchors(0x303446, 0xc6d0f5, 0xe78284, 0xa6d189, 0xe5c890, 0xef9f76, 0xca9ee6, 0xbabbf1);
const MACCHIATO: Anchors =
    anchors(0x24273a, 0xcad3f5, 0xed8796, 0xa6da95, 0xeed49f, 0xf5a97f, 0xc6a0f6, 0xb7bdf8);
const GITHUB_LIGHT: Anchors =
    anchors(0xffffff, 0x1f2328, 0xcf222e, 0x1a7f37, 0x9a6700, 0xbc4c00, 0x8250df, 0x0969da);
const MONOKAI: Anchors =
    anchors(0x272822, 0xf8f8f2, 0xf92672, 0xa6e22e, 0xe6db74, 0xfd971f, 0xae81ff, 0x66d9ef);
const TOKYO_NIGHT: Anchors =
    anchors(0x1a1b26, 0xc0caf5, 0xf7768e, 0x9ece6a, 0xe0af68, 0xff9e64, 0xbb9af7, 0x7aa2f7);
const TOKYO_NIGHT_DAY: Anchors =
    anchors(0xe1e2e7, 0x3760bf, 0xf52a65, 0x587539, 0x8c6c3e, 0xb15c00, 0x9854f1, 0x2e7de9);
const ROSE_PINE: Anchors =
    anchors(0x191724, 0xe0def4, 0xeb6f92, 0x9ccfd8, 0xf6c177, 0xebbcba, 0xc4a7e7, 0x31748f);
const ROSE_PINE_DAWN: Anchors =
    anchors(0xfaf4ed, 0x575279, 0xb4637a, 0x56949f, 0xea9d34, 0xd7827e, 0x907aa9, 0x286983);
/// ayu Dark from `ayu-colors` 9.1: `ui.bg`, `editor.fg`, and its syntax accents.
const AYU: Anchors =
    anchors(0x0d1017, 0xbfbdb6, 0xf07178, 0xaad94c, 0xffb454, 0xff8f40, 0xd2a6ff, 0x59c2ff);
/// Everforest dark, hard background: `bg0`, `fg` and the accents from `autoload/everforest.vim`.
const EVERFOREST: Anchors =
    anchors(0x272e33, 0xd3c6aa, 0xe67e80, 0xa7c080, 0xdbbc7f, 0xe69875, 0xd699b6, 0x7fbbb3);

/// `Anchors` from `0xRRGGBB` literals, one per slot, so a palette reads as one row.
#[allow(clippy::too_many_arguments)]
const fn anchors(
    base: u32,
    text: u32,
    red: u32,
    green: u32,
    yellow: u32,
    orange: u32,
    purple: u32,
    blue: u32,
) -> Anchors {
    Anchors {
        base: hex(base),
        text: hex(text),
        red: hex(red),
        green: hex(green),
        yellow: hex(yellow),
        orange: hex(orange),
        purple: hex(purple),
        blue: hex(blue),
    }
}

/// A `Color::Rgb` from a `0xRRGGBB` literal.
const fn hex(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// A palette from anchors: surfaces stepped from `base`, diff fills tinted and kept legible.
fn derive(a: Anchors, appearance: Appearance) -> Palette {
    let pole = match appearance {
        Appearance::Dark => WHITE,
        Appearance::Light => BLACK,
    };
    let surface = |t: f64| blend(a.base, pole, t);
    Palette {
        base: a.base,
        surface0: surface(0.045),
        surface1: surface(0.09),
        surface2: surface(0.14),
        dim2: surface(0.26),
        dim1: surface(0.34),
        dim0: blend(a.text, a.base, 0.18),
        text: a.text,
        red: a.red,
        green: a.green,
        yellow: a.yellow,
        orange: a.orange,
        purple: a.purple,
        blue: a.blue,
        del_bg: readable_tint(a.red, a.base, a.text, appearance, false),
        ins_bg: readable_tint(a.green, a.base, a.text, appearance, false),
        emph_del_bg: readable_tint(a.red, a.base, a.text, appearance, true),
        emph_ins_bg: readable_tint(a.green, a.base, a.text, appearance, true),
        match_hl: readable_tint(a.yellow, a.base, a.text, appearance, true),
        sel_bg: readable_tint(saturated(a.blue), a.base, a.text, appearance, true),
    }
}

const WHITE: Color = Color::Rgb(0xff, 0xff, 0xff);
const BLACK: Color = Color::Rgb(0x00, 0x00, 0x00);

/// The lowest contrast a diff fill keeps against the row's text.
const MIN_FILL_CONTRAST: f64 = 4.5;

/// The lowest contrast a change mark keeps against the base: WCAG's floor for non-text marks.
const MIN_MARK_CONTRAST: f64 = 3.0;

/// `fg` blended toward `toward` just enough to clear `min` contrast on `bg`.
fn lift(fg: Color, bg: Color, toward: Color, min: f64) -> Color {
    let mut t = 0.0;
    while t < 1.0 {
        let lifted = blend(fg, toward, t);
        if contrast(lifted, bg) >= min {
            return lifted;
        }
        t += 0.02;
    }
    toward
}

/// A syntax `fg` on `fill`, lifted back to its own contrast on plain `base`, capped at the floor,
/// so a dim comment stays readable and still dimmer than code.
pub fn legible(fg: Color, fill: Color, base: Color, toward: Color) -> Color {
    let target = contrast(fg, base).min(MIN_FILL_CONTRAST);
    let mut t = 0.0;
    while t < 1.0 {
        let lifted = blend(fg, toward, t);
        if contrast(lifted, fill) >= target {
            return lifted;
        }
        t += 0.02;
    }
    toward
}

/// A diff-row fill: `base` tinted with `accent` as strongly as `fg` stays legible on, else bare `base`.
fn readable_tint(
    accent: Color,
    base: Color,
    fg: Color,
    appearance: Appearance,
    strong: bool,
) -> Color {
    let start = match (appearance, strong) {
        (Appearance::Dark, false) => 0.20,
        (Appearance::Dark, true) => 0.38,
        (Appearance::Light, false) => 0.12,
        (Appearance::Light, true) => 0.22,
    };
    let mut t = start;
    while t > 0.0 {
        let fill = blend(base, accent, t);
        if contrast(fg, fill) >= MIN_FILL_CONTRAST {
            return fill;
        }
        t -= 0.02;
    }
    base
}

/// An accent pushed halfway to its saturated core, so its tint reads as a hue, not a gray.
fn saturated(c: Color) -> Color {
    let (r, g, b) = channels(c);
    let lo = r.min(g).min(b);
    let span = r.max(g).max(b) - lo;
    if span == 0 {
        return c;
    }
    let core = |ch: u8| (f64::from(ch - lo) * 255.0 / f64::from(span)).round() as u8;
    blend(c, Color::Rgb(core(r), core(g), core(b)), 0.5)
}

/// Linear per-channel blend: `t` of the way from `from` to `to` (0.0 = `from`, 1.0 = `to`).
fn blend(from: Color, to: Color, t: f64) -> Color {
    let (fr, fg, fb) = channels(from);
    let (tr, tg, tb) = channels(to);
    let mix = |lhs: u8, rhs: u8| (f64::from(lhs) * (1.0 - t) + f64::from(rhs) * t).round() as u8;
    Color::Rgb(mix(fr, tr), mix(fg, tg), mix(fb, tb))
}

/// The WCAG contrast ratio between two colors (1.0 .. 21.0).
fn contrast(fg: Color, bg: Color) -> f64 {
    let (lf, lb) = (luminance(fg), luminance(bg));
    let (hi, lo) = if lf >= lb { (lf, lb) } else { (lb, lf) };
    (hi + 0.05) / (lo + 0.05)
}

/// WCAG relative luminance, with sRGB linearization.
fn luminance(color: Color) -> f64 {
    let (r, g, b) = channels(color);
    let lin = |channel: u8| {
        let srgb = f64::from(channel) / 255.0;
        if srgb <= 0.03928 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// The RGB channels of a color; anchors are always `Rgb`, so the fallback never fires.
fn channels(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Appearance, CATPPUCCIN_LATTE, MIN_FILL_CONTRAST, Palette, contrast, derive, legible,
        resolve,
    };
    use ratatui::style::Color;

    #[test]
    fn change_marks_clear_three_to_one_on_every_base() {
        use crate::diff::{Bar, MarkerKind};
        for name in ["catppuccin", "catppuccin-latte", "rose-pine-dawn", "rose-pine"] {
            let p = resolve(Some(name)).palette;
            let colors = [
                p.bar_color(Bar::Added),
                p.bar_color(Bar::Modified),
                p.marker_color(MarkerKind::Removed),
                p.marker_color(MarkerKind::Unrendered),
            ];
            for c in colors {
                let ratio = contrast(c, p.base);
                assert!(ratio >= 3.0, "{name} {c:?}: {ratio:.2}");
            }
        }
    }

    #[test]
    fn contrast_black_white_is_max() {
        let r = contrast(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255));
        assert!((r - 21.0).abs() < 0.01, "black vs white is ~21:1, got {r}");
    }

    #[test]
    fn catppuccin_is_the_unchanged_mocha_palette() {
        let p = resolve(Some("catppuccin")).palette;
        assert_eq!(p.surface0, Color::Rgb(0x31, 0x32, 0x44));
        assert_eq!(p.text, Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(p.del_bg, Color::Rgb(0x45, 0x23, 0x2f));
        assert_eq!(p.ins_bg, Color::Rgb(0x1f, 0x3a, 0x2a));
        // Renamed slots keep their Mocha values (orange was peach, purple mauve).
        assert_eq!(p.orange, Color::Rgb(0xfa, 0xb3, 0x87));
        assert_eq!(p.purple, Color::Rgb(0xcb, 0xa6, 0xf7));
        assert_eq!(p.blue, Color::Rgb(0xb4, 0xbe, 0xfe));
        assert_eq!(p.dim0, Color::Rgb(0xa6, 0xad, 0xc8));
        assert_eq!(p.dim1, Color::Rgb(0x7f, 0x84, 0x9c));
        assert_eq!(p.dim2, Color::Rgb(0x6c, 0x70, 0x86));
        // The selection fill is a real hue, nowhere near the gray cursor fills.
        assert_eq!(p.sel_bg, Color::Rgb(0x35, 0x3d, 0x7d));
    }

    #[test]
    fn everforest_diff_rows_use_its_own_palette_fills() {
        let p = resolve(Some("everforest")).palette;
        assert_eq!(p.ins_bg, Color::Rgb(0x3c, 0x48, 0x41));
        assert_eq!(p.del_bg, Color::Rgb(0x49, 0x3b, 0x40));
        // Everything else still comes from the anchors.
        assert_eq!(p.base, Color::Rgb(0x27, 0x2e, 0x33));
        assert_eq!(p.text, Color::Rgb(0xd3, 0xc6, 0xaa));
    }

    #[test]
    fn unknown_and_terminal_fall_back_to_default() {
        assert_eq!(resolve(Some("nope")).name, "catppuccin");
        assert_eq!(resolve(Some("terminal")).name, "catppuccin");
        assert_eq!(resolve(None).name, "catppuccin");
    }

    #[test]
    fn latte_is_a_selectable_light_theme() {
        assert_eq!(resolve(Some("catppuccin-latte")).name, "catppuccin-latte");
    }

    #[test]
    fn an_emphasized_color_keeps_its_plain_background_legibility() {
        // Tokyo Night's comment color: on the raw emphasis fills it sits near 1.2.
        let comment = Color::Rgb(0x56, 0x5f, 0x89);
        for &(name, _) in NAMED {
            let p = resolve(Some(name)).palette;
            let target = contrast(comment, p.base).min(MIN_FILL_CONTRAST);
            for fill in [p.emph_del_bg, p.emph_ins_bg] {
                let lifted = legible(comment, fill, p.base, p.text);
                assert!(
                    contrast(lifted, fill) >= target,
                    "{name}: {fill:?} still costs legibility"
                );
            }
        }
    }

    #[test]
    fn a_color_already_legible_on_the_fill_is_untouched() {
        let p = resolve(Some("catppuccin")).palette;
        assert_eq!(legible(p.text, p.emph_ins_bg, p.base, p.text), p.text);
    }

    #[test]
    fn distinct_syntax_colors_stay_distinct_on_floor_hugging_themes() {
        // Fills just above the floor: a floor-lift would paint every color as `text`.
        let (comment, keyword) = (Color::Rgb(0x56, 0x5f, 0x89), Color::Rgb(0x9d, 0x7c, 0xd8));
        for name in ["tokyo-night-day", "solarized", "tokyo-night"] {
            let p = resolve(Some(name)).palette;
            for fill in [p.emph_del_bg, p.emph_ins_bg] {
                let (a, b) = (
                    legible(comment, fill, p.base, p.text),
                    legible(keyword, fill, p.base, p.text),
                );
                assert_ne!(a, b, "{name}: two syntax colors merged on {fill:?}");
                assert_ne!(a, p.text, "{name}: the comment lost its hue on {fill:?}");
            }
        }
    }

    #[test]
    fn light_derivation_keeps_diff_fills_legible() {
        // The shipped Latte anchors, so a retune breaking the floor or the ramp fails here.
        let anchors = CATPPUCCIN_LATTE;
        let p: Palette = derive(anchors, Appearance::Light);
        // Text stays readable on every derived fill, on a light base.
        for fill in [p.del_bg, p.ins_bg, p.emph_del_bg, p.emph_ins_bg] {
            assert!(
                contrast(p.text, fill) >= MIN_FILL_CONTRAST,
                "fill {fill:?} drops below the legibility floor",
            );
        }
        // A light theme's surfaces step darker than its base.
        let base_lum = super::luminance(anchors.base);
        assert!(super::luminance(p.surface0) < base_lum, "surface0 is darker than the base");
        assert!(
            super::luminance(p.surface2) < super::luminance(p.surface0),
            "the surface ramp keeps darkening",
        );
    }

    /// Every named theme and its appearance (`true` = light).
    const NAMED: &[(&str, bool)] = &[
        ("catppuccin", false),
        ("catppuccin-latte", true),
        ("dracula", false),
        ("nord", false),
        ("gruvbox", false),
        ("gruvbox-light", true),
        ("one-dark", false),
        ("one-light", true),
        ("solarized", false),
        ("solarized-light", true),
        ("catppuccin-frappe", false),
        ("catppuccin-macchiato", false),
        ("github-light", true),
        ("monokai", false),
        ("tokyo-night", false),
        ("tokyo-night-day", true),
        ("rose-pine", false),
        ("rose-pine-dawn", true),
        ("ayu", false),
        ("everforest", false),
    ];

    #[test]
    fn every_named_theme_resolves_to_itself() {
        for &(name, _) in NAMED {
            assert_eq!(resolve(Some(name)).name, name, "{name} should resolve to its own palette");
        }
    }

    #[test]
    fn every_theme_keeps_diff_fills_legible() {
        for &(name, _) in NAMED {
            let p = resolve(Some(name)).palette;
            for fill in [p.del_bg, p.ins_bg, p.emph_del_bg, p.emph_ins_bg, p.sel_bg] {
                assert!(
                    contrast(p.text, fill) >= MIN_FILL_CONTRAST,
                    "{name}: fill {fill:?} drops below the legibility floor",
                );
            }
        }
    }

    #[test]
    fn appearance_orients_text_against_surface() {
        for &(name, light) in NAMED {
            let p = resolve(Some(name)).palette;
            // Light theme: dark text on a lighter surface. Dark theme: the reverse.
            let text_darker = super::luminance(p.text) < super::luminance(p.surface0);
            assert_eq!(text_darker, light, "{name}: text/surface contrast points the wrong way");
        }
    }
}
