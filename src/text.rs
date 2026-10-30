//! HTML to plain text.
//!
//! The document is walked once, with an explicit stack, writing into a stack
//! of line buffers: one for the document and one for each block whose lines
//! get a prefix (quotes, headings, list items, definitions). Blocks ask for a
//! line break or a blank line before and after them; those requests are kept
//! pending and only carried out when something is written after them, so
//! empty blocks leave no trace and adjacent requests merge.

use std::collections::HashMap;
use std::mem;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::dom::{Dom, Handle, NodeData};
use crate::table;
use crate::url;

/// Rendering options, as `html2text()` takes them.
pub struct Options {
    pub width: usize,
    pub decorate: bool,
    pub link_footnotes: bool,
    pub table_borders: bool,
    pub wrap_links: bool,
    pub max_wrap_width: Option<usize>,
    pub raw: bool,
    pub base_url: Option<String>,
}

/// Prefixed blocks are never narrower than this, however deeply nested.
const MIN_WIDTH: usize = 3;

const STRIKE: char = '\u{336}';
const ZWSP: char = '\u{200b}';

/// Separator levels: nothing, a line break, a blank line.
const NONE: u8 = 0;
const LINE: u8 = 1;
const BLANK: u8 = 2;

pub fn render(dom: &Dom, opts: &Options) -> Result<String, String> {
    if opts.width == 0 {
        return Err("Output width not wide enough.".into());
    }
    // Each table's cells are rendered on their own, and may hold tables. So
    // that tables nested any depth don't need as deep a call stack, a cell
    // is a level here: the walk of the level below stops at a table, its
    // cells are rendered on levels above it, and then it carries on.
    struct Level<'a> {
        r: Renderer<'a>,
        steps: Vec<Step>,
        table: Option<table::Job>,
    }
    let mut levels = vec![Level {
        r: Renderer::new(opts, opts.width),
        steps: children(&dom.document),
        table: None,
    }];
    loop {
        let level = levels.last_mut().unwrap();
        if let Some(job) = &mut level.table {
            if let Some((cell, width)) = job.next_cell() {
                let mut r = Renderer::new(opts, width);
                r.footnotes = mem::take(&mut level.r.footnotes);
                r.table_sizes = mem::take(&mut level.r.table_sizes);
                levels.push(Level {
                    r,
                    steps: children(&cell),
                    table: None,
                });
                continue;
            }
            let lines = level.table.take().unwrap().finish();
            level.r.end_table(lines);
        }
        if let Some(t) = level.r.run(&mut level.steps) {
            level.table = Some(table::start(&mut level.r, &t));
            continue;
        }
        if levels.len() == 1 {
            break;
        }
        let mut done = levels.pop().unwrap();
        let parent = levels.last_mut().unwrap();
        parent.r.footnotes = mem::take(&mut done.r.footnotes);
        parent.r.table_sizes = mem::take(&mut done.r.table_sizes);
        let lines = done.r.bufs.pop().unwrap().finish();
        parent
            .table
            .as_mut()
            .unwrap()
            .add(lines.into_iter().map(Line::flatten).collect());
    }
    Ok(levels.pop().unwrap().r.finish())
}

pub fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Whitespace that collapses. No-break space is kept as it is.
fn collapses(c: char) -> bool {
    c.is_whitespace() && c != '\u{a0}'
}

/// Has something other than whitespace (no-break space counting as such).
pub fn visible(s: &str) -> bool {
    s.chars().any(|c| !c.is_whitespace())
}

/// A line of output, and its width as measured while writing it.
pub struct Line {
    pub text: String,
    pub width: usize,
    pub kind: LineKind,
    /// The prefixes of the blocks the line is in, still to be put in front
    /// of `text`, written back to front: each block then adds its own in
    /// time for its length, however deeply blocks are nested.
    rprefix: String,
}

/// What a line is, for a table it ends up in.
#[derive(Clone, Copy, PartialEq)]
pub enum LineKind {
    Text,
    /// A table's border.
    Rule,
    /// A row of a table with borders.
    Row,
}

impl Line {
    pub fn new(text: String, width: usize) -> Line {
        Line::of_kind(text, width, LineKind::Text)
    }

    pub fn of_kind(text: String, width: usize, kind: LineKind) -> Line {
        Line {
            text,
            width,
            kind,
            rprefix: String::new(),
        }
    }

    fn empty() -> Line {
        Line::new(String::new(), 0)
    }

    fn add_prefix(&mut self, prefix: &str) {
        self.rprefix.extend(prefix.chars().rev());
        self.width += prefix.len();
        // A prefix makes a table's lines plain text.
        self.kind = LineKind::Text;
    }

    /// The line with its prefixes in front of it.
    fn flatten(mut self) -> Line {
        if !self.rprefix.is_empty() {
            let mut text = String::with_capacity(self.rprefix.len() + self.text.len());
            text.extend(self.rprefix.chars().rev());
            text.push_str(&self.text);
            self.text = text;
            self.rprefix.clear();
        }
        self
    }
}

/// One buffer of output lines.
struct Buf {
    lines: Vec<Line>,
    /// The current line, and its display width.
    cur: String,
    cur_w: usize,
    /// The word being collected, not yet placed on a line, and where each
    /// piece of it (a text node, a marker) after the first starts.
    word: String,
    word_w: usize,
    pieces: Vec<usize>,
    word_style: u32,
    /// The word has a character of non-zero width.
    word_base: bool,
    /// A space is due before the next word.
    space: bool,
    /// The current line has been started.
    open: bool,
    /// The current line was started by wrapping.
    wrapped: bool,
    /// The last line is a blank line written as a separator.
    blank_after: bool,
    /// Something visible has been written.
    content: bool,
    /// Pending separators: from a block's start, from a block's end.
    ps: u8,
    pe: u8,
    /// Nothing has happened in this buffer yet, so its parent hasn't yet
    /// carried out the separator before the block it belongs to.
    fresh: bool,
    /// Lines are wrapped to `wrap`; nested blocks are sized from `width`.
    width: usize,
    wrap: usize,
    /// Inside `<pre>` (in this buffer: quotes and the like inside a `<pre>`
    /// are back to collapsing whitespace).
    pre: usize,
    /// Inside `<del>` or `<s>`, in this buffer.
    strike: usize,
}

impl Buf {
    fn new(width: usize, max_wrap: Option<usize>, fresh: bool) -> Buf {
        Buf {
            lines: Vec::new(),
            cur: String::new(),
            cur_w: 0,
            word: String::new(),
            word_w: 0,
            pieces: Vec::new(),
            word_style: 0,
            word_base: false,
            space: false,
            open: false,
            wrapped: false,
            blank_after: false,
            content: false,
            ps: NONE,
            pe: NONE,
            fresh,
            width,
            wrap: max_wrap.map_or(width, |m| m.min(width)).max(1),
            pre: 0,
            strike: 0,
        }
    }

    /// Place the pending word on the current line, wrapping as needed.
    fn place_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let word = mem::take(&mut self.word);
        let word_w = mem::replace(&mut self.word_w, 0);
        let pieces = mem::take(&mut self.pieces);
        self.word_base = false;
        let space = mem::replace(&mut self.space, false) && !self.cur.is_empty();
        // A word is measured character by character when deciding whether
        // it fits, but what is already on the line as a string, so that
        // emoji sequences count once they are placed. The two can disagree
        // enough to overfill a line, and then everything fits (see `room`).
        let need = word_w + usize::from(space);
        if need <= self.room() {
            if space {
                self.cur.push(' ');
                self.cur_w += 1;
            }
            self.cur_w += pieces_width(&word, &pieces);
            self.cur.push_str(&word);
        } else if word_w <= self.wrap {
            self.wrap_line();
            self.cur_w = pieces_width(&word, &pieces);
            self.cur = word;
        } else {
            if !self.cur.is_empty() {
                self.push_cur();
            }
            // Too long for any line: each piece goes on whole if it fits,
            // and is cut where the line is full otherwise.
            let mut start = 0;
            for end in pieces.into_iter().chain([word.len()]) {
                let piece = &word[start..end];
                start = end;
                if piece.width() <= self.room() {
                    self.cur_w += piece.width();
                    self.cur.push_str(piece);
                } else {
                    self.chop(piece);
                }
            }
        }
    }

    /// Room left on the line. Once a line is overfull this wraps around to
    /// a huge number and anything fits, as it did in earlier releases.
    fn room(&self) -> usize {
        self.wrap.wrapping_sub(self.cur_w)
    }

    /// A new piece of the word starts here.
    fn piece(&mut self) {
        if !self.word.is_empty() && self.pieces.last() != Some(&self.word.len()) {
            self.pieces.push(self.word.len());
        }
    }

    /// Write `s` from the current position, breaking lines at the wrap width
    /// wherever they reach it.
    fn chop(&mut self, s: &str) {
        // Characters are measured one by one while cutting; what ends up on
        // the line is then measured as a string.
        let mut start = self.cur.len();
        let mut base = self.cur_w;
        for c in s.chars() {
            let w = char_width(c);
            if w > self.room() && self.cur_w > 0 {
                self.cur_w = base + self.cur[start..].width();
                self.wrap_line();
                start = 0;
                base = 0;
            }
            self.cur.push(c);
            self.cur_w += w;
        }
        self.cur_w = base + self.cur[start..].width();
    }

    fn push_cur(&mut self) {
        self.lines
            .push(Line::new(mem::take(&mut self.cur), self.cur_w));
        self.cur_w = 0;
        self.wrapped = false;
    }

    /// Start a new line because the current one is full.
    fn wrap_line(&mut self) {
        self.push_cur();
        self.wrapped = true;
    }

    /// End the current line, if one has been started.
    fn close(&mut self) {
        self.place_word();
        self.space = false;
        if self.open {
            self.push_cur();
            self.open = false;
        }
    }

    fn realize(&mut self, level: u8) {
        if self.content && level > NONE {
            self.close();
            if level == BLANK && !self.blank_after {
                self.lines.push(Line::empty());
                self.blank_after = true;
            }
        }
    }

    fn finish(mut self) -> Vec<Line> {
        self.close();
        self.lines
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// `<p>`: blank lines around, the line left open at the end.
    Para,
    /// `<div>`: line breaks around, the line closed at the end.
    Div,
    /// `<ul>` and `<ol>`: line breaks around.
    List,
    /// `<pre>`.
    Pre,
    /// A list item outside a list.
    LooseItem,
    /// `<dl>` and `<dt>`: a separator before but none after.
    Lead(u8),
    /// A prefixed block in its own buffer.
    Prefixed(u8),
    /// A table: a blank line before, a line break after.
    Table,
}

impl Kind {
    fn level(self) -> u8 {
        match self {
            Kind::Para | Kind::Pre | Kind::LooseItem => BLANK,
            Kind::Div | Kind::List => LINE,
            Kind::Lead(l) | Kind::Prefixed(l) => l,
            Kind::Table => BLANK,
        }
    }

    fn end_level(self) -> u8 {
        match self {
            Kind::Table => LINE,
            k => k.level(),
        }
    }
}

struct Frame {
    kind: Kind,
    wrote: bool,
    saved: (u8, u8),
}

/// What to do once an element's children are done.
enum Leave {
    Nothing,
    Text(&'static str),
    /// Leave a styled element, then write its closing marker.
    Styled(&'static str),
    Frame,
    Pre,
    Strike,
    Link(String),
    Prefixed {
        first: String,
        rest: String,
    },
}

enum Step {
    Enter(Handle),
    /// A child of `<ul>` or `<ol>` that becomes an item.
    Item {
        node: Handle,
        first: String,
        rest: String,
        li: bool,
    },
    /// A `<dt>` or `<dd>` in a `<dl>`.
    Def(Handle),
    /// Something that isn't shown but still counts.
    Touch,
    Leave(Leave),
    /// A table, whose cells are rendered on their own.
    Table(Handle),
}

pub struct Renderer<'a> {
    opts: &'a Options,
    bufs: Vec<Buf>,
    frames: Vec<Frame>,
    pub footnotes: Vec<String>,
    /// Sizes of the tables seen so far, shared with the cells' renderers.
    pub table_sizes: table::Sizes,
    /// The styled elements (emphasis, code, links...) the text is inside.
    /// Text in one style is measured as one piece. Each distinct stack of
    /// them is numbered (`styles`, 0 for none), and this holds the number of
    /// the stack up to each of them.
    style: Vec<u32>,
    styles: HashMap<(u32, u8), u32>,
    /// Width of the leading whitespace of a `<pre>` line, or of whitespace
    /// after a tab, held until something follows it.
    prews: usize,
    after_tab: bool,
}

impl<'a> Renderer<'a> {
    pub fn new(opts: &'a Options, width: usize) -> Renderer<'a> {
        Renderer {
            opts,
            bufs: vec![Buf::new(width, opts.max_wrap_width, false)],
            frames: Vec::new(),
            footnotes: Vec::new(),
            table_sizes: table::Sizes::new(),
            style: Vec::new(),
            styles: HashMap::new(),
            prews: 0,
            after_tab: false,
        }
    }

    fn b(&mut self) -> &mut Buf {
        self.bufs.last_mut().unwrap()
    }

    fn mark(&mut self) {
        // Frames are marked from the outermost in, so once one is found
        // marked, so are all those outside it.
        for f in self.frames.iter_mut().rev() {
            if f.wrote {
                break;
            }
            f.wrote = true;
        }
    }

    fn push_style(&mut self, id: u8) {
        let next = self.styles.len() as u32 + 1;
        let style = *self.styles.entry((self.cur_style(), id)).or_insert(next);
        self.style.push(style);
    }

    fn cur_style(&self) -> u32 {
        self.style.last().copied().unwrap_or(0)
    }

    /// The first thing happens in a buffer: carry out the separator before
    /// its block, in the parent (and so on up).
    fn wake(&mut self) {
        let mut i = self.bufs.len() - 1;
        while i > 0 && self.bufs[i].fresh {
            i -= 1;
        }
        for j in i + 1..self.bufs.len() {
            self.bufs[j].fresh = false;
            let p = &mut self.bufs[j - 1];
            p.realize(p.ps);
            if p.ps >= p.pe {
                p.pe = NONE;
            }
            p.ps = NONE;
            p.close();
        }
    }

    /// Something visible is about to be written.
    fn begin_visible(&mut self) {
        self.mark();
        self.wake();
        let b = self.b();
        b.realize(b.ps.max(b.pe));
        b.ps = NONE;
        b.pe = NONE;
        b.blank_after = false;
    }

    /// Whitespace, or an empty block that still counts: carries out only
    /// the separator before the current block.
    fn touch(&mut self) {
        self.mark();
        self.wake();
        let b = self.b();
        b.realize(b.ps);
        if b.ps >= b.pe {
            b.pe = NONE;
        }
        b.ps = NONE;
    }

    fn strike_text(&self, t: &str) -> String {
        let strike = self.bufs.last().unwrap().strike;
        let mut out = String::with_capacity(t.len() * (1 + 2 * strike));
        for c in t.chars() {
            out.push(c);
            if char_width(c) > 0 {
                for _ in 0..strike {
                    out.push(STRIKE);
                }
            }
        }
        out
    }

    /// A text node, or markup written as text.
    fn text(&mut self, t: &str) {
        if self.b().pre > 0 {
            return self.pre_text(t);
        }
        let shown = visible(t);
        let struck;
        let t = if self.b().strike > 0 {
            struck = self.strike_text(t);
            &struck[..]
        } else {
            t
        };
        if shown {
            self.begin_visible();
        } else {
            let keeps = !t.chars().all(collapses);
            let b = self.b();
            if b.ps > NONE || b.fresh {
                self.touch();
            }
            let b = self.b();
            if keeps && b.pe == BLANK {
                // No-break spaces right after a paragraph are dropped, and
                // after a line break start the next line.
                return;
            } else if keeps && b.pe == LINE {
                self.begin_visible();
            }
        }
        for c in t.chars() {
            if collapses(c) {
                self.space_char();
            } else if c.is_control() {
            } else if c == ZWSP {
                let b = self.b();
                if b.word_base {
                    b.place_word();
                }
            } else {
                self.word_char(c);
            }
        }
    }

    fn space_char(&mut self) {
        let b = self.b();
        if b.word_base {
            b.place_word();
            b.space = true;
        } else if !b.cur.is_empty() {
            // Marks of zero width stay with the word that follows them,
            // after this space.
            b.space = true;
        }
    }

    fn word_char(&mut self, c: char) {
        let w = char_width(c);
        let style = self.cur_style();
        let b = self.bufs.last_mut().unwrap();
        if b.word_style != style {
            b.piece();
            b.word_style = style;
        }
        b.word.push(c);
        b.word_w += w;
        if w > 0 {
            b.word_base = true;
        }
        b.open = true;
        b.content = true;
    }

    fn pre_text(&mut self, t: &str) {
        let struck;
        let t = if self.b().strike > 0 {
            struck = self.strike_text(t);
            &struck[..]
        } else {
            t
        };
        for (i, seg) in t.split('\n').enumerate() {
            if i > 0 {
                self.br();
            }
            if !seg.is_empty() {
                self.pre_segment(seg);
            }
        }
    }

    fn pre_segment(&mut self, seg: &str) {
        if visible(seg) {
            self.begin_visible();
        } else {
            let b = self.b();
            if b.ps > NONE || b.fresh {
                self.touch();
            }
        }
        let style = self.cur_style();
        for c in seg.chars() {
            let b = self.bufs.last_mut().unwrap();
            let at_start = (b.cur.is_empty() || self.after_tab) && b.word.is_empty();
            if at_start && c != '\t' && c.is_whitespace() && c != '\u{a0}' {
                // Leading spaces count by their width, and only once
                // something follows them on the line.
                self.prews += char_width(c);
                continue;
            }
            if c == '\t' {
                b.place_word();
                if self.prews > 0 {
                    let n = mem::replace(&mut self.prews, 0);
                    b.chop(&" ".repeat(n));
                }
                // To the next tab stop, but not past the end of the line; at
                // the end of one, on the next.
                if b.room() == 0 {
                    b.wrap_line();
                }
                let n = (8 - b.cur_w % 8).min(b.room());
                b.chop(&" ".repeat(n));
                b.open = true;
                b.content = true;
                self.after_tab = true;
                continue;
            }
            if c == ZWSP || c.is_control() {
                continue;
            }
            if self.prews > 0 {
                // Whole lines of leading spaces are dropped.
                let mut n = mem::replace(&mut self.prews, 0);
                if n > b.room() {
                    n -= b.room();
                    if !b.cur.is_empty() {
                        b.wrap_line();
                    }
                    n %= b.wrap;
                }
                b.chop(&" ".repeat(n));
            }
            if b.word_style != style {
                b.piece();
                b.word_style = style;
            }
            b.word.push(c);
            b.word_w += char_width(c);
            if char_width(c) > 0 {
                b.word_base = true;
            }
            b.open = true;
            b.content = true;
            self.after_tab = false;
        }
    }

    fn br(&mut self) {
        self.prews = 0;
        self.after_tab = false;
        self.mark();
        self.wake();
        let b = self.b();
        b.realize(b.ps);
        b.ps = NONE;
        b.place_word();
        b.space = false;
        if b.open && (b.cur_w > 0 || b.wrapped) {
            b.push_cur();
            b.open = false;
        } else {
            // A line of nothing but marks of zero width ends, and still
            // counts as empty.
            if b.open {
                b.push_cur();
                b.open = false;
            }
            b.lines.push(Line::empty());
            b.pe = NONE;
            b.blank_after = false;
        }
    }

    fn start(&mut self, kind: Kind) {
        self.prews = 0;
        self.after_tab = false;
        let b = self.bufs.last_mut().unwrap();
        let saved = (b.ps, b.pe);
        b.ps = b.ps.max(kind.level());
        self.frames.push(Frame {
            kind,
            wrote: false,
            saved,
        });
    }

    fn end(&mut self) {
        let f = self.frames.pop().unwrap();
        let b = self.bufs.last_mut().unwrap();
        if !f.wrote {
            (b.ps, b.pe) = f.saved;
            return;
        }
        let level = f.kind.end_level();
        if f.kind.level() == BLANK {
            b.blank_after = false;
        }
        if matches!(f.kind, Kind::Lead(_)) {
            return;
        }
        b.pe = b.pe.max(level);
        if f.kind == Kind::Div {
            b.close();
        }
    }

    fn start_prefixed(&mut self, level: u8, prefix_w: usize, eager: bool) {
        self.start(Kind::Prefixed(level));
        let parent = self.b();
        let width = parent.width.saturating_sub(prefix_w).max(MIN_WIDTH);
        self.bufs
            .push(Buf::new(width, self.opts.max_wrap_width, true));
        if eager {
            self.mark();
            self.wake();
        }
    }

    fn end_prefixed(&mut self, first: &str, rest: &str) {
        let lines = self.bufs.pop().unwrap().finish();
        if !lines.is_empty() {
            let b = self.b();
            for (i, mut line) in lines.into_iter().enumerate() {
                let prefix = if i == 0 { first } else { rest };
                if !prefix.is_empty() {
                    line.add_prefix(prefix);
                }
                b.lines.push(line);
            }
            b.blank_after = false;
            b.content = true;
        }
        self.end();
    }

    /// Render the children of `node` into this renderer.
    /// Carry on with `stack`, until it is done or there is a table to write.
    fn run(&mut self, stack: &mut Vec<Step>) -> Option<Handle> {
        while let Some(step) = stack.pop() {
            match step {
                Step::Table(node) => return Some(node),
                Step::Enter(node) => self.enter(&node, stack),
                Step::Item {
                    node,
                    first,
                    rest,
                    li,
                } => {
                    self.start_prefixed(LINE, str_width(&first), true);
                    stack.push(Step::Leave(Leave::Prefixed { first, rest }));
                    if li {
                        push_children(stack, &node);
                    } else {
                        stack.push(Step::Enter(node));
                    }
                }
                Step::Def(node) => {
                    if html_name(&node) == Some("dt") {
                        self.dt(&node, stack);
                    } else {
                        self.dd(&node, stack);
                    }
                }
                Step::Touch => {
                    let b = self.b();
                    if b.ps > NONE || b.fresh {
                        self.touch();
                    }
                }
                Step::Leave(leave) => self.leave(leave),
            }
        }
        None
    }

    fn leave(&mut self, leave: Leave) {
        match leave {
            Leave::Nothing => {}
            Leave::Text(t) => self.text(t),
            Leave::Styled(t) => {
                self.style.pop();
                if !t.is_empty() {
                    self.text(t);
                }
            }
            Leave::Frame => self.end(),
            Leave::Pre => {
                self.prews = 0;
                self.after_tab = false;
                self.b().pre -= 1;
                self.end();
            }
            Leave::Strike => {
                self.style.pop();
                self.b().strike -= 1;
            }
            Leave::Link(url) => {
                self.style.pop();
                if self.opts.link_footnotes {
                    self.footnotes.push(url);
                    let n = self.footnotes.len();
                    self.text(&format!("][{n}]"));
                } else {
                    self.text("]");
                }
            }
            Leave::Prefixed { first, rest } => self.end_prefixed(&first, &rest),
        }
    }

    fn dt(&mut self, node: &Handle, stack: &mut Vec<Step>) {
        self.start(Kind::Lead(LINE));
        stack.push(Step::Leave(Leave::Frame));
        // Unlike emphasis, the markers are part of the term's style.
        self.push_style(b'd');
        stack.push(Step::Leave(Leave::Styled("")));
        if self.opts.decorate {
            self.text("*");
            stack.push(Step::Leave(Leave::Text("*")));
        } else {
            self.touch();
            self.b().blank_after = false;
        }
        push_children(stack, node);
    }

    fn dd(&mut self, node: &Handle, stack: &mut Vec<Step>) {
        self.start_prefixed(LINE, 2, true);
        stack.push(Step::Leave(Leave::Prefixed {
            first: "  ".into(),
            rest: "  ".into(),
        }));
        push_children(stack, node);
    }

    fn enter(&mut self, node: &Handle, stack: &mut Vec<Step>) {
        let (name, attrs) = match &node.data {
            NodeData::Text { contents } => {
                let t = contents.borrow();
                self.text(&t);
                return;
            }
            NodeData::Element { name, attrs, .. } => (name, attrs),
            NodeData::Document => {
                push_children(stack, node);
                return;
            }
            NodeData::Other => return,
        };
        if name.ns != html5ever::ns!(html) {
            if name.ns == html5ever::ns!(svg) {
                // Not rendered, but counts as something; a title is shown.
                if &*name.local == "svg" {
                    if let Some(title) = svg_title(node).filter(|t| !t.is_empty()) {
                        self.text(&format!("[{title}]"));
                        return;
                    }
                }
                let b = self.b();
                if b.ps > NONE || b.fresh {
                    self.touch();
                }
                return;
            }
            push_children(stack, node);
            return;
        }
        let decorate = self.opts.decorate;
        let attrs = attrs.borrow();
        let attr = |n: &str| {
            attrs
                .iter()
                .find(|a| a.name.ns.is_empty() && &*a.name.local == n)
                .map(|a| &*a.value)
        };
        if matches!(&*name.local, "i" | "ins" | "del" | "s" | "wbr")
            || (!decorate && matches!(&*name.local, "b" | "strong" | "em" | "code"))
            || (&*name.local == "a" && attr("href").is_none())
            || attr("id").is_some_and(|id| !id.is_empty())
            || (&*name.local == "img" && attr("src").is_some_and(|s| !s.is_empty()))
        {
            // These count as something even when empty.
            let b = self.b();
            if b.ps > NONE || b.fresh {
                self.touch();
            }
        }
        let leave = match &*name.local {
            "script" | "style" | "template" | "head" => return,
            "br" => {
                self.br();
                return;
            }
            "wbr" => {
                // A place the line may break, as a zero-width space is.
                let b = self.b();
                if b.word_base {
                    b.place_word();
                }
                return;
            }
            "img" => {
                if attr("src").is_some_and(|s| !s.is_empty()) {
                    if let Some(alt) = attr("alt").filter(|a| !a.is_empty()) {
                        let t = format!("[{alt}]");
                        self.text(&t);
                    }
                }
                return;
            }
            "b" | "strong" | "em" | "code" | "i" | "ins" | "sup" => {
                let (open, close, id) = match &*name.local {
                    "b" | "strong" if decorate => ("**", "**", b'b'),
                    "b" | "strong" => ("", "", b'b'),
                    "em" if decorate => ("*", "*", b'e'),
                    "em" => ("", "", b'e'),
                    "code" => {
                        // Unlike emphasis, the markers are part of the code's
                        // style.
                        self.push_style(b'c');
                        if decorate {
                            self.text("`");
                            stack.push(Step::Leave(Leave::Styled("")));
                            stack.push(Step::Leave(Leave::Text("`")));
                        } else {
                            stack.push(Step::Leave(Leave::Styled("")));
                        }
                        push_children(stack, node);
                        return;
                    }
                    "i" => ("", "", b'i'),
                    "ins" => ("", "", b'n'),
                    _ if sup_digits(node).is_some() => {
                        let digits = sup_digits(node).unwrap();
                        self.push_style(b's');
                        self.text(&digits);
                        self.style.pop();
                        return;
                    }
                    _ => ("^{", "}", b's'),
                };
                if !open.is_empty() {
                    self.text(open);
                }
                self.push_style(id);
                Leave::Styled(close)
            }
            "del" | "s" => {
                self.b().strike += 1;
                self.push_style(b'x');
                Leave::Strike
            }
            "a" => match attr("href") {
                Some(href) => {
                    if !link_has_content(node, decorate) {
                        return;
                    }
                    let href = href.replace('\n', " ");
                    let url = match &self.opts.base_url {
                        Some(base) => url::resolve(base, &href),
                        None => href,
                    };
                    self.text("[");
                    self.push_style(b'a');
                    Leave::Link(url)
                }
                None => Leave::Nothing,
            },
            "p" => {
                self.start(Kind::Para);
                Leave::Frame
            }
            "div" => {
                self.start(Kind::Div);
                Leave::Frame
            }
            "pre" => {
                self.start(Kind::Pre);
                self.touch();
                self.b().pre += 1;
                Leave::Pre
            }
            "blockquote" => {
                self.start_prefixed(BLANK, 2, false);
                Leave::Prefixed {
                    first: "> ".into(),
                    rest: "> ".into(),
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = usize::from(name.local.as_bytes()[1] - b'0');
                let prefix = format!("{} ", "#".repeat(level));
                self.start_prefixed(BLANK, level + 1, true);
                Leave::Prefixed {
                    first: prefix.clone(),
                    rest: prefix,
                }
            }
            "ul" => {
                self.start(Kind::List);
                stack.push(Step::Leave(Leave::Frame));
                let children = node.children.borrow();
                for child in children.iter().rev() {
                    let li = html_name(child) == Some("li");
                    if !li {
                        if let NodeData::Text { contents } = &child.data {
                            if contents.borrow().chars().all(collapses) {
                                stack.push(Step::Touch);
                                continue;
                            }
                        }
                    }
                    stack.push(Step::Item {
                        node: child.clone(),
                        first: "* ".into(),
                        rest: "  ".into(),
                        li,
                    });
                }
                return;
            }
            "ol" => {
                self.start(Kind::List);
                stack.push(Step::Leave(Leave::Frame));
                let children = node.children.borrow();
                let items: Vec<&Handle> = children
                    .iter()
                    .filter(|c| html_name(c) == Some("li"))
                    .collect();
                if children
                    .iter()
                    .any(|c| html_name(c) != Some("li") && survives(c))
                {
                    // Not shown, but a block around the list that has
                    // nothing else yet still counts as started.
                    self.wake();
                }
                let start = attr("start")
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(1);
                let labels: Vec<String> = (0..items.len() as i64)
                    .map(|i| format!("{}.", start.wrapping_add(i)))
                    .collect();
                let w = labels.iter().map(|l| l.len()).max().unwrap_or(0);
                for (item, label) in items.iter().zip(labels).rev() {
                    stack.push(Step::Item {
                        node: (*item).clone(),
                        first: format!("{label:<w$} "),
                        rest: " ".repeat(w + 1),
                        li: true,
                    });
                }
                return;
            }
            "dl" => {
                self.start(Kind::Lead(BLANK));
                stack.push(Step::Leave(Leave::Frame));
                let children = node.children.borrow();
                for child in children.iter().rev() {
                    if matches!(html_name(child), Some("dt" | "dd")) {
                        stack.push(Step::Def(child.clone()));
                    } else if survives(child) {
                        // Not shown, but counts as something.
                        stack.push(Step::Touch);
                    }
                }
                return;
            }
            "dt" => {
                self.dt(node, stack);
                return;
            }
            "dd" => {
                self.dd(node, stack);
                return;
            }
            "li" => {
                self.start(Kind::LooseItem);
                self.mark();
                self.wake();
                let b = self.b();
                b.realize(b.ps.max(b.pe));
                b.ps = NONE;
                b.pe = NONE;
                b.blank_after = false;
                Leave::Frame
            }
            "table" => {
                stack.push(Step::Table(node.clone()));
                return;
            }
            _ => Leave::Nothing,
        };
        stack.push(Step::Leave(leave));
        push_children(stack, node);
    }

    /// Start a table; one with rows counts as something even if nothing
    /// in it shows.
    pub fn begin_table(&mut self, has_rows: bool) {
        self.start(Kind::Table);
        if has_rows {
            self.touch();
            self.b().pe = NONE;
        }
    }

    /// Finish a table with its rendered lines.
    pub fn end_table(&mut self, lines: Vec<Line>) {
        if lines.is_empty() {
            self.end();
            return;
        }
        self.bufs.push(Buf::new(1, None, true));
        self.bufs.last_mut().unwrap().lines = lines;
        self.mark();
        self.wake();
        self.end_table_buf();
    }

    fn end_table_buf(&mut self) {
        let lines = self.bufs.pop().unwrap().lines;
        let b = self.b();
        b.lines.extend(lines);
        b.blank_after = false;
        b.content = true;
        self.end();
    }

    pub fn width(&self) -> usize {
        self.bufs.last().unwrap().width
    }

    pub fn opts(&self) -> &Options {
        self.opts
    }

    fn finish(mut self) -> String {
        let footnotes = mem::take(&mut self.footnotes);
        let opts = self.opts;
        let width = self.bufs[0].width;
        let mut b = self.bufs.pop().unwrap();
        if opts.link_footnotes && !footnotes.is_empty() {
            b.realize(BLANK);
            b.close();
            for (i, url) in footnotes.iter().enumerate() {
                let line = format!("[{}]: {url}", i + 1);
                if opts.wrap_links {
                    let mut f = Buf::new(width, None, false);
                    f.chop(&line);
                    f.push_cur();
                    b.lines.extend(f.lines);
                } else {
                    b.lines.push(Line::new(line.clone(), line.width()));
                }
            }
        }
        let lines = b.finish();
        let mut out = String::new();
        for line in lines {
            out.extend(line.rprefix.chars().rev());
            out.push_str(&line.text);
            out.push('\n');
        }
        out
    }
}

/// A superscript that is only digits, written with superscript digits.
fn sup_digits(sup: &Handle) -> Option<String> {
    let children = sup.children.borrow();
    let [only] = &children[..] else {
        return None;
    };
    let NodeData::Text { contents } = &only.data else {
        return None;
    };
    let t = contents.borrow();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(
        t.chars()
            .map(|c| ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'][c as usize - '0' as usize])
            .collect(),
    )
}

/// The text of an `<svg>`'s last `<title>`, if it has one.
pub fn svg_title(svg: &Handle) -> Option<String> {
    let children = svg.children.borrow();
    let title = children.iter().rev().find(|c| match &c.data {
        NodeData::Element { name, .. } => &*name.local == "title",
        _ => false,
    })?;
    let mut text = String::new();
    for c in title.children.borrow().iter() {
        if let NodeData::Text { contents } = &c.data {
            text.push_str(&contents.borrow());
        }
    }
    Some(text)
}

/// The width of a word on a line: each piece measured as a string.
fn pieces_width(word: &str, pieces: &[usize]) -> usize {
    let mut start = 0;
    let mut w = 0;
    for &end in pieces.iter().chain([&word.len()]) {
        w += word[start..end].width();
        start = end;
    }
    w
}

fn push_children(stack: &mut Vec<Step>, node: &Handle) {
    let children = node.children.borrow();
    stack.extend(children.iter().rev().cloned().map(Step::Enter));
}

fn children(node: &Handle) -> Vec<Step> {
    let mut stack = Vec::new();
    push_children(&mut stack, node);
    stack
}

pub fn html_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } if name.ns == html5ever::ns!(html) => Some(&name.local),
        _ => None,
    }
}

/// Whether a link has anything to show: text, an image with alternative
/// text, markup that writes something, or an element with something left in
/// it once empty elements are dropped.
pub fn link_has_content(link: &Handle, decorate: bool) -> bool {
    link.children.borrow().iter().any(|c| match &c.data {
        NodeData::Text { contents } => visible(&contents.borrow()),
        NodeData::Element { name, attrs, .. } => {
            let attrs = attrs.borrow();
            let get = |n: &str| {
                attrs
                    .iter()
                    .find(|a| a.name.ns.is_empty() && &*a.name.local == n)
                    .map(|a| &*a.value)
            };
            let id = get("id").is_some_and(|id| !id.is_empty());
            if name.ns == html5ever::ns!(svg) && &*name.local == "svg" {
                return id || svg_title(c).is_some_and(|t| visible(&t));
            }
            if name.ns == html5ever::ns!(html) {
                match &*name.local {
                    "img" => {
                        return id
                            || (get("src").is_some_and(|s| !s.is_empty())
                                && get("alt").is_some_and(visible));
                    }
                    "br" => return id,
                    "b" | "strong" | "em" | "code" | "dt" if decorate => return true,
                    _ => {}
                }
            }
            c.children.borrow().iter().any(survives)
        }
        _ => false,
    })
}

/// Whether a node is kept when empty elements are dropped. Some elements
/// are kept even when empty.
pub fn survives(node: &Handle) -> bool {
    let mut stack = vec![node.clone()];
    while let Some(node) = stack.pop() {
        match &node.data {
            NodeData::Text { .. } => return true,
            NodeData::Element { name, attrs, .. } => {
                if name.ns == html5ever::ns!(svg) && &*name.local == "svg" {
                    return true;
                }
                if name.ns == html5ever::ns!(html)
                    && &*name.local == "img"
                    && attrs.borrow().iter().any(|a| {
                        a.name.ns.is_empty() && &*a.name.local == "id" && !a.value.is_empty()
                    })
                {
                    return true;
                }
                if name.ns == html5ever::ns!(html) {
                    match &*name.local {
                        "b" | "br" | "code" | "dd" | "del" | "dt" | "em" | "h1" | "h2" | "h3"
                        | "h4" | "h5" | "h6" | "i" | "ins" | "li" | "pre" | "s" | "strong"
                        | "sup" | "wbr" => return true,
                        "img" => {
                            if attrs.borrow().iter().any(|a| {
                                a.name.ns.is_empty()
                                    && &*a.name.local == "src"
                                    && !a.value.is_empty()
                            }) {
                                return true;
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                stack.extend(node.children.borrow().iter().cloned());
            }
            _ => {}
        }
    }
    false
}
