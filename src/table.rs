//! Tables.
//!
//! Columns are sized from an estimate of how wide each cell's content would
//! be on one line, shrunk proportionally when the table doesn't fit. Each
//! cell is then rendered at its column's width and the rows are drawn with
//! box-drawing borders. A table too narrow for its columns is written one
//! cell under another instead.

use std::collections::HashMap;
use std::rc::Rc;

use unicode_width::UnicodeWidthStr;

use crate::dom::{Handle, Node, NodeData};
use crate::text::{
    char_width, html_name, link_has_content, survives, svg_title, Line, LineKind, Renderer,
};

/// Columns are never squeezed narrower than this (unless their content is).
const MIN_COL: usize = 3;

struct Cell {
    node: Handle,
    row: usize,
    col: usize,
    rowspan: usize,
    colspan: usize,
    est: usize,
    /// How narrow the content can be written: some markup can't be split.
    min: usize,
    /// Drawn empty, whatever the node holds.
    blank: bool,
}

/// A table being written: the cells to render, at their widths (`None`
/// for those drawn empty), and then how their lines go together.
pub struct Job {
    cells: Vec<Option<(Handle, usize)>>,
    lines: Vec<Vec<Line>>,
    kind: JobKind,
}

enum JobKind {
    Done(Vec<Line>),
    Grid {
        grid: Grid,
        borders: bool,
    },
    /// Lines, and the places of the cells' lines (in order) between them.
    Vertical(Vec<Option<Line>>),
}

impl Job {
    fn done(lines: Vec<Line>) -> Job {
        Job {
            cells: Vec::new(),
            lines: Vec::new(),
            kind: JobKind::Done(lines),
        }
    }

    /// The next cell to render, and its width.
    pub fn next_cell(&mut self) -> Option<(Handle, usize)> {
        while let Some(cell) = self.cells.get(self.lines.len()) {
            match cell {
                Some((node, width)) => return Some((node.clone(), *width)),
                None => self.lines.push(Vec::new()),
            }
        }
        None
    }

    /// The lines of the cell `next_cell` gave.
    pub fn add(&mut self, lines: Vec<Line>) {
        self.lines.push(lines);
    }

    /// The table's lines, once its cells are rendered.
    pub fn finish(self) -> Vec<Line> {
        match self.kind {
            JobKind::Done(lines) => lines,
            JobKind::Grid { grid, borders } => grid.draw(self.lines, borders),
            JobKind::Vertical(pieces) => {
                let mut cells = self.lines.into_iter();
                let mut out = Vec::new();
                for piece in pieces {
                    match piece {
                        Some(line) => out.push(line),
                        None => out.extend(cells.next().unwrap_or_default()),
                    }
                }
                out
            }
        }
    }
}

/// Start a table: size it and lay it out, leaving its cells to render.
pub fn start(r: &mut Renderer, table: &Handle) -> Job {
    let rows = collect_rows(table);
    r.begin_table(has_row(table));
    let decorate = r.opts().decorate;
    if !r.table_sizes.contains_key(&Rc::as_ptr(table)) {
        size_tables(table, decorate, &mut r.table_sizes);
    }
    let mut cells = place(&rows);
    for cell in &mut cells {
        cell.est = estimate(&cell.node, decorate, &r.table_sizes);
        cell.min = min_estimate(&cell.node, decorate, &r.table_sizes);
    }
    let width = r.width();
    layout(r, &mut cells, rows.len(), width)
}

fn has_row(table: &Handle) -> bool {
    table.children.borrow().iter().any(|c| match html_name(c) {
        Some("tr") => true,
        Some("thead" | "tbody") => c
            .children
            .borrow()
            .iter()
            .any(|tr| html_name(tr) == Some("tr")),
        _ => false,
    })
}

/// The rows of the table, each a list of its cells, in document order.
fn collect_rows(table: &Handle) -> Vec<Vec<Handle>> {
    let mut rows = Vec::new();
    let mut add_row = |tr: &Handle| {
        let cells: Vec<Handle> = tr
            .children
            .borrow()
            .iter()
            .filter(|c| matches!(html_name(c), Some("td" | "th")))
            .cloned()
            .collect();
        rows.push(cells);
    };
    for child in table.children.borrow().iter() {
        match html_name(child) {
            Some("tr") => add_row(child),
            // A footer is left out.
            Some("thead" | "tbody") => {
                for tr in child.children.borrow().iter() {
                    if html_name(tr) == Some("tr") {
                        add_row(tr);
                    }
                }
            }
            _ => {}
        }
    }
    rows
}

fn span_attr(node: &Handle, name: &str, zero: usize) -> usize {
    let NodeData::Element { attrs, .. } = &node.data else {
        return 1;
    };
    let attrs = attrs.borrow();
    let value = attrs
        .iter()
        .find(|a| a.name.ns.is_empty() && &*a.name.local == name)
        .and_then(|a| a.value.trim().parse::<usize>().ok());
    match value {
        Some(0) => zero,
        Some(n) => n.min(1000),
        None => 1,
    }
}

/// Give each cell its row and column, skipping places taken by cells from
/// rows above that span down. `colspan="0"` spans what the cells after it
/// in the row leave of the table's columns.
fn place(rows: &[Vec<Handle>]) -> Vec<Cell> {
    let ncols = place_with(rows, None)
        .iter()
        .map(|c| c.col + c.colspan)
        .max()
        .unwrap_or(0);
    place_with(rows, Some(ncols))
}

fn place_with(rows: &[Vec<Handle>], ncols: Option<usize>) -> Vec<Cell> {
    let mut cells = Vec::new();
    // For each column, the first row it is free again.
    let mut busy: Vec<usize> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let mut col = 0;
        let spans: Vec<usize> = row.iter().map(|n| span_attr(n, "colspan", 0)).collect();
        for (i, node) in row.iter().enumerate() {
            while busy.get(col).is_some_and(|&until| until > r) {
                col += 1;
            }
            let colspan = match (spans[i], ncols) {
                (0, Some(n)) => {
                    let after: usize = spans[i + 1..].iter().map(|&s| s.max(1)).sum();
                    n.saturating_sub(col + after).max(1)
                }
                (s, _) => s.max(1),
            };
            let rowspan = span_attr(node, "rowspan", 1).max(1);
            if busy.len() < col + colspan {
                busy.resize(col + colspan, 0);
            }
            for b in &mut busy[col..col + colspan] {
                *b = r + rowspan;
            }
            cells.push(Cell {
                node: node.clone(),
                row: r,
                col,
                rowspan,
                colspan,
                est: 0,
                min: 0,
                blank: false,
            });
            col += colspan;
        }
    }
    cells
}

/// How wide the content would be written on one line.
fn estimate(node: &Handle, decorate: bool, sizes: &Sizes) -> usize {
    let mut total = 0;
    let mut stack = vec![node.clone()];
    let mut first = true;
    while let Some(node) = stack.pop() {
        let is_root = std::mem::replace(&mut first, false);
        match &node.data {
            NodeData::Text { contents } => total += text_estimate(&contents.borrow()),
            NodeData::Element { name, attrs, .. } => {
                if name.ns != html5ever::ns!(html) {
                    if name.ns != html5ever::ns!(svg) {
                        stack.extend(node.children.borrow().iter().rev().cloned());
                    } else if &*name.local == "svg" {
                        if let Some(title) = svg_title(&node) {
                            total += text_estimate(&title);
                        }
                    }
                    continue;
                }
                let get = |n: &str| {
                    attrs
                        .borrow()
                        .iter()
                        .find(|a| a.name.ns.is_empty() && &*a.name.local == n)
                        .map(|a| a.value.to_string())
                };
                let children = node.children.borrow();
                let local = &*name.local;
                if is_root && matches!(local, "td" | "th") {
                    stack.extend(children.iter().rev().cloned());
                    continue;
                }
                match local {
                    "script" | "style" | "template" => continue,
                    "br" => {
                        total += 1;
                        continue;
                    }
                    "img" => {
                        if get("src").is_some_and(|s| !s.is_empty()) {
                            total += 2 + get("alt").map_or(0, |a| text_estimate(&a));
                        }
                        continue;
                    }
                    "b" | "strong" if decorate => total += 4,
                    "em" | "code" | "dt" if decorate => total += 2,
                    "a" if get("href").is_some() => {
                        if !link_has_content(&node, decorate) {
                            continue;
                        }
                        total += 5;
                    }
                    "ul" | "ol" | "blockquote" if !children.iter().any(survives) => continue,
                    "ul" => total += 2,
                    "ol" => {
                        let items: Vec<&Handle> = children
                            .iter()
                            .filter(|c| html_name(c) == Some("li"))
                            .collect();
                        let start = get("start")
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or(1);
                        let last = start.wrapping_add(items.len() as i64 - 1);
                        let w = format!("{start}.").len().max(format!("{last}.").len());
                        total += w + 1;
                        stack.extend(items.into_iter().rev().cloned());
                        continue;
                    }
                    "dl" => {
                        stack.extend(
                            children
                                .iter()
                                .rev()
                                .filter(|c| matches!(html_name(c), Some("dt" | "dd")))
                                .cloned(),
                        );
                        continue;
                    }
                    "blockquote" | "dd" => total += 2,
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        total += usize::from(local.as_bytes()[1] - b'0') + 1
                    }
                    "table" => {
                        total += sizes.get(&Rc::as_ptr(&node)).map_or(0, |s| s.0);
                        continue;
                    }
                    _ => {}
                }
                stack.extend(children.iter().rev().cloned());
            }
            _ => {}
        }
    }
    total
}

/// How narrow the content of `node` can be written: its widest part (a
/// text up to three characters, a marker, a link with its markers), plus
/// the prefixes of the blocks around it.
fn min_estimate(node: &Handle, decorate: bool, sizes: &Sizes) -> usize {
    // Worked out after the children (with an explicit stack, as documents
    // can nest deeply): `add` plus the widest child, or `floor` if more.
    struct Frame {
        children: Vec<Handle>,
        next: usize,
        widest: usize,
        add: usize,
        floor: usize,
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut node = node.clone();
    loop {
        let mut value = match min_plan(&node, decorate, sizes) {
            Plan::Value(v) => Some(v),
            Plan::Children {
                children,
                add,
                floor,
            } => {
                stack.push(Frame {
                    children,
                    next: 0,
                    widest: 0,
                    add,
                    floor,
                });
                None
            }
        };
        loop {
            if let Some(v) = value {
                match stack.last_mut() {
                    Some(top) => top.widest = top.widest.max(v),
                    None => return v,
                }
            }
            let top = stack.last_mut().unwrap();
            if let Some(child) = top.children.get(top.next) {
                top.next += 1;
                node = child.clone();
                break;
            }
            let top = stack.pop().unwrap();
            value = Some(top.add + top.widest.max(top.floor));
        }
    }
}

enum Plan {
    Value(usize),
    /// `add` plus the widest of `children`, or `floor` if more.
    Children {
        children: Vec<Handle>,
        add: usize,
        floor: usize,
    },
}

fn min_plan(node: &Handle, decorate: bool, sizes: &Sizes) -> Plan {
    let all = || node.children.borrow().clone();
    let max = |add: usize, floor: usize| Plan::Children {
        children: all(),
        add,
        floor,
    };
    match &node.data {
        NodeData::Text { contents } => Plan::Value(text_estimate(&contents.borrow()).min(MIN_COL)),
        NodeData::Element { name, attrs, .. } => {
            if name.ns != html5ever::ns!(html) {
                return if name.ns == html5ever::ns!(svg) {
                    Plan::Value(0)
                } else {
                    max(0, 0)
                };
            }
            let get = |n: &str| {
                attrs
                    .borrow()
                    .iter()
                    .find(|a| a.name.ns.is_empty() && &*a.name.local == n)
                    .map(|a| a.value.to_string())
            };
            let local = &*name.local;
            let survive = || node.children.borrow().iter().any(survives);
            match local {
                "script" | "style" | "template" => Plan::Value(0),
                "br" => Plan::Value(1),
                "img" => Plan::Value(if get("src").is_some_and(|s| !s.is_empty()) {
                    2 + usize::from(get("alt").is_some_and(|a| !a.chars().all(collapses)))
                } else {
                    0
                }),
                "b" | "strong" if decorate => max(0, 2),
                "em" | "code" | "dt" if decorate => max(0, 1),
                "a" if get("href").is_some() => {
                    if link_has_content(node, decorate) {
                        max(0, 5)
                    } else {
                        Plan::Value(0)
                    }
                }
                "ul" | "ol" | "blockquote" if !survive() => Plan::Value(0),
                "ul" | "blockquote" | "dd" => max(2, 0),
                "ol" => {
                    let items: Vec<Handle> = node
                        .children
                        .borrow()
                        .iter()
                        .filter(|c| html_name(c) == Some("li"))
                        .cloned()
                        .collect();
                    let start = get("start")
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(1);
                    let last = start.wrapping_add(items.len() as i64 - 1);
                    let w = format!("{start}.").len().max(format!("{last}.").len());
                    Plan::Children {
                        children: items,
                        add: w + 1,
                        floor: 0,
                    }
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    max(usize::from(local.as_bytes()[1] - b'0') + 1, 0)
                }
                "dl" => Plan::Children {
                    children: node
                        .children
                        .borrow()
                        .iter()
                        .filter(|c| matches!(html_name(c), Some("dt" | "dd")))
                        .cloned()
                        .collect(),
                    add: 0,
                    floor: 0,
                },
                "table" => Plan::Value(sizes.get(&Rc::as_ptr(node)).map_or(0, |s| s.1)),
                _ => max(0, 0),
            }
        }
        _ => Plan::Value(0),
    }
}

/// Whether a cell has a superscript, whose markers are written even round
/// nothing (though they don't count towards its width).
fn has_sup(cell: &Handle) -> bool {
    let mut stack = vec![cell.clone()];
    while let Some(node) = stack.pop() {
        match html_name(&node) {
            Some("sup") => return true,
            Some("script" | "style" | "template") => {}
            _ => stack.extend(node.children.borrow().iter().cloned()),
        }
    }
    false
}

/// The estimate and minimum width of tables, by node.
pub type Sizes = HashMap<*const Node, (usize, usize)>;

/// Size `table` and the tables inside it, innermost first, so that sizing
/// a cell never has to look inside a table in it.
fn size_tables(table: &Handle, decorate: bool, sizes: &mut Sizes) {
    let mut tables = Vec::new();
    let mut stack = vec![table.clone()];
    while let Some(node) = stack.pop() {
        if html_name(&node) == Some("table") && !sizes.contains_key(&Rc::as_ptr(&node)) {
            tables.push(node.clone());
        }
        stack.extend(node.children.borrow().iter().cloned());
    }
    for t in tables.iter().rev() {
        let rows = collect_rows(t);
        // All the cells, and the separators of the longest row.
        let cells: usize = rows
            .iter()
            .flatten()
            .map(|c| estimate(c, decorate, sizes))
            .sum();
        let seps = rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .saturating_sub(1);
        // The narrowest row, its cells at their narrowest.
        let min = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| min_estimate(c, decorate, sizes))
                    .sum::<usize>()
                    + row.len().saturating_sub(1)
            })
            .max()
            .unwrap_or(0);
        sizes.insert(Rc::as_ptr(t), (cells + seps, min));
    }
}

/// Words with a space before each one that had whitespace before it.
fn text_estimate(t: &str) -> usize {
    if !t.chars().any(|c| !collapses(c)) {
        return 0;
    }
    // A text of no width at all counts nothing.
    let width_of = |word: &str| -> usize {
        word.chars()
            .filter(|c| !c.is_control())
            .map(char_width)
            .sum()
    };
    if width_of(t) == 0 {
        return 0;
    }
    let mut total = 0;
    for (i, word) in t.split(collapses).enumerate() {
        if !word.is_empty() {
            total += width_of(word) + usize::from(i > 0);
        }
    }
    total
}

fn collapses(c: char) -> bool {
    c.is_whitespace() && c != '\u{a0}'
}

/// Column widths for the space available. Natural widths that add up to
/// more than the width are scaled down to shares of it; each column is
/// raised to its minimum, and the total then trimmed a character at a time
/// from the column with the most room above its minimum (and, if that isn't
/// enough, columns below their minimum topped up from the widest).
fn allocate(m: &[usize], mins: &[usize], width: usize) -> Vec<usize> {
    let n = m.len();
    let total: usize = m.iter().sum();
    let avail = width.saturating_sub(n.saturating_sub(1));
    if total + n.saturating_sub(1) <= width {
        return m.to_vec();
    }
    // Shares of the width, unless the columns would be widened by them.
    let mut r: Vec<usize> = if total <= width {
        m.to_vec()
    } else {
        m.iter().map(|&x| width * x / total).collect()
    };
    for (w, &min) in r.iter_mut().zip(mins) {
        *w = (*w).max(min);
    }
    // Most room above the minimum, then the widest, then the first.
    let roomiest = |r: &[usize]| {
        let key = |i: usize| (r[i] as isize - mins[i] as isize, r[i]);
        let mut j = 0;
        for i in 1..r.len() {
            if key(i) > key(j) {
                j = i;
            }
        }
        j
    };
    while r.iter().sum::<usize>() > avail {
        let j = roomiest(&r);
        if r[j] == 0 {
            break;
        }
        r[j] -= 1;
    }
    let widest = |r: &[usize]| {
        let mut j = 0;
        for i in 1..r.len() {
            if r[i] > r[j] {
                j = i;
            }
        }
        j
    };
    for i in 0..n {
        while r[i] < mins[i] {
            let j = widest(&r);
            if j == i || r[j] == 0 {
                break;
            }
            r[i] += 1;
            r[j] -= 1;
        }
    }
    r
}

fn layout(r: &mut Renderer, cells: &mut [Cell], nrows_all: usize, width: usize) -> Job {
    // Columns are only divided where a cell starts or ends; a cell spanning
    // past the others spans fewer columns.
    let all_cols = cells.iter().map(|c| c.col + c.colspan).max().unwrap_or(0);
    let mut exists = vec![false; all_cols];
    for c in cells.iter() {
        exists[c.col] = true;
        if c.col + c.colspan < all_cols {
            exists[c.col + c.colspan] = true;
        }
    }
    let mut index = vec![0usize; all_cols + 1];
    for j in 0..all_cols {
        index[j + 1] = index[j] + usize::from(exists[j]);
    }
    for c in cells.iter_mut() {
        let first = index[c.col];
        let end = index[c.col + c.colspan];
        c.col = first;
        c.colspan = (end - first).max(1);
    }
    let ncols = index[all_cols];
    // Written one under another, every row and cell is kept, empty or not.
    let every: Vec<Cell> = cells
        .iter()
        .map(|c| Cell {
            node: c.node.clone(),
            ..*c
        })
        .collect();
    // Rows with nothing in any of their cells are left out (once they have
    // counted for the columns), unless a cell from a row above spans into
    // them. A left-out row's cells go with it.
    let mut shown = vec![false; nrows_all];
    for c in cells.iter() {
        if c.est > 0 || has_sup(&c.node) {
            shown[c.row] = true;
        }
        for s in &mut shown[(c.row + 1).min(nrows_all)..(c.row + c.rowspan).min(nrows_all)] {
            *s = true;
        }
    }
    let mut row_index = vec![0usize; nrows_all + 1];
    for r in 0..nrows_all {
        row_index[r + 1] = row_index[r] + usize::from(shown[r]);
    }
    // Where a left-out row's cell spans down, the rows below have an empty
    // cell of their own.
    let mut gaps: Vec<Cell> = cells
        .iter()
        .filter(|c| !shown[c.row])
        .flat_map(|c| {
            (c.row + 1..(c.row + c.rowspan).min(nrows_all)).map(move |row| Cell {
                node: c.node.clone(),
                row,
                col: c.col,
                rowspan: 1,
                colspan: c.colspan,
                est: 0,
                min: 0,
                blank: true,
            })
        })
        .collect();
    let mut cells: Vec<&mut Cell> = cells
        .iter_mut()
        .filter(|c| shown[c.row])
        .chain(gaps.iter_mut())
        .collect();
    for c in cells.iter_mut() {
        let end = (c.row + c.rowspan).min(nrows_all);
        // Spanning past the last row stays past it.
        let past = c.row + c.rowspan - end;
        let first = row_index[c.row];
        c.rowspan = (row_index[end] - first + past).max(1);
        c.row = first;
    }
    let nrows = row_index[nrows_all];
    // Natural widths: single cells first, then spanning cells widen the
    // columns they cover.
    let mut m = vec![0usize; ncols];
    for c in cells.iter().filter(|c| c.colspan == 1) {
        m[c.col] = m[c.col].max(c.est);
    }
    // The narrowest each column can be: what its cells can't do without,
    // a spanning cell's shared out like its width.
    let mut mins = vec![0usize; ncols];
    for c in cells.iter().filter(|c| c.colspan == 1) {
        mins[c.col] = mins[c.col].max(c.min);
    }
    for c in cells.iter().filter(|c| c.colspan > 1) {
        let n = c.colspan;
        let each = c.est.saturating_sub(n - 1).div_ceil(n);
        for w in &mut m[c.col..c.col + n] {
            *w = (*w).max(each);
        }
        let each_min = c.min.saturating_sub(n - 1).div_ceil(n);
        for w in &mut mins[c.col..c.col + n] {
            *w = (*w).max(each_min);
        }
    }
    if m.iter().all(|&w| w == 0) {
        return Job::done(Vec::new());
    }
    // A column only spanned into still needs a character.
    for j in 0..ncols {
        if mins[j] == 0 && m[j] > 0 && !cells.iter().any(|c| c.colspan == 1 && c.col == j) {
            mins[j] = 1;
        }
    }
    // Raw mode writes every table one cell under another, without borders.
    let raw = r.opts().raw;
    let borders = r.opts().table_borders && !raw;
    let narrow = raw || mins.iter().sum::<usize>() + ncols - 1 > width;
    // Empty columns are sized with the others (to nothing), and kept for
    // the cells spanning them.
    let widths = if narrow {
        m.clone()
    } else {
        allocate(&m, &mins, width)
    };
    let kept: Vec<(&Cell, usize, usize)> = cells
        .iter()
        .filter(|c| c.colspan > 1 || m[c.col] > 0)
        .map(|c| (&**c, c.col, c.colspan))
        .collect();
    if narrow {
        return vertical(&every, nrows_all, width, borders);
    }
    let mut grid = Grid::new(&kept, nrows, ncols, &widths);
    Job {
        cells: std::mem::take(&mut grid.pending),
        lines: Vec::new(),
        kind: JobKind::Grid { grid, borders },
    }
}

/// Each cell under the one before, at the full width. A cell spanning down
/// from a row above has an empty place in the rows below.
fn vertical(cells: &[Cell], nrows: usize, width: usize, borders: bool) -> Job {
    let mut pieces = Vec::new();
    let mut to_render = Vec::new();
    let rule = || Line {
        text: "─".repeat(width),
        width,
        kind: LineKind::Rule,
    };
    let sep = || Line::new("/".repeat(width), width);
    if borders {
        pieces.push(Some(rule()));
    }
    for row in 0..nrows {
        let mut in_row: Vec<&Cell> = cells
            .iter()
            .filter(|c| c.row <= row && row < c.row + c.rowspan)
            .collect();
        in_row.sort_by_key(|c| c.col);
        for (i, c) in in_row.iter().enumerate() {
            if i > 0 && borders {
                pieces.push(Some(sep()));
            }
            if c.row == row {
                pieces.push(None);
                to_render.push(Some((c.node.clone(), width + c.colspan - 1)));
            }
        }
        if borders {
            pieces.push(Some(rule()));
        }
    }
    Job {
        cells: to_render,
        lines: Vec::new(),
        kind: JobKind::Vertical(pieces),
    }
}

struct Placed {
    row: usize,
    col: usize,
    rowspan: usize,
    colspan: usize,
    lines: Vec<Line>,
    width: usize,
    /// A nested table's top and bottom borders, drawn as part of this
    /// table's borders above and below the cell.
    top: Option<Line>,
    bottom: Option<Line>,
}

struct Grid {
    cells: Vec<Placed>,
    widths: Vec<usize>,
    nrows: usize,
    heights: Vec<usize>,
    pending: Vec<Option<(Handle, usize)>>,
    /// For each row, its cells as drawn: (cell, x, width).
    vis: Vec<Vec<(usize, usize, usize)>>,
}

impl Grid {
    fn new(cells: &[(&Cell, usize, usize)], nrows: usize, ncols: usize, widths: &[usize]) -> Grid {
        let mut owner = vec![vec![None; ncols]; nrows];
        let mut placed = Vec::new();
        let mut pending = Vec::new();
        for (c, col, span) in cells {
            let w = widths[*col..*col + span].iter().sum::<usize>() + span - 1;
            let idx = placed.len();
            for row in owner.iter_mut().skip(c.row).take(c.rowspan) {
                for o in &mut row[*col..*col + span] {
                    if o.is_none() {
                        *o = Some(idx);
                    }
                }
            }
            placed.push(Placed {
                row: c.row,
                col: *col,
                rowspan: c.rowspan,
                colspan: *span,
                lines: Vec::new(),
                width: w,
                top: None,
                bottom: None,
            });
            pending.push((!c.blank).then(|| (c.node.clone(), w)));
        }
        // Each row's cells as drawn: columns of no width are left out (with
        // their separators) unless a cell spans them.
        let mut vis = Vec::with_capacity(nrows);
        for (r, row) in owner.iter().enumerate() {
            let mut cells_in_row = Vec::new();
            let mut x = 0;
            let mut col = 0;
            while col < ncols {
                match row[col] {
                    Some(idx) => {
                        let c: &Placed = &placed[idx];
                        if c.colspan == 1 && widths[col] == 0 {
                            col += 1;
                            continue;
                        }
                        if !cells_in_row.is_empty() {
                            x += 1;
                        }
                        cells_in_row.push((idx, x, c.width));
                        x += c.width;
                        col = c.col + c.colspan;
                    }
                    None => {
                        // Nothing more in this row, unless only an empty
                        // column is missing. A gap before a cell spanning
                        // down from above is an empty cell.
                        if widths[col] == 0 {
                            col += 1;
                            continue;
                        }
                        if row[col..].iter().all(Option::is_none) {
                            break;
                        }
                        if !cells_in_row.is_empty() {
                            x += 1;
                        }
                        cells_in_row.push((placed.len(), x, widths[col]));
                        placed.push(Placed {
                            row: r,
                            col,
                            rowspan: 1,
                            colspan: 1,
                            lines: Vec::new(),
                            width: widths[col],
                            top: None,
                            bottom: None,
                        });
                        x += widths[col];
                        col += 1;
                    }
                }
            }
            vis.push(cells_in_row);
        }
        Grid {
            cells: placed,
            widths: widths.to_vec(),
            nrows,
            heights: Vec::new(),
            pending,
            vis,
        }
    }

    fn draw(mut self, rendered: Vec<Vec<Line>>, borders: bool) -> Vec<Line> {
        for (i, mut lines) in rendered.into_iter().enumerate() {
            if borders {
                let cell = &mut self.cells[i];
                if lines.first().is_some_and(|l| l.kind == LineKind::Rule) {
                    cell.top = Some(lines.remove(0));
                }
                if lines.last().is_some_and(|l| l.kind == LineKind::Rule) {
                    cell.bottom = lines.pop();
                }
            }
            self.cells[i].lines = lines;
        }
        self.row_heights();
        // Rows with nothing in them are left out, borders and all, unless a
        // cell from a row above spans into them.
        let mut keep: Vec<bool> = self.heights.iter().map(|&h| h > 0).collect();
        for c in &self.cells {
            if !c.lines.is_empty() {
                keep[c.row] = true;
            }
            for k in keep.iter_mut().take(c.row + c.rowspan).skip(c.row + 1) {
                *k = true;
            }
        }
        let drawn: Vec<usize> = (0..self.nrows).filter(|&r| keep[r]).collect();
        let mut out = Vec::new();
        let sep = if borders { "│" } else { " " };
        if drawn.is_empty() {
            if borders {
                out.push(self.border(None, (self.nrows > 0).then_some(0)));
            }
            return out;
        }
        let mut prev = None;
        for &row in &drawn {
            if borders {
                out.push(self.border(prev, Some(row)));
            }
            for line in 0..self.heights[row] {
                // Lines of a cell spanning rows are counted as if there were
                // borders between the rows, shown or not.
                out.push(self.row_line(row, line, sep));
            }
            prev = Some(row);
        }
        if borders {
            let bottom = self.border(prev, None);
            if !bottom.text.is_empty() {
                out.push(bottom);
            }
        }
        out
    }

    fn row_heights(&mut self) {
        let mut h = vec![0usize; self.nrows];
        for c in self.cells.iter().filter(|c| c.rowspan == 1) {
            h[c.row] = h[c.row].max(c.lines.len());
        }
        // A cell spanning rows shares its lines out between them, a row at a
        // time, counting the borders between them and the rows it spans past
        // the end of the table.
        for c in self.cells.iter().filter(|c| c.rowspan > 1) {
            let mut left = c.lines.len();
            for (k, hr) in h[c.row..].iter_mut().take(c.rowspan).enumerate() {
                let rows = c.rowspan - k;
                *hr = (*hr).max(left.saturating_sub(rows - 1).div_ceil(rows));
                left = left.saturating_sub(*hr + 1);
            }
        }
        self.heights = h;
    }

    /// Where line `line` of `row` falls within a cell starting above it.
    fn offset(&self, idx: usize, row: usize, line: usize) -> usize {
        let c = &self.cells[idx];
        let mut offset = line;
        for r in c.row..row.min(self.nrows) {
            offset += self.heights[r] + 1;
        }
        offset
    }

    fn push_cell_line(&self, idx: usize, offset: usize, out: &mut Line) {
        let c = &self.cells[idx];
        match c.lines.get(offset) {
            Some(l) => push_padded(out, l, c.width),
            None => match c.lines.last().filter(|l| l.kind == LineKind::Row) {
                // Below a nested table, its columns carry on down.
                Some(last) if c.bottom.is_some() => {
                    // Each run of text becomes spaces as wide as it is.
                    let mut frame = String::new();
                    let mut run = 0;
                    for (i, ch) in last.text.char_indices() {
                        if is_box(ch) {
                            let w = last.text[run..i].width();
                            frame.extend(std::iter::repeat_n(' ', w));
                            frame.push(ch);
                            run = i + ch.len_utf8();
                        }
                    }
                    frame.extend(std::iter::repeat_n(' ', last.text[run..].width()));
                    let w = frame.width();
                    push_padded(out, &Line::new(frame, w), c.width);
                }
                _ => {
                    out.text.extend(std::iter::repeat_n(' ', c.width));
                    out.width += c.width;
                }
            },
        }
    }

    fn row_line(&self, row: usize, line: usize, sep: &str) -> Line {
        let mut out = Line {
            text: String::new(),
            width: 0,
            kind: if sep == " " {
                LineKind::Text
            } else {
                LineKind::Row
            },
        };
        for (i, &(idx, _, _)) in self.vis[row].iter().enumerate() {
            if i > 0 {
                out.text.push_str(sep);
                out.width += 1;
            }
            self.push_cell_line(idx, self.offset(idx, row, line), &mut out);
        }
        out
    }

    /// The whole width of the table, its empty columns left out.
    fn full_width(&self) -> usize {
        let shown: Vec<usize> = self.widths.iter().copied().filter(|&w| w > 0).collect();
        shown.iter().sum::<usize>() + shown.len().saturating_sub(1)
    }

    /// The border between rows `up` and `down` (the top of the table when
    /// there is no `up`, its bottom when there is no `down`).
    fn border(&self, up: Option<usize>, down: Option<usize>) -> Line {
        let none = Vec::new();
        let above = up.map_or(&none, |r| &self.vis[r]);
        let below = down.map_or(&none, |r| &self.vis[r]);
        // Where the border is, counted in rows.
        let row = down.unwrap_or(self.nrows);
        let extent = |v: &Vec<(usize, usize, usize)>| v.last().map_or(0, |&(_, x, w)| x + w);
        let end = if up.is_none() {
            self.full_width().max(extent(below))
        } else {
            extent(above).max(extent(below))
        };
        // A cell spanning rows across this border, at the bottom one that
        // spans past the last row.
        let through = |&(idx, _, _): &(usize, usize, usize)| -> bool {
            let c = &self.cells[idx];
            if down.is_none() {
                c.row + c.rowspan > self.nrows
            } else {
                c.row < row && c.row + c.rowspan > row
            }
        };
        // The box-drawing directions at each column of the line, under the
        // text of any cell spanning across it.
        let mut slots = vec![0u8; end];
        let mut horizontal = vec![true; end];
        let mut texts: Vec<(usize, Line)> = Vec::new();
        for v in above.iter().filter(|v| through(v)) {
            let (idx, x, w) = *v;
            let mut line = Line::new(String::new(), 0);
            let offset = self.offset(idx, row, 0) - 1;
            self.push_cell_line(idx, offset, &mut line);
            for h in &mut horizontal[x..(x + w).min(end)] {
                *h = false;
            }
            texts.push((x, line));
        }
        for (d, &h) in slots.iter_mut().zip(&horizontal) {
            if h {
                *d |= LEFT | RIGHT;
            }
        }
        // Separators above and below.
        for (list, dir) in [(above, UP), (below, DOWN)] {
            for &(_, x, _) in list.iter().skip(1) {
                if let Some(d) = slots.get_mut(x - 1) {
                    *d |= dir;
                }
            }
        }
        // Where a separator meets a cell spanning across, the line on that
        // side stops (on the left, only for a separator from above).
        for x in 0..end {
            let d = &mut slots[x];
            if *d & (UP | DOWN) != 0 {
                if x == 0 || (!horizontal[x - 1] && *d & UP != 0) {
                    *d &= !LEFT;
                }
                if x + 1 >= end || !horizontal[x + 1] {
                    *d &= !RIGHT;
                }
            }
        }
        // Nested tables' borders falling on this one.
        for (list, is_below) in [(above, false), (below, true)] {
            for &(idx, x, w) in list {
                let c = &self.cells[idx];
                let rule = if is_below {
                    (c.row == row).then_some(c.top.as_ref()).flatten()
                } else {
                    (Some(c.row + c.rowspan - 1) == up && !through(&(idx, x, w)))
                        .then_some(c.bottom.as_ref())
                        .flatten()
                };
                if let Some(line) = rule {
                    let mut padded = Line::new(String::new(), 0);
                    padded.kind = LineKind::Rule;
                    push_padded(&mut padded, line, w);
                    for (k, ch) in padded.text.chars().enumerate() {
                        if let Some(d) = slots.get_mut(x + k) {
                            *d |= char_dirs(ch);
                        }
                    }
                }
            }
        }
        // Lay the spanning cells' text over the line.
        let mut out = Line {
            text: String::new(),
            width: 0,
            kind: LineKind::Rule,
        };
        texts.sort_by_key(|t| t.0);
        let mut ti = texts.into_iter().peekable();
        let mut x = 0;
        while x < end {
            if let Some((tx, _)) = ti.peek() {
                if *tx == x {
                    let (_, line) = ti.next().unwrap();
                    out.text.push_str(&line.text);
                    out.width += line.width;
                    x += line.width.max(1);
                    continue;
                }
            }
            out.text.push(dirs_char(slots[x]));
            out.width += 1;
            x += 1;
        }
        out
    }
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

fn char_dirs(c: char) -> u8 {
    match c {
        '─' => LEFT | RIGHT,
        '│' => UP | DOWN,
        '┬' => LEFT | RIGHT | DOWN,
        '┴' => LEFT | RIGHT | UP,
        '┼' => LEFT | RIGHT | UP | DOWN,
        '├' => UP | DOWN | RIGHT,
        '┤' => UP | DOWN | LEFT,
        '└' => UP | RIGHT,
        '┘' => UP | LEFT,
        '┌' => DOWN | RIGHT,
        '┐' => DOWN | LEFT,
        _ => 0,
    }
}

fn dirs_char(d: u8) -> char {
    junction(d & UP != 0, d & DOWN != 0, d & LEFT != 0, d & RIGHT != 0)
}

/// `line` padded to `width`: a border with more border, text with spaces.
fn push_padded(out: &mut Line, line: &Line, width: usize) {
    out.text.push_str(&line.text);
    let pad = width.saturating_sub(line.width);
    let fill = if line.kind == LineKind::Rule {
        '─'
    } else {
        ' '
    };
    out.text.extend(std::iter::repeat_n(fill, pad));
    out.width += line.width + pad;
}

fn is_box(c: char) -> bool {
    matches!(
        c,
        '─' | '│' | '┬' | '┴' | '┼' | '├' | '┤' | '└' | '┘' | '┌' | '┐'
    )
}

fn junction(up: bool, down: bool, left: bool, right: bool) -> char {
    match (up, down, left, right) {
        (false, false, false, false) => ' ',
        (false, false, _, _) => '─',
        (true, true, true, true) => '┼',
        (false, true, true, true) => '┬',
        (true, false, true, true) => '┴',
        (true, true, false, true) => '├',
        (true, true, true, false) => '┤',
        (true, true, false, false) => '│',
        (true, false, false, true) => '└',
        (true, false, true, false) => '┘',
        (false, true, false, true) => '┌',
        (false, true, true, false) => '┐',
        (true, false, false, false) => '│',
        (false, true, false, false) => '│',
    }
}
