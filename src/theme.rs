//! The color model: named palettes, derivation from anchors, and selection.
//!
//! A theme is a few anchor colors plus a paired syntax theme;
//! every other slot is derived from the anchors. One theme — `catppuccin` — instead
//! pins its whole palette as a literal, to stay byte-identical to the pre-theming
//! colors. One selection sets both the chrome `Palette` and the syntax theme, so they
//! never desync. The pane background stays the terminal's, so only these fills and the
//! syntax foregrounds are painted.

// This file is a color table; 6-digit `0xRRGGBB` literals read better grouped as one value.
#![allow(clippy::unreadable_literal)]

use ratatui::style::Color;
use two_face::theme::EmbeddedThemeName;

use crate::roles::{Cast, Fill, Overrides, Primitives, Roles, blend, contrast};

/// The default theme name; the fallback for an unset CLI value.
pub const DEFAULT: &str = "catppuccin";

/// A theme's intrinsic cast, which sets the derivation direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// The syntax theme paired with a palette: a bundled `.tmTheme`'s vendored bytes (for themes
/// `two-face` lacks, and for Catppuccin Mocha kept byte-identical to today's), or a theme
/// from the `two-face` embedded set.
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

/// What every UI element paints with: the theme's semantic roles, resolved per fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    roles: Roles,
}

impl Palette {
    /// The color of `fill`.
    pub fn fill(&self, fill: Fill) -> Color {
        self.roles.fill(fill)
    }

    /// `ink` painting text on `on`.
    pub fn ink(&self, ink: crate::roles::Ink, on: Fill) -> Color {
        self.roles.ink(ink, on)
    }

    /// `ink` painting a glyph, sign or border on `on`.
    pub fn mark(&self, ink: crate::roles::Ink, on: Fill) -> Color {
        self.roles.mark(ink, on)
    }

    /// The roles themselves, for the checks that walk every one.
    pub fn roles(&self) -> &Roles {
        &self.roles
    }

    /// Recede a painted color behind an open modal: halfway to `base`, so the modal owns the
    /// eye while the page behind stays recognizable. Non-RGB colors are the
    /// terminal's own defaults, which have no known distance to `base`; they pass through.
    pub fn scrim(&self, color: Color) -> Color {
        match color {
            Color::Rgb(..) => blend(color, self.roles.fill(Fill::Base), 0.5),
            other => other,
        }
    }
}

/// Resolve a theme name to a `Theme`. `None`, an unknown name, or a not-yet-supported
/// one (including `terminal`) falls back to the default and logs; never a half-palette.
pub fn resolve(name: Option<&str>) -> Theme {
    match name {
        None => catppuccin(),
        Some(n) => build(n).unwrap_or_else(|| {
            logln!("unknown theme {n:?}; using {DEFAULT}");
            catppuccin()
        }),
    }
}

/// Whether `name` selects a complete built-in theme. Plugin configuration validates against
/// this same catalog before a snapshot is applied.
pub fn is_known(name: &str) -> bool {
    build(name).is_some()
}

/// The built theme for `name`, or `None` when it is not a known palette. Names match herdr's
/// so the value a user copies from their herdr config resolves to the same palette.
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
    Theme {
        name,
        palette: derive(anchors, appearance, Overrides::default()),
        syntax: SyntaxChoice::Embedded(syntax),
    }
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
    /// The theme's UI accent: herdr's pick for the themes it ships, upstream's otherwise.
    accent: Color,
}

impl Anchors {
    fn primitives(self, appearance: Appearance) -> Primitives {
        Primitives {
            base: self.base,
            text: self.text,
            red: self.red,
            green: self.green,
            yellow: self.yellow,
            orange: self.orange,
            purple: self.purple,
            blue: self.blue,
            accent: self.accent,
            cast: match appearance {
                Appearance::Dark => Cast::Dark,
                Appearance::Light => Cast::Light,
            },
        }
    }
}

/// Catppuccin Mocha's anchors: its canonical values, with herdr's blue as the accent.
const MOCHA: Anchors = anchors(
    0x1e1e2e, 0xcdd6f4, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0xfab387, 0xcba6f7, 0xb4befe, 0x89b4fa,
);

/// Catppuccin Mocha: pinned to its canonical values so it renders identically to the
/// pre-theming palette.
fn catppuccin() -> Theme {
    // Catppuccin ships its own surfaces and fills, so they enter the roles as given.
    let fills = Overrides::default()
        .fill(Fill::Bar, hex(0x313244))
        .fill(Fill::CursorInactive, hex(0x45475a))
        .fill(Fill::Cursor, hex(0x585b70))
        .fill(Fill::Removed, hex(0x45232f))
        .fill(Fill::Added, hex(0x1f3a2a))
        .fill(Fill::RemovedEmph, hex(0x6e3446))
        .fill(Fill::AddedEmph, hex(0x30553f))
        .fill(Fill::Highlight, hex(0x5c512b))
        .fill(Fill::Selection, hex(0x353d7d));
    Theme {
        name: "catppuccin",
        palette: derive(MOCHA, Appearance::Dark, fills),
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
    Theme {
        name,
        palette: derive(anchors, appearance, Overrides::default()),
        syntax: SyntaxChoice::Bundled(syntax),
    }
}

/// Vendored `.tmTheme` assets for the syntax themes `two-face` does not carry (and Mocha,
/// kept as the byte-identical source of today's highlighting). Licenses listed in the
/// README's License section.
const MOCHA_TM: &[u8] = include_bytes!("../assets/Catppuccin Mocha.tmTheme");
const TOKYO_NIGHT_TM: &[u8] = include_bytes!("../assets/tokyo-night.tmTheme");
const TOKYO_NIGHT_DAY_TM: &[u8] = include_bytes!("../assets/tokyo-night-day.tmTheme");
const ROSE_PINE_TM: &[u8] = include_bytes!("../assets/rose-pine.tmTheme");
const ROSE_PINE_DAWN_TM: &[u8] = include_bytes!("../assets/rose-pine-dawn.tmTheme");
const AYU_TM: &[u8] = include_bytes!("../assets/ayu-dark.tmTheme");
const EVERFOREST_TM: &[u8] = include_bytes!("../assets/everforest.tmTheme");

/// Everforest dark hard: derived from its anchors, except the diff row fills. They are the one
/// hand-set fill in a derived theme because upstream ships its own (`bg_green`, `bg_red`),
/// so the rows match the Neovim theme. It has no word-emphasis fills, so those stay derived.
fn everforest() -> Theme {
    let fills =
        Overrides::default().fill(Fill::Added, hex(0x3c4841)).fill(Fill::Removed, hex(0x493b40));
    Theme {
        name: "everforest",
        palette: derive(EVERFOREST, Appearance::Dark, fills),
        syntax: SyntaxChoice::Bundled(EVERFOREST_TM),
    }
}

/// Catppuccin Latte: a light theme, derived from its anchors to exercise the derivation
/// path (and paired with `two-face`'s Latte syntax theme).
fn catppuccin_latte() -> Theme {
    derived(
        "catppuccin-latte",
        Appearance::Light,
        EmbeddedThemeName::CatppuccinLatte,
        CATPPUCCIN_LATTE,
    )
}

const CATPPUCCIN_LATTE: Anchors = anchors(
    0xeff1f5, 0x4c4f69, 0xd20f39, 0x40a02b, 0xdf8e1d, 0xfe640b, 0x8839ef, 0x7287fd, 0x1e66f5,
);

/// Canonical anchors for the derived themes. base, text, then the six accents
/// (red, green, yellow, orange, purple, blue); surfaces and diff fills are derived.
const DRACULA: Anchors = anchors(
    0x282a36, 0xf8f8f2, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xffb86c, 0xbd93f9, 0x8be9fd, 0xbd93f9,
);
const NORD: Anchors = anchors(
    0x2e3440, 0xd8dee9, 0xbf616a, 0xa3be8c, 0xebcb8b, 0xd08770, 0xb48ead, 0x81a1c1, 0x88c0d0,
);
const GRUVBOX: Anchors = anchors(
    0x282828, 0xebdbb2, 0xfb4934, 0xb8bb26, 0xfabd2f, 0xfe8019, 0xd3869b, 0x83a598, 0xd79921,
);
const GRUVBOX_LIGHT: Anchors = anchors(
    0xfbf1c7, 0x3c3836, 0x9d0006, 0x79740e, 0xb57614, 0xaf3a03, 0x8f3f71, 0x076678, 0x076678,
);
const ONE_DARK: Anchors = anchors(
    0x282c34, 0xabb2bf, 0xe06c75, 0x98c379, 0xe5c07b, 0xd19a66, 0xc678dd, 0x61afef, 0x61afef,
);
const ONE_LIGHT: Anchors = anchors(
    0xfafafa, 0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x986801, 0xa626a4, 0x4078f2, 0x4078f2,
);
const SOLARIZED: Anchors = anchors(
    0x002b36, 0x93a1a1, 0xdc322f, 0x859900, 0xb58900, 0xcb4b16, 0x6c71c4, 0x268bd2, 0x268bd2,
);
const SOLARIZED_LIGHT: Anchors = anchors(
    0xfdf6e3, 0x586e75, 0xdc322f, 0x859900, 0xb58900, 0xcb4b16, 0x6c71c4, 0x268bd2, 0x268bd2,
);
const FRAPPE: Anchors = anchors(
    0x303446, 0xc6d0f5, 0xe78284, 0xa6d189, 0xe5c890, 0xef9f76, 0xca9ee6, 0xbabbf1, 0x8caaee,
);
const MACCHIATO: Anchors = anchors(
    0x24273a, 0xcad3f5, 0xed8796, 0xa6da95, 0xeed49f, 0xf5a97f, 0xc6a0f6, 0xb7bdf8, 0x8aadf4,
);
const GITHUB_LIGHT: Anchors = anchors(
    0xffffff, 0x1f2328, 0xcf222e, 0x1a7f37, 0x9a6700, 0xbc4c00, 0x8250df, 0x0969da, 0x0969da,
);
const MONOKAI: Anchors = anchors(
    0x272822, 0xf8f8f2, 0xf92672, 0xa6e22e, 0xe6db74, 0xfd971f, 0xae81ff, 0x66d9ef, 0x66d9ef,
);
const TOKYO_NIGHT: Anchors = anchors(
    0x1a1b26, 0xc0caf5, 0xf7768e, 0x9ece6a, 0xe0af68, 0xff9e64, 0xbb9af7, 0x7aa2f7, 0x7aa2f7,
);
const TOKYO_NIGHT_DAY: Anchors = anchors(
    0xe1e2e7, 0x3760bf, 0xf52a65, 0x587539, 0x8c6c3e, 0xb15c00, 0x9854f1, 0x2e7de9, 0x2e7de9,
);
const ROSE_PINE: Anchors = anchors(
    0x191724, 0xe0def4, 0xeb6f92, 0x9ccfd8, 0xf6c177, 0xebbcba, 0xc4a7e7, 0x31748f, 0xc4a7e7,
);
const ROSE_PINE_DAWN: Anchors = anchors(
    0xfaf4ed, 0x575279, 0xb4637a, 0x56949f, 0xea9d34, 0xd7827e, 0x907aa9, 0x286983, 0x907aa9,
);
/// ayu Dark from `ayu-colors` 9.1: the `ui.bg` base (the terminal background ayu's own
/// ports use), the `editor.fg` text, and its syntax palette for the accents.
const AYU: Anchors = anchors(
    0x0d1017, 0xbfbdb6, 0xf07178, 0xaad94c, 0xffb454, 0xff8f40, 0xd2a6ff, 0x59c2ff, 0xe6b450,
);
/// Everforest dark, hard background: `bg0`, `fg` and the accents from `autoload/everforest.vim`.
const EVERFOREST: Anchors = anchors(
    0x272e33, 0xd3c6aa, 0xe67e80, 0xa7c080, 0xdbbc7f, 0xe69875, 0xd699b6, 0x7fbbb3, 0xa7c080,
);

/// Build `Anchors` from `0xRRGGBB` hex literals, so a palette reads as one compact row.
/// One argument per anchor slot — the count is the palette's shape, not accidental.
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
    accent: u32,
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
        accent: hex(accent),
    }
}

/// A `Color::Rgb` from a `0xRRGGBB` literal.
const fn hex(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// A theme's palette: its roles, derived from `anchors`, with `fills` taken as given.
fn derive(anchors: Anchors, appearance: Appearance, fills: Overrides) -> Palette {
    Palette { roles: Roles::derive(anchors.primitives(appearance), fills) }
}

/// The highest contrast [`legible`] restores a color to on a fill: WCAG's text floor.
const MIN_FILL_CONTRAST: f64 = 4.5;

/// Lift a syntax `fg` painted on `fill` just enough that the fill costs it no legibility: to
/// its own contrast on the plain `base`, capped at [`MIN_FILL_CONTRAST`].
///
/// A fill is floored against body text only, so a dim syntax color — a code comment, above
/// all — can drop far lower on the same fill. Holding each color to its own plain-background
/// contrast keeps it as readable as it was, and keeps a comment dimmer than code. `toward` is
/// the theme's text, so this lightens on a dark theme and darkens on a light one; a color
/// already at its target comes back unchanged.
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

#[cfg(test)]
mod tests {
    use super::{MIN_FILL_CONTRAST, contrast, legible, resolve};
    use crate::roles::{Fill, Ink};
    use ratatui::style::Color;

    #[test]
    fn contrast_black_white_is_max() {
        let r = contrast(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255));
        assert!((r - 21.0).abs() < 0.01, "black vs white is ~21:1, got {r}");
    }

    #[test]
    fn catppuccin_keeps_its_own_surfaces_and_fills() {
        let p = resolve(Some("catppuccin")).palette;
        assert_eq!(p.fill(Fill::Bar), Color::Rgb(0x31, 0x32, 0x44));
        assert_eq!(p.fill(Fill::Cursor), Color::Rgb(0x58, 0x5b, 0x70));
        assert_eq!(p.fill(Fill::Removed), Color::Rgb(0x45, 0x23, 0x2f));
        assert_eq!(p.fill(Fill::Added), Color::Rgb(0x1f, 0x3a, 0x2a));
        assert_eq!(p.fill(Fill::Selection), Color::Rgb(0x35, 0x3d, 0x7d));
        assert_eq!(p.ink(Ink::Text, Fill::Base), Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(p.ink(Ink::Comment, Fill::Base), Color::Rgb(0xfa, 0xb3, 0x87));
    }

    #[test]
    fn everforest_diff_rows_use_its_own_palette_fills() {
        let p = resolve(Some("everforest")).palette;
        assert_eq!(p.fill(Fill::Added), Color::Rgb(0x3c, 0x48, 0x41));
        assert_eq!(p.fill(Fill::Removed), Color::Rgb(0x49, 0x3b, 0x40));
        // Everything else still comes from the anchors.
        assert_eq!(p.fill(Fill::Base), Color::Rgb(0x27, 0x2e, 0x33));
        assert_eq!(p.ink(Ink::Text, Fill::Base), Color::Rgb(0xd3, 0xc6, 0xaa));
    }

    #[test]
    fn unknown_and_terminal_fall_back_to_default() {
        assert_eq!(resolve(Some("nope")).name, "catppuccin");
        assert_eq!(resolve(Some("terminal")).name, "catppuccin");
        assert_eq!(resolve(None).name, "catppuccin");
    }

    #[test]
    fn a_light_theme_steps_its_surfaces_darker() {
        let p = resolve(Some("catppuccin-latte")).palette;
        let lum = |c| contrast(c, Color::Rgb(0, 0, 0));
        assert!(lum(p.fill(Fill::Bar)) < lum(p.fill(Fill::Base)), "the bar is darker than base");
        assert!(lum(p.fill(Fill::Cursor)) < lum(p.fill(Fill::Bar)), "the ramp keeps darkening");
    }

    #[test]
    fn an_emphasized_color_keeps_its_plain_background_legibility() {
        // Tokyo Night's comment color: on the raw emphasis fills it sits near 1.2.
        let comment = Color::Rgb(0x56, 0x5f, 0x89);
        for &(name, _) in NAMED {
            let p = resolve(Some(name)).palette;
            let base = p.fill(Fill::Base);
            let target = contrast(comment, base).min(MIN_FILL_CONTRAST);
            for on in [Fill::RemovedEmph, Fill::AddedEmph] {
                let fill = p.fill(on);
                let lifted = legible(comment, fill, base, p.ink(Ink::Text, on));
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
        let (base, text) = (p.fill(Fill::Base), p.ink(Ink::Text, Fill::Base));
        assert_eq!(legible(text, p.fill(Fill::AddedEmph), base, text), text);
    }

    #[test]
    fn distinct_syntax_colors_stay_distinct_on_floor_hugging_themes() {
        // These themes keep their fills just above the floor for `text`; lifting every color to
        // that floor would paint them all as `text`.
        let (comment, keyword) = (Color::Rgb(0x56, 0x5f, 0x89), Color::Rgb(0x9d, 0x7c, 0xd8));
        for name in ["tokyo-night-day", "solarized", "tokyo-night"] {
            let p = resolve(Some(name)).palette;
            let base = p.fill(Fill::Base);
            for on in [Fill::RemovedEmph, Fill::AddedEmph] {
                let (fill, text) = (p.fill(on), p.ink(Ink::Text, on));
                let (a, b) =
                    (legible(comment, fill, base, text), legible(keyword, fill, base, text));
                assert_ne!(a, b, "{name}: two syntax colors merged on {fill:?}");
                assert_ne!(a, text, "{name}: the comment lost its hue on {fill:?}");
            }
        }
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

    /// A theme's base and text as its anchors list them, to check the derive left them exact.
    fn theme_anchors(name: &str) -> (Color, Color) {
        use super::*;
        let a = match name {
            "catppuccin" => MOCHA,
            "catppuccin-latte" => CATPPUCCIN_LATTE,
            "dracula" => DRACULA,
            "nord" => NORD,
            "gruvbox" => GRUVBOX,
            "gruvbox-light" => GRUVBOX_LIGHT,
            "one-dark" => ONE_DARK,
            "one-light" => ONE_LIGHT,
            "solarized" => SOLARIZED,
            "solarized-light" => SOLARIZED_LIGHT,
            "catppuccin-frappe" => FRAPPE,
            "catppuccin-macchiato" => MACCHIATO,
            "github-light" => GITHUB_LIGHT,
            "monokai" => MONOKAI,
            "tokyo-night" => TOKYO_NIGHT,
            "tokyo-night-day" => TOKYO_NIGHT_DAY,
            "rose-pine" => ROSE_PINE,
            "rose-pine-dawn" => ROSE_PINE_DAWN,
            "ayu" => AYU,
            "everforest" => EVERFOREST,
            other => panic!("unknown theme {other}"),
        };
        (a.base, a.text)
    }

    /// The spec's accent table: herdr's pick for the themes it ships, upstream's otherwise.
    #[test]
    fn every_theme_has_its_accent() {
        let accents = [
            ("catppuccin", 0x89b4fa),
            ("catppuccin-latte", 0x1e66f5),
            ("catppuccin-frappe", 0x8caaee),
            ("catppuccin-macchiato", 0x8aadf4),
            ("tokyo-night", 0x7aa2f7),
            ("tokyo-night-day", 0x2e7de9),
            ("dracula", 0xbd93f9),
            ("nord", 0x88c0d0),
            ("gruvbox", 0xd79921),
            ("gruvbox-light", 0x076678),
            ("one-dark", 0x61afef),
            ("one-light", 0x4078f2),
            ("solarized", 0x268bd2),
            ("solarized-light", 0x268bd2),
            ("rose-pine", 0xc4a7e7),
            ("rose-pine-dawn", 0x907aa9),
            ("ayu", 0xe6b450),
            ("everforest", 0xa7c080),
            ("github-light", 0x0969da),
            ("monokai", 0x66d9ef),
        ];
        assert_eq!(accents.len(), NAMED.len(), "every theme names its accent");
        for (name, accent) in accents {
            let roles = *resolve(Some(name)).palette.roles();
            assert_eq!(roles.primitives().accent, super::hex(accent), "{name}");
        }
    }

    /// The spec's guarantees 2-6, measured on every theme, role and fill. Collects every
    /// failure before asserting, so one run shows the whole picture.
    #[test]
    fn every_role_reads_on_every_fill() {
        use crate::roles::{
            FILL_SEP, FILLS, Fill, INK_SEP, INKS, Ink, MARK_FLOOR, STACKING, TEXT_FLOOR, TIER_STEP,
            contrast, oklab_distance,
        };
        let mut failures = Vec::new();
        for &(name, _) in NAMED {
            let theme = resolve(Some(name));
            let roles = *theme.palette.roles();
            // 8: the primitives come back exact.
            let prim = roles.primitives();
            let expected = theme_anchors(name);
            if prim.base != expected.0 || prim.text != expected.1 {
                failures.push(format!("{name}: primitives changed"));
            }
            for on in FILLS {
                let bg = roles.fill(on);
                // 2: syntax colors keep their plain-background legibility on every fill. A dim
                // comment-like gray is the hardest case.
                let gray = super::blend(prim.text, prim.base, 0.45);
                let lifted = legible(gray, bg, prim.base, roles.ink(Ink::Text, on));
                let want = contrast(gray, prim.base).min(TEXT_FLOOR) - 0.05;
                if contrast(lifted, bg) < want {
                    failures.push(format!("{name}: syntax gray on {on:?} below {want:.2}"));
                }
                // 2: text floors; 3: glyph floors.
                for ink in INKS {
                    let floor = match ink {
                        Ink::TextMuted | Ink::Pending | Ink::Border => MARK_FLOOR,
                        _ => TEXT_FLOOR,
                    };
                    let c = contrast(roles.ink(ink, on), bg);
                    if c < floor - 0.005 {
                        failures.push(format!("{name}: {ink:?} text on {on:?} {c:.2} < {floor}"));
                    }
                    let m = contrast(roles.mark(ink, on), bg);
                    if m < MARK_FLOOR - 0.005 {
                        failures.push(format!("{name}: {ink:?} mark on {on:?} {m:.2} < 3"));
                    }
                }
                // 4: the tiers keep their order and a visible step.
                let tier = |ink| contrast(roles.ink(ink, on), bg);
                let (t, s, m) = (tier(Ink::Text), tier(Ink::TextSecondary), tier(Ink::TextMuted));
                if t < s * TIER_STEP - 0.01 || s < m * TIER_STEP - 0.01 {
                    failures.push(format!("{name}: tiers on {on:?} {t:.2} / {s:.2} / {m:.2}"));
                }
            }
            // 5: every fill reads as a fill over what it can sit on.
            for (top, unders) in STACKING {
                for &under in unders {
                    let d = oklab_distance(roles.fill(top), roles.fill(under));
                    if d < FILL_SEP - 0.0005 {
                        failures.push(format!("{name}: {top:?} over {under:?} ΔE {d:.3}"));
                    }
                }
            }
            // 6: inks that appear side by side read as different.
            let beside: &[(Ink, &[Ink])] = &[
                (
                    Ink::Accent,
                    &[Ink::Added, Ink::Removed, Ink::Modified, Ink::Comment, Ink::Merged],
                ),
                (Ink::Comment, &[Ink::Added, Ink::Removed, Ink::Modified]),
            ];
            for &(a, others) in beside {
                for &b in others {
                    let d = oklab_distance(roles.ink(a, Fill::Base), roles.ink(b, Fill::Base));
                    if d < INK_SEP - 0.0005 {
                        failures.push(format!("{name}: {a:?} beside {b:?} ΔE {d:.3}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    }

    #[test]
    fn every_named_theme_resolves_to_itself() {
        for &(name, _) in NAMED {
            assert_eq!(resolve(Some(name)).name, name, "{name} should resolve to its own palette");
        }
    }

    #[test]
    fn appearance_orients_text_against_surface() {
        for &(name, light) in NAMED {
            let p = resolve(Some(name)).palette;
            // Light theme: dark text on a lighter surface. Dark theme: the reverse.
            let dark = Color::Rgb(0, 0, 0);
            let (text, bar) = (p.ink(Ink::Text, Fill::Base), p.fill(Fill::Bar));
            let text_darker = contrast(text, dark) < contrast(bar, dark);
            assert_eq!(text_darker, light, "{name}: text/surface contrast points the wrong way");
        }
    }
}
