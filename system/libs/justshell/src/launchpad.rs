//! The launchpad's grid: columns and rows for the screen, pages, and the
//! search that ignores case and accents.

use alloc::string::String;

/// Width of an icon cell's row, and the gap between rows.
pub const ROW_H: i32 = 118;
pub const ROW_GAP: i32 = 28;
pub const ICON: i32 = 78;
/// The page dots under the grid.
pub const DOTS_GAP: i32 = 22;
pub const DOT: i32 = 8;
/// The grid sits higher than the middle by this (the bar is under it).
pub const BOTTOM_PAD: i32 = 96;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    pub cols: usize,
    pub rows: usize,
    /// The grid's width and top-left corner, and one page's height.
    pub w: i32,
    pub x: i32,
    pub y: i32,
    pub page_h: i32,
}

impl Layout {
    pub fn per_page(&self) -> usize {
        self.cols * self.rows
    }

    /// How many pages `n` apps fill (at least one).
    pub fn pages(&self, n: usize) -> usize {
        n.div_ceil(self.per_page()).max(1)
    }

    /// The cell of the `i`th app on its page: x, y, width.
    pub fn cell(&self, i: usize) -> (i32, i32, i32) {
        let i = i % self.per_page();
        let cw = self.w / self.cols as i32;
        let (c, r) = ((i % self.cols) as i32, (i / self.cols) as i32);
        (self.x + c * cw, self.y + r * (ROW_H + ROW_GAP), cw)
    }

    /// Where the page dots go (their row's top).
    pub fn dots_y(&self) -> i32 {
        self.y + self.page_h + DOTS_GAP
    }
}

/// The grid for a screen of `sw` x `sh`.
pub fn layout(sw: i32, sh: i32) -> Layout {
    let w = 1080.min(sw * 92 / 100);
    let cols = if w > 980 {
        7
    } else if w > 800 {
        6
    } else if w > 620 {
        5
    } else {
        4
    };
    let rows = if sh > 840 { 4 } else { 3 };
    let page_h = rows as i32 * ROW_H + (rows as i32 - 1) * ROW_GAP;
    let block = page_h + DOTS_GAP + DOT;
    let y = ((sh - BOTTOM_PAD - block) / 2).max(16);
    Layout { cols, rows, w, x: (sw - w) / 2, y, page_h }
}

/// Lowercase, without accents (the search matches "citta" with "Città").
pub fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' | 'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => 'a',
            'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            'ñ' | 'Ñ' => 'n',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// Does `name` contain what was typed?
pub fn matches(name: &str, query: &str) -> bool {
    let q = fold(query.trim());
    q.is_empty() || fold(name).contains(q.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_rows_follow_the_screen() {
        assert_eq!(layout(1440, 1024).cols, 7);
        assert_eq!(layout(1440, 1024).rows, 4);
        assert_eq!(layout(1024, 768).cols, 6); // 942
        assert_eq!(layout(1024, 768).rows, 3);
        assert_eq!(layout(800, 600).cols, 5); // 736
        assert_eq!(layout(600, 600).cols, 4);
        assert_eq!(layout(1920, 1080).w, 1080);
    }

    #[test]
    fn pages_and_cells() {
        let l = layout(1440, 1024);
        assert_eq!(l.per_page(), 28);
        assert_eq!(l.pages(0), 1);
        assert_eq!(l.pages(28), 1);
        assert_eq!(l.pages(29), 2);
        let (x0, y0, cw) = l.cell(0);
        assert_eq!((x0, y0), (l.x, l.y));
        assert_eq!(l.cell(7), (l.x, l.y + ROW_H + ROW_GAP, cw));
        assert_eq!(l.cell(28), l.cell(0));
        assert_eq!(l.page_h, 4 * 118 + 3 * 28);
    }

    #[test]
    fn search_ignores_case_and_accents() {
        assert!(matches("Città", "citta"));
        assert!(matches("Impostazioni", "POST"));
        assert!(matches("Note", ""));
        assert!(!matches("Musica", "foto"));
        assert_eq!(fold("Perché È"), "perche e");
    }
}
