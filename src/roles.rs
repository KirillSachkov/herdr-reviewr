//! Semantic color roles: what a color is for, resolved per theme so it reads everywhere.
//!
//! A theme supplies [`Primitives`]: its background, text, six hues and its UI accent. One
//! shared rule set derives every [`Fill`] (a layer under text) and every [`Ink`] (a color text
//! or a glyph paints with), each ink resolved once per fill it can sit on. Components ask for
//! a role on a fill and never see a primitive, so a theme's look and its legibility live here
//! alone. Rules, in order:
//!
//! - A fill is a step or a tint off `base`. It softens toward `base` until body text reads on it,
//!   but never into `base`: it stays visibly a fill. The match highlight is the one solid fill:
//!   it must be found at a glance, so it never softens.
//! - Body text that still falls short lifts toward the contrast pole, on that fill only. On a
//!   fill bright enough that the theme's background reads better than its text, text takes the
//!   background's side and the opposite pole.
//! - The secondary and muted tiers sit at fixed shares of the text's contrast, floored at their
//!   minimums, so the three tiers keep their order and a visible step on every fill.
//! - A colored ink keeps its official color wherever it clears its floor, and lifts toward the
//!   pole on the fills where it doesn't.
//! - Roles that can appear side by side keep a minimum perceptual distance: the lower-priority
//!   one takes the theme's next candidate hue.

use ratatui::style::Color;

/// A color text or a glyph paints with, by what it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// Body text, file names, the footer's status message.
    Text,
    /// Labels, inactive tabs, footer labels, other people's PR comment authors.
    TextSecondary,
    /// Line numbers, placeholders, trails, empty-state hints.
    TextMuted,
    /// Focus, the caret, and everything you can act on.
    Accent,
    /// Your comments.
    Comment,
    Added,
    Removed,
    Modified,
    Success,
    Danger,
    /// Warnings, and checks still running or queued.
    Warning,
    /// The merged PR chip.
    Merged,
    /// Pane borders, rules, separators.
    Border,
}

/// Every ink, in table order.
pub const INKS: [Ink; 13] = [
    Ink::Text,
    Ink::TextSecondary,
    Ink::TextMuted,
    Ink::Accent,
    Ink::Comment,
    Ink::Added,
    Ink::Removed,
    Ink::Modified,
    Ink::Success,
    Ink::Danger,
    Ink::Warning,
    Ink::Merged,
    Ink::Border,
];

/// A layer text sits on. Fills stack in one order (topmost first): selection, highlight,
/// cursor, word emphasis, diff row, bar or code chip, the terminal background. A selection is text you
/// dragged over or a line range you picked for a comment: one fill, one meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// The terminal's own background, which the theme's `base` stands for.
    Base,
    /// Header, footer and fold rows.
    Bar,
    /// Inline code's chip in rendered markdown.
    Code,
    /// The cursor row in an unfocused pane.
    CursorInactive,
    /// The cursor row in the focused pane.
    Cursor,
    /// A text selection, or a line range picked for a comment.
    Selection,
    /// A find or search match: a solid block of the highlight hue.
    Highlight,
    Added,
    Removed,
    /// Changed words inside an added row.
    AddedEmph,
    /// Changed words inside a removed row.
    RemovedEmph,
}

/// Every fill, in table order.
pub const FILLS: [Fill; 11] = [
    Fill::Base,
    Fill::Bar,
    Fill::Code,
    Fill::CursorInactive,
    Fill::Cursor,
    Fill::Selection,
    Fill::Highlight,
    Fill::Added,
    Fill::Removed,
    Fill::AddedEmph,
    Fill::RemovedEmph,
];

/// A theme's cast, which sets the direction steps and lifts move in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cast {
    Dark,
    Light,
}

/// The colors a theme supplies. Exact upstream values; nothing here is ever adjusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Primitives {
    pub base: Color,
    pub text: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub orange: Color,
    pub purple: Color,
    pub blue: Color,
    /// The theme's UI accent: focus, selection, primary UI in its own ports.
    pub accent: Color,
    pub cast: Cast,
}

/// Fills a theme sets itself because upstream ships an official value. They pass the same
/// checks as derived ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    fills: [Option<Color>; FILLS.len()],
}

impl Overrides {
    #[must_use]
    pub fn fill(mut self, fill: Fill, color: Color) -> Self {
        self.fills[fill as usize] = Some(color);
        self
    }
}

/// Lowest contrast for text, WCAG AA.
pub const TEXT_FLOOR: f64 = 4.5;
/// Lowest contrast for muted text, glyphs and borders.
pub const MARK_FLOOR: f64 = 3.0;
/// The smallest contrast ratio between neighboring text tiers that reads as a step.
pub const TIER_STEP: f64 = 1.2;
/// Body text's target on a fill: room for the secondary tier one step below it at its floor,
/// with a hair of margin for 8-bit rounding.
const TEXT_TARGET: f64 = TEXT_FLOOR * TIER_STEP + 0.05;
/// How far a fill may soften, as a share of its starting strength. Below it a fill stops
/// reading as its own layer, so body text lifts instead.
const MIN_STRENGTH: f64 = 0.6;
/// The secondary tier's share of body text's contrast.
const SECONDARY_SHARE: f64 = 0.70;
/// The muted tier's share of body text's contrast.
const MUTED_SHARE: f64 = 0.45;
/// A border's share of body text's contrast, floored at [`MARK_FLOOR`].
const BORDER_SHARE: f64 = 0.27;
/// The smallest `OKLab` distance at which a fill reads as distinct from the one beneath it.
pub const FILL_SEP: f64 = 0.03;
/// The smallest `OKLab` distance at which two inks that appear side by side read as different:
/// about two and a half just-noticeable differences.
pub const INK_SEP: f64 = 0.05;

/// Every role a theme paints, resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles {
    primitives: Primitives,
    fills: [Color; FILLS.len()],
    text: [[Color; FILLS.len()]; INKS.len()],
    mark: [[Color; FILLS.len()]; INKS.len()],
}

impl Roles {
    /// The color of `fill`.
    #[must_use]
    pub fn fill(&self, fill: Fill) -> Color {
        self.fills[fill as usize]
    }

    /// `ink` painting text on `on`: clears [`TEXT_FLOOR`] there ([`MARK_FLOOR`] for the muted
    /// tier).
    #[must_use]
    pub fn ink(&self, ink: Ink, on: Fill) -> Color {
        self.text[ink as usize][on as usize]
    }

    /// `ink` painting a glyph, a sign or a border on `on`: clears [`MARK_FLOOR`] there.
    #[must_use]
    pub fn mark(&self, ink: Ink, on: Fill) -> Color {
        self.mark[ink as usize][on as usize]
    }

    /// The primitives these roles were derived from, unchanged.
    #[must_use]
    pub fn primitives(&self) -> Primitives {
        self.primitives
    }

    /// Derive every role from `p`, taking `o`'s fills as given.
    #[must_use]
    pub fn derive(p: Primitives, o: Overrides) -> Self {
        let hues = Hues::resolve(&p);
        let fills = derive_fills(&p, &hues, &o);
        let mut text = [[p.base; FILLS.len()]; INKS.len()];
        let mut mark = [[p.base; FILLS.len()]; INKS.len()];
        for fill in FILLS {
            let bg = fills[fill as usize];
            let tiers = Tiers::on(&p, bg);
            for ink in INKS {
                let (t, m) = match ink {
                    Ink::Text => (tiers.text, tiers.text),
                    Ink::TextSecondary => (tiers.secondary, tiers.secondary),
                    Ink::TextMuted => (tiers.muted, tiers.muted),
                    Ink::Border => (tiers.border, tiers.border),
                    colored => {
                        let hue = hues.of(colored);
                        (
                            lift(hue, bg, pole_on(bg), TEXT_FLOOR),
                            lift(hue, bg, pole_on(bg), MARK_FLOOR),
                        )
                    }
                };
                text[ink as usize][fill as usize] = t;
                mark[ink as usize][fill as usize] = m;
            }
        }
        Roles { primitives: p, fills, text, mark }
    }
}

/// The hue each colored ink takes, after side-by-side distinctness picked among candidates.
struct Hues {
    accent: Color,
    comment: Color,
    merged: Color,
    added: Color,
    removed: Color,
    modified: Color,
}

impl Hues {
    /// Resolve in priority order: the diff and status hues are fixed, then `comment`, then
    /// `accent`, then `merged`, each taking its first candidate that keeps [`INK_SEP`] from
    /// every higher role it can appear beside. `comment` also steers clear of the theme's own
    /// accent, so a comment never takes the color that marks focus.
    fn resolve(p: &Primitives) -> Self {
        let (added, removed, modified) = (p.green, p.red, p.yellow);
        let pick = |candidates: &[Color], taken: &[Color]| first_distinct(p, candidates, taken);
        let comment = pick(&[p.orange, p.blue, p.purple], &[added, removed, modified, p.accent]);
        let accent =
            pick(&[p.accent, p.blue, p.purple, p.orange], &[added, removed, modified, comment]);
        // A PR is open, merged or closed, never two at once, so `merged` only has to stand
        // apart from the accent of the PR number chip beside it.
        let merged = pick(&[p.purple, p.blue, p.orange], &[accent]);
        Hues { accent, comment, merged, added, removed, modified }
    }

    fn of(&self, ink: Ink) -> Color {
        match ink {
            Ink::Accent => self.accent,
            Ink::Comment => self.comment,
            Ink::Merged => self.merged,
            Ink::Added | Ink::Success => self.added,
            Ink::Removed | Ink::Danger => self.removed,
            Ink::Modified | Ink::Warning => self.modified,
            _ => unreachable!("tier inks resolve in Tiers"),
        }
    }
}

/// The first candidate at least [`INK_SEP`] from every `taken` color, compared as each paints
/// on `base` (lifted to its text floor); when none is, the one farthest from its nearest taken
/// color.
fn first_distinct(p: &Primitives, candidates: &[Color], taken: &[Color]) -> Color {
    let paints = |c: Color| lift(c, p.base, pole(p.cast), TEXT_FLOOR);
    let nearest = |c: Color| {
        taken.iter().map(|&t| oklab_distance(paints(c), paints(t))).fold(f64::MAX, f64::min)
    };
    candidates.iter().copied().find(|&c| nearest(c) >= INK_SEP).unwrap_or_else(|| {
        candidates.iter().copied().max_by(|&a, &b| nearest(a).total_cmp(&nearest(b))).unwrap()
    })
}

/// The three text tiers and the border, resolved on one background.
struct Tiers {
    text: Color,
    secondary: Color,
    muted: Color,
    border: Color,
}

impl Tiers {
    fn on(p: &Primitives, bg: Color) -> Self {
        // Text keeps the theme's side unless the theme's background reads better on this fill.
        let side = if contrast(p.text, bg) >= contrast(p.base, bg) { p.text } else { p.base };
        let text = lift(side, bg, pole_on(bg), TEXT_TARGET);
        let tc = contrast(text, bg);
        let toward_bg = |target: f64| fade(text, bg, target);
        Tiers {
            text,
            secondary: toward_bg((tc * SECONDARY_SHARE).max(TEXT_FLOOR)),
            muted: toward_bg((tc * MUTED_SHARE).max(MARK_FLOOR)),
            border: toward_bg((tc * BORDER_SHARE).max(MARK_FLOOR)),
        }
    }
}

/// Which fills each fill can sit on, in stacking order bottom up: a fill must read as a layer
/// over every one of them.
pub const STACKING: [(Fill, &[Fill]); 10] = [
    (Fill::Bar, &[Fill::Base]),
    (Fill::Code, &[Fill::Base]),
    (Fill::CursorInactive, &[Fill::Base]),
    (Fill::Added, &[Fill::Base]),
    (Fill::Removed, &[Fill::Base]),
    (Fill::AddedEmph, &[Fill::Added]),
    (Fill::RemovedEmph, &[Fill::Removed]),
    (Fill::Cursor, &[Fill::Base, Fill::Added, Fill::Removed]),
    (Fill::Highlight, &[Fill::Base, Fill::Cursor, Fill::Added, Fill::Removed]),
    (Fill::Selection, &[Fill::Base, Fill::Cursor, Fill::Highlight, Fill::Added, Fill::Removed]),
];

/// The strongest a derived fill may get while making room over the fills beneath it.
const MAX_STRENGTH: f64 = 0.6;

/// Every fill: overrides as given, the rest derived bottom up. A derived fill starts at its
/// strength and softens toward `base` until body text clears its target, no further than
/// [`MIN_STRENGTH`] of the start. Then it strengthens, if it must, until it reads as a layer
/// over every fill it can sit on; body text lifts on it instead.
fn derive_fills(p: &Primitives, hues: &Hues, o: &Overrides) -> [Color; FILLS.len()] {
    let mut fills = [p.base; FILLS.len()];
    for (fill, unders) in STACKING {
        if let Some(given) = o.fills[fill as usize] {
            fills[fill as usize] = given;
            continue;
        }
        // A match must be found at a glance, so it paints the highlight hue solid; its text
        // takes whichever side reads on it.
        if fill == Fill::Highlight {
            fills[fill as usize] = hues.modified;
            continue;
        }
        let (toward, start) = recipe(p, hues, fill);
        let at = |t: f64| blend(p.base, toward, t);
        let mut t = start;
        while t - 0.01 >= start * MIN_STRENGTH && contrast(p.text, at(t)) < TEXT_TARGET {
            t -= 0.01;
        }
        let distinct = |c: Color| {
            unders.iter().all(|&under| oklab_distance(c, fills[under as usize]) >= FILL_SEP)
        };
        while !distinct(at(t)) && t + 0.01 <= MAX_STRENGTH {
            t += 0.01;
        }
        fills[fill as usize] = at(t);
    }
    fills
}

/// What a derived fill blends `base` toward, and how far it starts.
fn recipe(p: &Primitives, hues: &Hues, fill: Fill) -> (Color, f64) {
    let dark = p.cast == Cast::Dark;
    match fill {
        Fill::Base => (p.base, 0.0),
        Fill::Bar | Fill::Code => (pole(p.cast), 0.045),
        Fill::CursorInactive => (pole(p.cast), 0.09),
        Fill::Cursor => (pole(p.cast), 0.14),
        Fill::Selection => (saturated(hues.accent), if dark { 0.38 } else { 0.22 }),
        Fill::Highlight => (hues.modified, 1.0),
        Fill::Added => (hues.added, if dark { 0.20 } else { 0.12 }),
        Fill::Removed => (hues.removed, if dark { 0.20 } else { 0.12 }),
        Fill::AddedEmph => (hues.added, if dark { 0.38 } else { 0.22 }),
        Fill::RemovedEmph => (hues.removed, if dark { 0.38 } else { 0.22 }),
    }
}

/// The contrast pole a theme lifts toward: white on a dark theme, black on a light one.
fn pole(cast: Cast) -> Color {
    match cast {
        Cast::Dark => Color::Rgb(0xff, 0xff, 0xff),
        Cast::Light => Color::Rgb(0x00, 0x00, 0x00),
    }
}

/// The pole that reads best on `bg`: black on a bright fill, white on a dark one.
fn pole_on(bg: Color) -> Color {
    let (black, white) = (Color::Rgb(0, 0, 0), Color::Rgb(0xff, 0xff, 0xff));
    if contrast(black, bg) >= contrast(white, bg) { black } else { white }
}

/// `fg` blended toward `toward` just far enough to clear `min` on `bg`; `fg` itself when it
/// already does. A `Color::Reset` target means "as is".
pub(crate) fn lift(fg: Color, bg: Color, toward: Color, min: f64) -> Color {
    if toward == Color::Reset || contrast(fg, bg) >= min {
        return fg;
    }
    let mut t = 0.0;
    while t < 1.0 {
        let lifted = blend(fg, toward, t);
        if contrast(lifted, bg) >= min {
            return lifted;
        }
        t += 0.01;
    }
    toward
}

/// `fg` blended toward `bg` as far as it can go while keeping `target` contrast on it.
fn fade(fg: Color, bg: Color, target: f64) -> Color {
    if contrast(fg, bg) <= target {
        return fg;
    }
    // Contrast falls monotonically along the blend, so bisect for the last blend that keeps it.
    let (mut keep, mut lose) = (0.0_f64, 1.0_f64);
    for _ in 0..24 {
        let mid = f64::midpoint(keep, lose);
        if contrast(blend(fg, bg, mid), bg) >= target { keep = mid } else { lose = mid }
    }
    blend(fg, bg, keep)
}

/// Halfway between a hue and its colorful core, so a pastel tints `base` into a clear hue.
pub(crate) fn saturated(c: Color) -> Color {
    let (r, g, b) = channels(c);
    let lo = r.min(g).min(b);
    let span = r.max(g).max(b) - lo;
    if span == 0 {
        return c;
    }
    let core = |ch: u8| (f64::from(ch - lo) * 255.0 / f64::from(span)).round() as u8;
    blend(c, Color::Rgb(core(r), core(g), core(b)), 0.5)
}

/// Linear per-channel blend: `t` of the way from `from` to `to`.
pub(crate) fn blend(from: Color, to: Color, t: f64) -> Color {
    let (fr, fg, fb) = channels(from);
    let (tr, tg, tb) = channels(to);
    let mix = |lhs: u8, rhs: u8| (f64::from(lhs) * (1.0 - t) + f64::from(rhs) * t).round() as u8;
    Color::Rgb(mix(fr, tr), mix(fg, tg), mix(fb, tb))
}

/// The WCAG contrast ratio between two colors (1.0 .. 21.0).
#[must_use]
pub fn contrast(fg: Color, bg: Color) -> f64 {
    let (lf, lb) = (luminance(fg), luminance(bg));
    let (hi, lo) = if lf >= lb { (lf, lb) } else { (lb, lf) };
    (hi + 0.05) / (lo + 0.05)
}

fn linear(channel: u8) -> f64 {
    let srgb = f64::from(channel) / 255.0;
    if srgb <= 0.040_45 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055).powf(2.4) }
}

/// WCAG relative luminance.
fn luminance(color: Color) -> f64 {
    let (r, g, b) = channels(color);
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// Euclidean distance in `OKLab`: how different two colors look.
#[must_use]
pub fn oklab_distance(a: Color, b: Color) -> f64 {
    let (l1, a1, b1) = oklab(a);
    let (l2, a2, b2) = oklab(b);
    ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
}

// The OKLab matrices name their channels by convention: l, m, s and L, a, b.
#[allow(clippy::many_single_char_names)]
fn oklab(color: Color) -> (f64, f64, f64) {
    let (r, g, b) = channels(color);
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    (
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    )
}

fn channels(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}
