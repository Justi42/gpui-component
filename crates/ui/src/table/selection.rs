use std::collections::HashSet;

use gpui::Modifiers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectDir {
    Up,
    Down,
}

/// Multi-select state over row indices, independent of what the rows hold —
/// every operation takes the current row count so the cyclic arithmetic has a
/// modulus without the selection needing to see the table.
///
/// `wrap` mirrors [`TableState::loop_selection`]: when set, navigation and
/// range extension walk a cycle; when clear, both clamp at the ends and the
/// range is simply the span between anchor and cursor.
#[derive(Debug, Default)]
pub(super) struct RowSelection {
    rows: HashSet<usize>,
    /// Fixed reference point of a range selection. Set on plain click or plain
    /// keyboard nav, and stays put for as long as Shift-extensions continue.
    anchor: Option<usize>,
    /// Moving end of the range. Advances on Shift+Arrow / Shift+Click and
    /// wraps modulo the row count. With direction tracking the cursor can sit
    /// numerically *below* the anchor while the selection still walks through
    /// the wrap point — preserving the gap on the other side of the cycle.
    cursor: Option<usize>,
    /// Which way the cursor most recently moved during a Shift-extension.
    /// Determines whether the cyclic arc walks clockwise (`Down`) or
    /// counter-clockwise (`Up`) from anchor to cursor.
    dir: Option<SelectDir>,
}

impl RowSelection {
    pub(super) fn rows(&self) -> &HashSet<usize> {
        &self.rows
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(super) fn contains(&self, row_ix: usize) -> bool {
        self.rows.contains(&row_ix)
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.anchor = None;
        self.cursor = None;
        self.dir = None;
    }

    /// Where keyboard navigation resumes from.
    pub(super) fn cursor(&self) -> Option<usize> {
        self.cursor.or(self.anchor)
    }

    pub(super) fn select_all(&mut self, row_count: usize) {
        self.rows = (0..row_count).collect();
    }

    pub(super) fn set_rows(&mut self, rows: impl IntoIterator<Item = usize>, row_count: usize) {
        self.rows = rows.into_iter().filter(|ix| *ix < row_count).collect();
        self.anchor = self.rows.iter().min().copied();
        self.cursor = self.anchor;
        self.dir = None;
    }

    /// Apply a selection change for row `ix` given the active modifiers:
    /// - No modifier  → single selection; resets anchor, cursor, and direction.
    /// - Platform (⌘) → toggle `ix` without disturbing anchor / cursor.
    /// - Shift → range from `anchor` to `ix`; cyclic (walking in `dir`) when
    ///   `wrap` is set, otherwise the plain span between the two. A full
    ///   revolution brings `ix` back onto the anchor, where the arc is a
    ///   single row, so the selection restarts from the anchor.
    pub(super) fn apply(&mut self, ix: usize, modifiers: Modifiers, row_count: usize, wrap: bool) {
        if row_count == 0 {
            return;
        }

        if modifiers.platform {
            if !self.rows.remove(&ix) {
                self.rows.insert(ix);
            }
            return;
        }

        if !modifiers.shift {
            self.rows = HashSet::from([ix]);
            self.anchor = Some(ix);
            self.cursor = Some(ix);
            self.dir = None;
            return;
        }

        // Shift extension. Establish an anchor on the first shift op.
        let anchor = *self.anchor.get_or_insert(ix);

        if !wrap {
            self.rows = (anchor.min(ix)..=anchor.max(ix)).collect();
            self.cursor = Some(ix);
            return;
        }

        // The cursor has caught up to the anchor after a full revolution; the
        // walk restarts from here rather than pivoting to the previous cursor.
        if ix == anchor && self.cursor.is_some() && self.cursor != Some(anchor) {
            self.anchor = Some(ix);
        }

        // Direction defaults to Down for Shift-clicks below the anchor (or Up
        // for above) when no explicit keyboard direction has been recorded.
        let dir = self.dir.unwrap_or(if ix >= anchor {
            SelectDir::Down
        } else {
            SelectDir::Up
        });

        self.rows = walk_selection(anchor, ix, dir, row_count);
        self.cursor = Some(ix);
    }

    /// Move the selection cursor by `delta` rows and apply the new selection.
    /// With `wrap` set, Down past the last row jumps to row 0 and vice versa;
    /// otherwise the cursor stops at the ends. Updates the active direction so
    /// `apply` knows which way to walk the cyclic arc.
    pub(super) fn move_cursor(
        &mut self,
        delta: i64,
        modifiers: Modifiers,
        row_count: usize,
        wrap: bool,
    ) {
        if row_count == 0 {
            return;
        }
        let next = match self.cursor() {
            Some(ix) => {
                let moved = ix as i64 + delta;
                if wrap {
                    moved.rem_euclid(row_count as i64) as usize
                } else {
                    moved.clamp(0, row_count as i64 - 1) as usize
                }
            }
            None if delta > 0 => 0,
            None => row_count - 1,
        };
        if modifiers.shift {
            // Only adopt the keypress direction when there isn't an active
            // walk, or when the cursor is back on the anchor and pivoting.
            // Mid-walk the reverse key shrinks the existing arc — flipping
            // direction here would jump to the short arc on the other side
            // of the anchor (visible when a wrapped Down arc is reduced by
            // Shift+Up: it shouldn't unwrap into a non-wrapped Up arc).
            let pivoting = self.cursor == self.anchor;
            if self.dir.is_none() || pivoting {
                self.dir = Some(if delta > 0 {
                    SelectDir::Down
                } else {
                    SelectDir::Up
                });
            }
        }
        self.apply(next, modifiers, row_count, wrap);
    }
}

/// Inclusive cyclic arc from `start` to `end` walking in `dir` over a cycle of
/// length `n`. `Down` walks `start → start+1 → ... → end` (with wrap); `Up`
/// walks the other way. When `start == end` the arc is just `{start}`.
fn walk_selection(start: usize, end: usize, dir: SelectDir, n: usize) -> HashSet<usize> {
    let mut arc = HashSet::with_capacity(n);
    let mut pos = start;
    arc.insert(pos);
    while pos != end {
        pos = match dir {
            SelectDir::Down => (pos + 1) % n,
            SelectDir::Up => (pos + n - 1) % n,
        };
        arc.insert(pos);
    }
    arc
}

#[cfg(test)]
mod tests {
    use super::{RowSelection, SelectDir, walk_selection};
    use gpui::Modifiers;
    use std::collections::HashSet;

    const N: usize = 10;

    fn set(rows: impl IntoIterator<Item = usize>) -> HashSet<usize> {
        rows.into_iter().collect()
    }

    fn plain() -> Modifiers {
        Modifiers::none()
    }

    fn shift() -> Modifiers {
        Modifiers::shift()
    }

    fn cmd() -> Modifiers {
        Modifiers {
            platform: true,
            ..Modifiers::none()
        }
    }

    #[test]
    fn walk_down_without_wrap() {
        assert_eq!(walk_selection(2, 5, SelectDir::Down, N), set(2..=5));
    }

    #[test]
    fn walk_down_wraps_past_the_end() {
        assert_eq!(walk_selection(8, 1, SelectDir::Down, N), set([8, 9, 0, 1]));
    }

    #[test]
    fn walk_up_wraps_past_the_start() {
        assert_eq!(walk_selection(1, 8, SelectDir::Up, N), set([1, 0, 9, 8]));
    }

    #[test]
    fn walk_of_a_single_row_is_that_row() {
        assert_eq!(walk_selection(4, 4, SelectDir::Down, N), set([4]));
    }

    #[test]
    fn plain_click_selects_one_row_and_reseats_the_anchor() {
        let mut sel = RowSelection::default();
        sel.apply(3, shift(), N, true);
        sel.apply(7, plain(), N, true);
        assert_eq!(sel.rows(), &set([7]));
        assert_eq!(sel.cursor(), Some(7));

        // The old anchor is gone: a following shift-extension starts at 7.
        sel.apply(9, shift(), N, true);
        assert_eq!(sel.rows(), &set(7..=9));
    }

    #[test]
    fn cmd_click_toggles_without_disturbing_the_range() {
        let mut sel = RowSelection::default();
        sel.apply(2, plain(), N, true);
        sel.apply(4, shift(), N, true);
        sel.apply(8, cmd(), N, true);
        assert_eq!(sel.rows(), &set([2, 3, 4, 8]));

        sel.apply(8, cmd(), N, true);
        assert_eq!(sel.rows(), &set([2, 3, 4]));

        // Anchor and cursor survived the toggles.
        sel.apply(6, shift(), N, true);
        assert_eq!(sel.rows(), &set(2..=6));
    }

    #[test]
    fn shift_click_above_the_anchor_walks_upward() {
        let mut sel = RowSelection::default();
        sel.apply(6, plain(), N, true);
        sel.apply(3, shift(), N, true);
        assert_eq!(sel.rows(), &set(3..=6));
    }

    #[test]
    fn shift_arrow_extends_and_wraps() {
        let mut sel = RowSelection::default();
        sel.apply(8, plain(), N, true);
        sel.move_cursor(1, shift(), N, true);
        assert_eq!(sel.rows(), &set([8, 9]));
        sel.move_cursor(1, shift(), N, true);
        assert_eq!(sel.rows(), &set([8, 9, 0]));
        sel.move_cursor(1, shift(), N, true);
        assert_eq!(sel.rows(), &set([8, 9, 0, 1]));
    }

    #[test]
    fn reversing_mid_walk_shrinks_the_wrapped_arc() {
        let mut sel = RowSelection::default();
        sel.apply(8, plain(), N, true);
        for _ in 0..3 {
            sel.move_cursor(1, shift(), N, true);
        }
        assert_eq!(sel.rows(), &set([8, 9, 0, 1]));

        // Shift+Up must shorten the existing Down arc, not flip to the short
        // arc on the other side of the anchor.
        sel.move_cursor(-1, shift(), N, true);
        assert_eq!(sel.rows(), &set([8, 9, 0]));
        sel.move_cursor(-1, shift(), N, true);
        assert_eq!(sel.rows(), &set([8, 9]));
    }

    #[test]
    fn shift_extension_grows_to_the_whole_table_then_restarts() {
        let mut sel = RowSelection::default();
        sel.apply(0, plain(), N, true);
        for _ in 0..N - 1 {
            sel.move_cursor(1, shift(), N, true);
        }
        assert_eq!(sel.rows(), &set(0..N));

        // One more step lands the cursor back on the anchor, and the arc from
        // the anchor to itself is a single row.
        sel.move_cursor(1, shift(), N, true);
        assert_eq!(sel.rows(), &set([0]));
    }

    #[test]
    fn plain_arrow_collapses_to_a_single_row_and_wraps() {
        let mut sel = RowSelection::default();
        sel.apply(2, plain(), N, true);
        sel.apply(5, shift(), N, true);
        sel.move_cursor(1, plain(), N, true);
        assert_eq!(sel.rows(), &set([6]));

        sel.apply(0, plain(), N, true);
        sel.move_cursor(-1, plain(), N, true);
        assert_eq!(sel.rows(), &set([N - 1]));
    }

    #[test]
    fn navigation_without_a_cursor_enters_from_the_matching_end() {
        let mut down = RowSelection::default();
        down.move_cursor(1, plain(), N, true);
        assert_eq!(down.rows(), &set([0]));

        let mut up = RowSelection::default();
        up.move_cursor(-1, plain(), N, true);
        assert_eq!(up.rows(), &set([N - 1]));
    }

    #[test]
    fn select_all_covers_every_row() {
        let mut sel = RowSelection::default();
        sel.select_all(4);
        assert_eq!(sel.rows(), &set(0..4));
    }

    #[test]
    fn an_empty_table_ignores_every_operation() {
        let mut sel = RowSelection::default();
        sel.apply(0, plain(), 0, true);
        sel.move_cursor(1, shift(), 0, true);
        assert!(sel.is_empty());
        assert_eq!(sel.cursor(), None);
    }

    #[test]
    fn without_wrap_navigation_stops_at_the_ends() {
        let mut sel = RowSelection::default();
        sel.apply(0, plain(), N, false);
        sel.move_cursor(-1, plain(), N, false);
        assert_eq!(sel.rows(), &set([0]));

        sel.apply(N - 1, plain(), N, false);
        sel.move_cursor(1, plain(), N, false);
        assert_eq!(sel.rows(), &set([N - 1]));
    }

    #[test]
    fn without_wrap_extension_is_the_span_between_anchor_and_cursor() {
        let mut sel = RowSelection::default();
        sel.apply(7, plain(), N, false);
        for _ in 0..3 {
            sel.move_cursor(1, shift(), N, false);
        }
        assert_eq!(sel.rows(), &set(7..=9));

        // Clamped at the last row, so further extension is a no-op.
        sel.move_cursor(1, shift(), N, false);
        assert_eq!(sel.rows(), &set(7..=9));

        sel.apply(2, shift(), N, false);
        assert_eq!(sel.rows(), &set(2..=7));
    }

    #[test]
    fn set_rows_seats_the_cursor_on_the_first_row() {
        let mut sel = RowSelection::default();
        sel.set_rows([5, 2, 99], N);
        assert_eq!(sel.rows(), &set([2, 5]));
        assert_eq!(sel.cursor(), Some(2));
    }

    #[test]
    fn clear_drops_the_cursor_too() {
        let mut sel = RowSelection::default();
        sel.apply(4, plain(), N, true);
        sel.clear();
        assert!(sel.is_empty());
        assert_eq!(sel.cursor(), None);
    }
}
