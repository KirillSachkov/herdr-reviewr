//! A file tab's rendered markdown view: the reviewer's choice, the content it renders, the
//! render itself, its change marks, and the index over its rows. One unit, so a tab switch and
//! a config recovery move it whole.

use crate::diff::{MarkerKind, RenderedKind, Row};
use crate::marks::{DocMap, MarkMap, Unit, landing};
use std::collections::HashMap;

/// The open file's rendered view in one file tab.
#[derive(Debug, Default)]
pub(crate) struct RenderedView {
    /// The reviewer's choice: rendered, or source. Every file open resets it to rendered, so
    /// `m` flips one file and never the next.
    pub on: bool,
    /// The open markdown file's current content, the render's input. Empty whenever the
    /// content does not render: a non-markdown file, a notice, or an empty new side.
    pub text: String,
    /// The old side in the `Changes` tab — the document its deleted lines belonged to — empty
    /// elsewhere.
    pub old: String,
    /// The old side's source map, with the old text and open `<details>` it came from: the
    /// width never moves it, so a resize renders the new side alone.
    pub old_map: Option<(String, Vec<String>, DocMap)>,
    /// The reviewer's own `<details>` choices, by key: open or closed. A disclosure without
    /// one opens while it holds a change or a comment ([`crate::marks::open_details`]).
    pub details: HashMap<String, bool>,
    /// The render behind the rows: the styled lines a `Row::Rendered` indexes, each line's
    /// links and `<details>`, and the heading anchors.
    pub doc: crate::markdown::Rendered,
    /// The change marks of the rows on screen.
    pub marks: MarkMap,
    /// The index over the rows on screen.
    pub index: RenderedIndex,
    /// What the rows on screen were built from, and whether that content rendered nothing.
    /// `None` forces the next build.
    pub built: Option<Built>,
}

impl RenderedView {
    /// A fresh view: rendered, nothing built yet.
    pub(crate) fn new() -> Self {
        Self { on: true, ..Self::default() }
    }

    /// Whether the open file asks for rendered rows: the choice armed over markdown content.
    pub(crate) fn wants(&self) -> bool {
        self.on && !self.text.is_empty()
    }

    /// Whether the current content was found to render no rows at all: its source shows,
    /// though the choice stays armed.
    pub(crate) fn renders_nothing(&self) -> bool {
        self.built.as_ref().is_some_and(|b| b.empty && b.input.text == self.text)
    }

    /// Whether the rows on screen are the rendered ones.
    pub(crate) fn shows(&self) -> bool {
        self.wants() && !self.renders_nothing()
    }
}

/// One build of the rendered rows: its input, and whether the content rendered nothing.
#[derive(Debug)]
pub(crate) struct Built {
    pub input: RenderedInput,
    pub empty: bool,
}

/// The input rendered rows build from — the content, the open `<details>` keys (sorted), the
/// wrap width, the theme, the changes the marks read, and the `m` label the markers name. Two
/// equal inputs build the same rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RenderedInput {
    pub text: String,
    pub details: Vec<String>,
    pub width: usize,
    pub theme: &'static str,
    /// A digest of the diff's changed lines in the `Changes` tab, `0` elsewhere: a scope
    /// switch moves the marks without touching the text.
    pub changes: u64,
    pub see: String,
}

/// A rendered row's identity across rebuilds: a block's line by its block's first source
/// line and its index among the block's rows, or a marker row by its unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowId {
    Block { src: u32, offset: u32 },
    Marker { src: u32, kind: MarkerKind },
}

impl RowId {
    /// The identity of rendered row `row`; `None` for any other row.
    pub(crate) fn of(row: &Row) -> Option<Self> {
        match row {
            Row::Rendered { src, kind: RenderedKind::Block { offset, .. }, .. } => {
                Some(RowId::Block { src: *src, offset: *offset })
            }
            Row::Rendered { src, kind: RenderedKind::Marker(kind), .. } => {
                Some(RowId::Marker { src: *src, kind: *kind })
            }
            _ => None,
        }
    }

    /// The source line it names.
    pub(crate) fn src(self) -> u32 {
        match self {
            RowId::Block { src, .. } | RowId::Marker { src, .. } => src,
        }
    }

    /// The same identity at another source line.
    pub(crate) fn at(self, src: u32) -> Self {
        match self {
            RowId::Block { offset, .. } => RowId::Block { src, offset },
            RowId::Marker { kind, .. } => RowId::Marker { src, kind },
        }
    }
}

/// The unit a rendered row belongs to: its block, or the marker it is.
pub(crate) fn unit_of(row: &Row) -> Option<Unit> {
    match row {
        Row::Rendered { src, kind: RenderedKind::Block { .. }, .. } => Some(Unit::Block(*src)),
        Row::Rendered { src, kind: RenderedKind::Marker(kind), .. } => {
            Some(Unit::Marker(*src, *kind))
        }
        _ => None,
    }
}

/// One unit's run of rendered rows: its rows `start..end`, its source range, and its lead row
/// — its first line with text, where it reads as starting (a block's leading rows are the
/// blank gap the renderer sets above it), where a flip lands and the gutter numbers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnitRows {
    pub unit: Unit,
    pub start: usize,
    pub end: usize,
    pub src: u32,
    pub src_end: u32,
    pub lead: usize,
}

/// The index over a build's rendered rows: each unit's run in row order. Built once per
/// rebuild; every place that groups rendered rows, or lands a source line on them, reads it.
#[derive(Clone, Debug, Default)]
pub(crate) struct RenderedIndex {
    units: Vec<UnitRows>,
    by_unit: HashMap<Unit, usize>,
    /// The blocks' positions in `units`, and their source ranges, for the landing rule.
    blocks: Vec<usize>,
    block_ranges: Vec<(u32, u32)>,
}

impl RenderedIndex {
    pub(crate) fn build(rows: &[Row]) -> Self {
        let mut index = Self::default();
        let mut start = 0;
        while start < rows.len() {
            let (Some(unit), Row::Rendered { src, src_end, .. }) =
                (unit_of(&rows[start]), &rows[start])
            else {
                start += 1;
                continue;
            };
            let end = start + rows[start..].iter().take_while(|r| unit_of(r) == Some(unit)).count();
            let lead = match unit {
                Unit::Marker(..) => start,
                Unit::Block(_) => {
                    start
                        + rows[start..end]
                            .iter()
                            .position(|r| !r.text().trim().is_empty())
                            .unwrap_or(0)
                }
            };
            if let Unit::Block(_) = unit {
                index.blocks.push(index.units.len());
                index.block_ranges.push((*src, *src_end));
            }
            index.by_unit.insert(unit, index.units.len());
            index.units.push(UnitRows { unit, start, end, src: *src, src_end: *src_end, lead });
            start = end;
        }
        index
    }

    /// Every unit's run, in row order.
    pub(crate) fn units(&self) -> &[UnitRows] {
        &self.units
    }

    /// The run of `unit`.
    pub(crate) fn get(&self, unit: Unit) -> Option<&UnitRows> {
        self.units.get(*self.by_unit.get(&unit)?)
    }

    /// The position of `unit`'s run in [`Self::units`].
    pub(crate) fn position(&self, unit: Unit) -> Option<usize> {
        self.by_unit.get(&unit).copied()
    }

    /// The run row `row` belongs to.
    pub(crate) fn unit_at(&self, row: usize) -> Option<&UnitRows> {
        let k = self.units.partition_point(|u| u.start <= row).checked_sub(1)?;
        self.units.get(k).filter(|u| row < u.end)
    }

    /// The block source line `line` lands on, by the landing rule ([`landing`]).
    pub(crate) fn land(&self, line: Option<u32>) -> Option<&UnitRows> {
        landing(&self.block_ranges, line).map(|k| &self.units[self.blocks[k]])
    }

    /// The row source line `line` lands on: the lead row of its landing block.
    pub(crate) fn row_at_line(&self, line: u32) -> Option<usize> {
        self.land(Some(line)).map(|u| u.lead)
    }

    /// The row `id` reconciles onto (Continuity): the same unit at the same line of it,
    /// clamped to the unit. A block's line inside another block (a line prepended to its
    /// paragraph) keeps its place there: the offset grows by the line's distance from that
    /// block's start. Else the lead row of the block it lands on. `None` only over no rows.
    pub(crate) fn row_of(&self, id: RowId) -> Option<usize> {
        let (unit, offset) = match id {
            RowId::Block { src, offset } => (Unit::Block(src), offset),
            RowId::Marker { src, kind } => (Unit::Marker(src, kind), 0),
        };
        if let Some(u) = self.get(unit) {
            return Some(u.start + (offset as usize).min(u.end - u.start - 1));
        }
        let src = id.src();
        let u = self.land(Some(src))?;
        match id {
            RowId::Block { offset, .. } if (u.src..=u.src_end).contains(&src) => {
                let want = offset.saturating_add(src - u.src) as usize;
                Some(u.start + want.min(u.end - u.start - 1))
            }
            _ => Some(u.lead),
        }
    }
}
