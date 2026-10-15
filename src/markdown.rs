//! HTML to Markdown.
//!
//! The output is CommonMark plus the two GitHub Flavored Markdown extensions
//! that HTML needs, pipe tables and `~~strikethrough~~`, written in the most
//! common conventions: `#` headings, `-` bullets, `_em_` and `**strong**`,
//! fenced code blocks, inline links and images, one blank line between blocks,
//! and no trailing whitespace except the two spaces of a hard line break.
//!
//! The document is parsed by html2text (and so by the patched html5ever) and
//! walked with an explicit stack rather than recursion, so no input can
//! overflow the thread's stack.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::mem;
use std::rc::Rc;

use html2text::{Handle, NodeData, RcDom};
use html5ever::Attribute;

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Blockquotes and lists nested deeper than this are not indented any
/// further. Every line repeats the prefixes of everything around it, so
/// unbounded nesting would make the output quadratic in the input.
const MAX_DEPTH: usize = 32;

/// A table whose grid, with row and column spans expanded into empty cells,
/// would be larger than this is written as its cells alone, for the same
/// reason.
const MAX_GRID_CELLS: usize = 100_000;

/// HTML's own limits on `colspan` and `rowspan`.
const MAX_COLSPAN: usize = 1000;
const MAX_ROWSPAN: usize = 65534;

/// Convert an HTML document to Markdown. Relative links and image sources are
/// resolved against `base_url` when one is given.
pub fn from_read(input: &[u8], base_url: Option<&str>) -> Result<String, html2text::Error> {
    let dom = html2text::config::plain().parse_html(input)?;
    Ok(convert(&dom, base_url.filter(|base| !base.is_empty())))
}

fn convert(dom: &RcDom, base_url: Option<&str>) -> String {
    // Emphasis is written `_like this_` unless it touches a letter, where only
    // `*` works. A document that needed both is written again preferring `*`,
    // so that it uses one style. A few constructs can only be written one way
    // (`**_class_ dict(_\*\*kwargs_)**` has no form with `*` that parses), so
    // when neither attempt is consistent the one that mixes less is kept.
    let (out, used) = convert_with(dom, base_url, EmStyle::Underscore);
    if used.minority() == 0 {
        return out;
    }
    let (again, used_again) = convert_with(dom, base_url, EmStyle::Asterisk);
    if used_again.minority() < used.minority() {
        again
    } else {
        out
    }
}

fn convert_with(dom: &RcDom, base_url: Option<&str>, em_style: EmStyle) -> (String, EmUse) {
    let mut conv = Converter::new(base_url, em_style);
    let mut stack = vec![Step::Enter(dom.document.clone())];
    while let Some(step) = stack.pop() {
        match step {
            Step::Enter(node) => {
                if let Some(leave) = conv.enter(&node) {
                    stack.push(Step::Leave(leave));
                    stack.extend(
                        node.children
                            .borrow()
                            .iter()
                            .rev()
                            .cloned()
                            .map(Step::Enter),
                    );
                }
            }
            Step::Leave(leave) => conv.leave(leave),
        }
    }
    let used = conv.em_used;
    (conv.finish(), used)
}

enum Step {
    Enter(Handle),
    Leave(Leave),
}

/// What to do once an element's children have been converted.
enum Leave {
    Nothing,
    Block,
    Heading,
    /// The flag says whether a container was pushed (see `MAX_DEPTH`).
    Quote(bool),
    List(bool),
    Item(bool),
    Term,
    Link(Rc<Link>, Option<String>),
    Strong,
    Em,
    Strike,
    Code,
    Quotation,
    RubyText,
    Table,
    Caption,
    Section(Section),
    Cell,
}

// ---------------------------------------------------------------------------
// Inline content
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Link {
    url: String,
    title: Option<String>,
}

/// The inline formatting in effect for a run of text.
#[derive(Clone, Default)]
struct Marks {
    strong: bool,
    em: bool,
    strike: bool,
    /// The `<code>` element the text is in, numbered so that two adjacent
    /// ones stay separate spans; 0 outside code.
    code: u32,
    link: Option<Rc<Link>>,
}

impl PartialEq for Marks {
    fn eq(&self, other: &Self) -> bool {
        self.strong == other.strong
            && self.em == other.em
            && self.strike == other.strike
            && self.code == other.code
            && same_link(&self.link, &other.link)
    }
}

fn same_link(a: &Option<Rc<Link>>, b: &Option<Rc<Link>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

#[derive(Clone)]
enum RunKind {
    /// Text with its whitespace already collapsed.
    Text(String),
    Image {
        alt: String,
        src: String,
        title: Option<String>,
    },
    /// A hard line break (`<br>`).
    Break,
}

#[derive(Clone)]
struct Run {
    kind: RunKind,
    marks: Marks,
}

impl Run {
    /// Whether the run shows anything, as opposed to whitespace or a break.
    fn is_content(&self) -> bool {
        match &self.kind {
            RunKind::Text(text) => text.bytes().any(|b| b != b' '),
            RunKind::Image { .. } => true,
            RunKind::Break => false,
        }
    }
}

/// Where a run of inline content ends up, which decides how it is escaped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A paragraph: hard line breaks, and escapes for syntax that only means
    /// something at the start of a line.
    Block,
    /// A heading's text, which is a single line.
    Heading,
    /// A table cell: `|` is escaped and line breaks are written as `<br>`.
    Cell,
}

/// An open inline construct. When several open at once they nest in this
/// order: a link outermost, so that formatting which changes partway through
/// its text doesn't split it in two, and a code span innermost, since nothing
/// else can be written inside one.
#[derive(Clone)]
enum Mark {
    Link(Rc<Link>),
    Strong,
    /// Emphasis, and the character it is delimited with.
    Em(char),
    Strike,
    Code(u32),
}

impl PartialEq for Mark {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Mark::Link(a), Mark::Link(b)) => Rc::ptr_eq(a, b),
            (Mark::Code(a), Mark::Code(b)) => a == b,
            _ => mem::discriminant(self) == mem::discriminant(other),
        }
    }
}

impl Mark {
    fn is_code(&self) -> bool {
        matches!(self, Mark::Code(_))
    }
}

fn wanted(marks: &Marks, em: char) -> Vec<Mark> {
    let mut want = Vec::with_capacity(5);
    if let Some(link) = &marks.link {
        want.push(Mark::Link(link.clone()));
    }
    if marks.strong {
        want.push(Mark::Strong);
    }
    if marks.em {
        want.push(Mark::Em(em));
    }
    if marks.strike {
        want.push(Mark::Strike);
    }
    if marks.code != 0 {
        want.push(Mark::Code(marks.code));
    }
    want
}

/// How emphasis is delimited.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EmStyle {
    /// `_emphasis_`, the usual convention, except where a letter or digit
    /// touches it: `_` can't open or close inside a word, so there it is `*`.
    Underscore,
    Asterisk,
}

/// How many emphasis delimiters of each kind a document has used.
#[derive(Clone, Copy, Default)]
struct EmUse {
    underscore: usize,
    asterisk: usize,
}

impl EmUse {
    /// How many are not in the document's majority style.
    fn minority(self) -> usize {
        self.underscore.min(self.asterisk)
    }
}

/// Write runs of inline content as Markdown, preferring one emphasis style
/// and noting in `used` which delimiters were written.
fn serialize(runs: &[Run], mode: Mode, prefer: EmStyle, used: &mut EmUse) -> String {
    // Spaces and line breaks at either end of the content mean nothing.
    let start = runs.iter().position(Run::is_content).unwrap_or(runs.len());
    let end = runs
        .iter()
        .rposition(Run::is_content)
        .map_or(start, |i| i + 1);
    let runs = merge_runs(&fix_flanking(&runs[start..end]));
    if !runs
        .iter()
        .any(|r| r.marks.strong || r.marks.em || r.marks.strike)
    {
        return write_inline(&runs, mode, prefer).0;
    }
    // Whether a `*`, `_` or `~~` opens or closes depends on the characters
    // around it and on every other delimiter in the paragraph, so no local
    // rule gets every case right. Each attempt is checked: every delimiter
    // against the flanking rules, then the whole read back with a CommonMark
    // parser.
    let other = match prefer {
        EmStyle::Underscore => EmStyle::Asterisk,
        EmStyle::Asterisk => EmStyle::Underscore,
    };
    for style in [prefer, other] {
        let (out, delimiters) = write_inline(&runs, mode, style);
        if delimiters_flank(&out, &delimiters) && reads_back(&out, &runs, mode) {
            for d in delimiters.iter().filter(|d| d.len == 1) {
                match out.as_bytes()[d.at] {
                    b'_' => used.underscore += 1,
                    b'*' => used.asterisk += 1,
                    _ => {}
                }
            }
            return out;
        }
    }
    // Last resort: the text, links and code, without the formatting.
    let plain: Vec<Run> = runs
        .iter()
        .map(|run| Run {
            kind: run.kind.clone(),
            marks: Marks {
                strong: false,
                em: false,
                strike: false,
                ..run.marks.clone()
            },
        })
        .collect();
    write_inline(&merge_runs(&plain), mode, prefer).0
}

/// Join neighbouring text runs with the same formatting, so that escaping
/// sees all the text that ends up side by side: `&copy` followed by `;` is
/// an entity even if the two were collected separately.
fn merge_runs(runs: &[Run]) -> Vec<Run> {
    let mut merged: Vec<Run> = Vec::with_capacity(runs.len());
    for run in runs {
        if let (
            Some(Run {
                kind: RunKind::Text(previous),
                marks,
            }),
            RunKind::Text(text),
        ) = (merged.last_mut(), &run.kind)
        {
            if *marks == run.marks {
                previous.push_str(text);
                continue;
            }
        }
        merged.push(run.clone());
    }
    merged
}

fn write_inline(runs: &[Run], mode: Mode, style: EmStyle) -> (String, Vec<Delimiter>) {
    let delimiters = em_delimiters(runs, style);
    let mut writer = InlineWriter {
        out: String::new(),
        mode,
        open: Vec::new(),
        code: String::new(),
        last_span: None,
        link: None,
        pending_space: false,
        line_start: true,
        delimiters: Vec::new(),
    };
    for (i, (run, &em)) in runs.iter().zip(&delimiters).enumerate() {
        let link_starts = run.marks.link.is_some()
            && (i == 0 || !same_link(&runs[i - 1].marks.link, &run.marks.link));
        if link_starts {
            // Formatting already open that stops partway through the link
            // must close before it and reopen inside: Markdown can't overlap
            // the two, and splitting the link instead would repeat the URL.
            let span = runs[i..]
                .iter()
                .take_while(|r| same_link(&r.marks.link, &run.marks.link));
            let (mut strong, mut em_all, mut strike) = (true, true, true);
            for r in span {
                strong &= r.marks.strong;
                em_all &= r.marks.em;
                strike &= r.marks.strike;
            }
            writer.close_unless(strong, em_all, strike);
        }
        writer.run(run, em);
    }
    writer.close_to(0);
    (writer.out, writer.delimiters)
}

/// An emphasis, strong or strikethrough marker written at `at`.
struct Delimiter {
    at: usize,
    len: usize,
    opens: bool,
}

/// Whether every marker can do its job under CommonMark's flanking rules,
/// which GitHub also applies to `~~`: one that opens must be left-flanking
/// and one that closes right-flanking, with `_` stricter still inside words.
/// Markers of the same character side by side form a single run.
fn delimiters_flank(out: &str, delimiters: &[Delimiter]) -> bool {
    let bytes = out.as_bytes();
    let mut i = 0;
    while i < delimiters.len() {
        let first = &delimiters[i];
        let marker = bytes[first.at];
        let (mut end, mut opens, mut closes) = (first.at + first.len, first.opens, !first.opens);
        i += 1;
        while let Some(next) = delimiters.get(i) {
            if next.at != end || bytes[next.at] != marker {
                break;
            }
            end += next.len;
            opens |= next.opens;
            closes |= !next.opens;
            i += 1;
        }
        let before = out[..first.at].chars().next_back();
        let after = out[end..].chars().next();
        let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
        let punct = |c: Option<char>| c.is_some_and(is_punct);
        let left = !space(after) && (!punct(after) || space(before) || punct(before));
        let right = !space(before) && (!punct(before) || space(after) || punct(after));
        let (can_open, can_close) = if marker == b'_' {
            (
                left && (!right || punct(before)),
                right && (!left || punct(after)),
            )
        } else {
            (left, right)
        };
        if (opens && !can_open) || (closes && !can_close) {
            return false;
        }
    }
    true
}

/// The emphasis delimiter for each run, chosen for each stretch of
/// emphasized runs from the characters on either side of it.
fn em_delimiters(runs: &[Run], style: EmStyle) -> Vec<char> {
    let mut delimiters = vec!['*'; runs.len()];
    if style == EmStyle::Asterisk {
        return delimiters;
    }
    // The character a delimiter will actually touch: a code span's backtick
    // rather than its first letter.
    let first_char = |run: &Run| match &run.kind {
        RunKind::Text(_) if run.marks.code != 0 => Some('`'),
        RunKind::Text(text) => text.chars().next(),
        _ => None,
    };
    let last_char = |run: &Run| match &run.kind {
        RunKind::Text(_) if run.marks.code != 0 => Some('`'),
        RunKind::Text(text) => text.chars().next_back(),
        _ => None,
    };
    let wordlike = |c: Option<char>| c.is_some_and(|c| !c.is_whitespace() && !is_punct(c));
    let mut i = 0;
    while i < runs.len() {
        if !runs[i].marks.em {
            i += 1;
            continue;
        }
        let start = i;
        while i < runs.len() && runs[i].marks.em {
            i += 1;
        }
        // A space at either end is written outside the delimiters.
        let before = match first_char(&runs[start]) {
            Some(' ') => None,
            _ => start.checked_sub(1).and_then(|j| last_char(&runs[j])),
        };
        let after = match last_char(&runs[i - 1]) {
            Some(' ') => None,
            _ => runs.get(i).and_then(first_char),
        };
        let delimiter = if wordlike(before) || wordlike(after) {
            '*'
        } else {
            '_'
        };
        delimiters[start..i].fill(delimiter);
    }
    delimiters
}

/// Whether `out`, read by a CommonMark parser, has the text of `runs` with
/// the same emphasis, strong emphasis and strikethrough on each character.
fn reads_back(out: &str, runs: &[Run], mode: Mode) -> bool {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let bits = |em: bool, strong: bool, strike: bool| {
        u8::from(em) | u8::from(strong) << 1 | u8::from(strike) << 2
    };
    let mut expected = runs
        .iter()
        .filter_map(|run| match &run.kind {
            RunKind::Text(text) => {
                Some((text, bits(run.marks.em, run.marks.strong, run.marks.strike)))
            }
            _ => None,
        })
        .flat_map(|(text, style)| {
            text.chars()
                .filter(|c| !c.is_whitespace())
                .map(move |c| (c, style))
        });

    let doc: Cow<str> = match mode {
        Mode::Block => out.into(),
        Mode::Heading => format!("# {out} #").into(),
        Mode::Cell => format!("| |\n| - |\n| {out} |").into(),
    };
    let (mut em, mut strong, mut strike, mut image) = (0u32, 0u32, 0u32, 0u32);
    for event in Parser::new_ext(&doc, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(Tag::Emphasis) => em += 1,
            Event::End(TagEnd::Emphasis) => em = em.saturating_sub(1),
            Event::Start(Tag::Strong) => strong += 1,
            Event::End(TagEnd::Strong) => strong = strong.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => strike += 1,
            Event::End(TagEnd::Strikethrough) => strike = strike.saturating_sub(1),
            // An image's alt text is not part of the text.
            Event::Start(Tag::Image { .. }) => image += 1,
            Event::End(TagEnd::Image) => image = image.saturating_sub(1),
            Event::Text(text) | Event::Code(text) if image == 0 => {
                let style = bits(em > 0, strong > 0, strike > 0);
                if text
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .any(|c| expected.next() != Some((c, style)))
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    expected.next().is_none()
}

/// CommonMark's flanking rules don't let `**` close after punctuation unless
/// a space or more punctuation follows, nor open before punctuation unless
/// one precedes it. So `**타이타닉(Titanic)**의`, common in Korean and other
/// languages without spaces before particles, stays literal. Moving the
/// punctuation outside the markers, `**타이타닉(Titanic**)의`, keeps the
/// emphasis and every character.
fn fix_flanking(runs: &[Run]) -> Cow<'_, [Run]> {
    /// Where formatting closes (or opens) between punctuation and a letter.
    enum Fix {
        Close,
        Open,
    }
    let needs_fix = |a: &Run, b: &Run| -> Option<Fix> {
        let (RunKind::Text(ta), RunKind::Text(tb)) = (&a.kind, &b.kind) else {
            return None;
        };
        if a.marks.code != 0 || b.marks.code != 0 || !same_link(&a.marks.link, &b.marks.link) {
            return None;
        }
        let (last, first) = (ta.chars().next_back()?, tb.chars().next()?);
        let wordlike = |c: char| !c.is_whitespace() && !is_punct(c);
        let closes = (a.marks.strong && !b.marks.strong)
            || (a.marks.em && !b.marks.em)
            || (a.marks.strike && !b.marks.strike);
        let opens = (b.marks.strong && !a.marks.strong)
            || (b.marks.em && !a.marks.em)
            || (b.marks.strike && !a.marks.strike);
        if closes && is_punct(last) && wordlike(first) {
            Some(Fix::Close)
        } else if opens && wordlike(last) && is_punct(first) {
            Some(Fix::Open)
        } else {
            None
        }
    };
    if !runs.windows(2).any(|w| needs_fix(&w[0], &w[1]).is_some()) {
        return Cow::Borrowed(runs);
    }

    let mut out: Vec<Run> = Vec::with_capacity(runs.len() + 2);
    for run in runs {
        let mut next = run.clone();
        let Some(prev) = out.last_mut() else {
            out.push(next);
            continue;
        };
        let Some(fix) = needs_fix(prev, &next) else {
            out.push(next);
            continue;
        };
        let (RunKind::Text(ta), RunKind::Text(tb)) = (&mut prev.kind, &mut next.kind) else {
            unreachable!("needs_fix only matches text runs");
        };
        let styled = match fix {
            Fix::Close => &*ta,
            Fix::Open => &*tb,
        };
        if styled.chars().all(is_punct) {
            // Nothing to move it away from: formatting on punctuation alone,
            // against a letter, can't be written. Drop that formatting.
            let (keep, from) = match fix {
                Fix::Close => (next.marks.clone(), &mut prev.marks),
                Fix::Open => (prev.marks.clone(), &mut next.marks),
            };
            from.strong &= keep.strong;
            from.em &= keep.em;
            from.strike &= keep.strike;
            out.push(next);
            continue;
        }
        // The punctuation goes in a run of its own, with only the marks
        // shared by both sides.
        let moved = match fix {
            Fix::Close => {
                let cut = ta.trim_end_matches(is_punct).len();
                ta.split_off(cut)
            }
            Fix::Open => {
                let cut = tb.len() - tb.trim_start_matches(is_punct).len();
                tb.drain(..cut).collect()
            }
        };
        let shared = Marks {
            strong: prev.marks.strong && next.marks.strong,
            em: prev.marks.em && next.marks.em,
            strike: prev.marks.strike && next.marks.strike,
            code: 0,
            link: prev.marks.link.clone(),
        };
        out.push(Run {
            kind: RunKind::Text(moved),
            marks: shared,
        });
        out.push(next);
    }
    Cow::Owned(out)
}

/// CommonMark's punctuation: the Unicode P and S categories, approximated as
/// ASCII punctuation plus any other character that is not a letter, digit,
/// space or control character.
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control())
}

struct OpenLink {
    /// Where the link's `[` was written, so it can become an autolink.
    start: usize,
    /// The link's text as it reads, before escaping.
    text: String,
    /// Whether the text is plain: no images, formatting or line breaks.
    plain: bool,
}

struct InlineWriter {
    out: String,
    mode: Mode,
    open: Vec<Mark>,
    /// The text of the open code span, written out when it closes.
    code: String,
    /// Where the last code span was written and what it held. Two spans
    /// side by side would read as one span containing backticks, so a span
    /// that opens right where the last one ended continues it instead.
    last_span: Option<(usize, String)>,
    link: Option<OpenLink>,
    /// A space that belongs between the last thing written and the next.
    /// Held back so it lands outside any markers that close or open there:
    /// `**a **b` would not close the emphasis, `**a** b` does.
    pending_space: bool,
    /// Nothing has been written on the current line yet.
    line_start: bool,
    delimiters: Vec<Delimiter>,
}

impl InlineWriter {
    fn run(&mut self, run: &Run, em: char) {
        match &run.kind {
            RunKind::Text(text) => {
                let core = text.trim_matches(' ');
                if core.is_empty() {
                    // A space between two code spans is not part of either.
                    if run.marks.code == 0 && self.open.last().is_some_and(Mark::is_code) {
                        self.close_to(self.open.len() - 1);
                    }
                    self.pending_space = true;
                    return;
                }
                self.pending_space |= text.starts_with(' ');
                self.transition(&run.marks, em);
                self.write_text(core);
                self.pending_space = text.ends_with(' ');
            }
            RunKind::Image { alt, src, title } => {
                self.transition(&run.marks, em);
                self.write_image(alt, src, title.as_deref());
            }
            RunKind::Break => self.line_break(&run.marks),
        }
    }

    /// Close and open markers so that exactly `marks` are in effect, putting
    /// any pending space between the two.
    fn transition(&mut self, marks: &Marks, em: char) {
        let want = wanted(marks, em);
        let mut keep = self.open.iter().take_while(|m| want.contains(m)).count();
        // A code span has to close before anything else can open.
        let opens_more = want
            .iter()
            .any(|m| !m.is_code() && !self.open[..keep].contains(m));
        if opens_more && keep > 0 && self.open[keep - 1].is_code() {
            keep -= 1;
        }
        self.close_to(keep);
        if mem::take(&mut self.pending_space) && !self.line_start {
            self.write_raw(" ");
        }
        for mark in want {
            if !self.open.contains(&mark) {
                self.open_mark(mark);
            }
        }
    }

    fn open_mark(&mut self, mark: Mark) {
        match &mark {
            Mark::Strong => self.delimiter("**", true),
            Mark::Em(delimiter) => self.delimiter(if *delimiter == '_' { "_" } else { "*" }, true),
            Mark::Strike => self.delimiter("~~", true),
            Mark::Link(_) => {
                // `![` would make the link an image.
                if ends_with_unescaped(&self.out, '!') {
                    self.out.insert(self.out.len() - 1, '\\');
                }
                self.link = Some(OpenLink {
                    start: self.out.len(),
                    text: String::new(),
                    plain: true,
                });
                self.out.push('[');
            }
            Mark::Code(_) => match self.last_span.take() {
                Some((start, content))
                    if self.out.len() == start + code_span_len(&content, self.mode) =>
                {
                    self.out.truncate(start);
                    self.code = content;
                }
                _ => self.code.clear(),
            },
        }
        if !matches!(mark, Mark::Link(_)) {
            if let Some(link) = &mut self.link {
                link.plain = false;
            }
        }
        self.line_start = false;
        self.open.push(mark);
    }

    fn close_to(&mut self, keep: usize) {
        while self.open.len() > keep {
            match self.open.pop() {
                Some(Mark::Strong) => self.delimiter("**", false),
                Some(Mark::Em(delimiter)) => {
                    self.delimiter(if delimiter == '_' { "_" } else { "*" }, false)
                }
                Some(Mark::Strike) => self.delimiter("~~", false),
                Some(Mark::Code(_)) => {
                    let span = code_span(&self.code, self.mode == Mode::Cell);
                    self.last_span = Some((self.out.len(), mem::take(&mut self.code)));
                    self.out.push_str(&span);
                }
                Some(Mark::Link(link)) => self.close_link(&link),
                None => {}
            }
        }
    }

    /// Close any open strong, emphasis or strikethrough not flagged to stay
    /// open, along with whatever was opened inside it.
    fn close_unless(&mut self, strong: bool, em: bool, strike: bool) {
        let first = self.open.iter().position(|mark| match mark {
            Mark::Strong => !strong,
            Mark::Em(_) => !em,
            Mark::Strike => !strike,
            _ => false,
        });
        if let Some(first) = first {
            self.close_to(first);
        }
    }

    fn delimiter(&mut self, marker: &str, opens: bool) {
        self.delimiters.push(Delimiter {
            at: self.out.len(),
            len: marker.len(),
            opens,
        });
        self.out.push_str(marker);
    }

    fn close_link(&mut self, link: &Link) {
        let Some(open) = self.link.take() else {
            return;
        };
        let cell = self.mode == Mode::Cell;
        if open.plain && open.text == link.url && is_autolink(&link.url, cell) {
            // `[https://x.org](https://x.org)` says the same as `<https://x.org>`.
            self.out.truncate(open.start);
            self.out.push('<');
            self.out.push_str(&link.url);
            self.out.push('>');
            return;
        }
        self.out.push_str("](");
        self.out.push_str(&destination(&link.url, cell));
        if let Some(title) = &link.title {
            if *title != open.text {
                push_title(&mut self.out, title, cell);
            }
        }
        self.out.push(')');
    }

    /// Text that needs no escaping: a space, or anything inside a code span.
    fn write_raw(&mut self, text: &str) {
        if let Some(link) = &mut self.link {
            link.text.push_str(text);
        }
        if self.open.last().is_some_and(Mark::is_code) {
            self.code.push_str(text);
        } else {
            self.out.push_str(text);
        }
    }

    fn write_text(&mut self, text: &str) {
        if self.open.last().is_some_and(Mark::is_code) {
            self.write_raw(text);
            return;
        }
        if let Some(link) = &mut self.link {
            link.text.push_str(text);
        }
        let line_start = self.line_start && self.mode == Mode::Block;
        escape(
            &mut self.out,
            text,
            self.link.is_some(),
            self.mode,
            line_start,
        );
        self.line_start = false;
    }

    fn write_image(&mut self, alt: &str, src: &str, title: Option<&str>) {
        if let Some(link) = &mut self.link {
            link.plain = false;
        }
        let cell = self.mode == Mode::Cell;
        self.out.push_str("![");
        escape(&mut self.out, alt, true, self.mode, false);
        self.out.push_str("](");
        self.out.push_str(&destination(src, cell));
        if let Some(title) = title {
            push_title(&mut self.out, title, cell);
        }
        self.out.push(')');
        self.line_start = false;
    }

    fn line_break(&mut self, marks: &Marks) {
        // Close whatever the text after the break does not share (a code
        // span always), but never open a marker at the end of a line.
        // Emphasis compares equal whatever its delimiter.
        let want = wanted(marks, '*');
        let keep = self.open.iter().take_while(|m| want.contains(m)).count();
        self.close_to(keep);
        self.pending_space = false;
        if let Some(link) = &mut self.link {
            link.plain = false;
        }
        match self.mode {
            Mode::Block => {
                self.out.push_str("  \n");
                self.line_start = true;
            }
            Mode::Cell => self.out.push_str("<br>"),
            Mode::Heading => self.pending_space = true,
        }
    }
}

/// Whether `text` ends with `c` not already escaped by a backslash.
fn ends_with_unescaped(text: &str, c: char) -> bool {
    let Some(rest) = text.strip_suffix(c) else {
        return false;
    };
    let backslashes = rest.len() - rest.trim_end_matches('\\').len();
    backslashes % 2 == 0
}

fn is_blank(c: Option<char>) -> bool {
    c.is_some_and(char::is_whitespace)
}

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(char::is_alphanumeric)
}

/// Write `text` with a backslash before each character that Markdown would
/// otherwise read as syntax. Neighbours outside `text` are unknown, so
/// characters at its edges are escaped whenever they could matter.
fn escape(out: &mut String, text: &str, in_link: bool, mode: Mode, line_start: bool) {
    let chars: Vec<char> = text.chars().collect();
    let at_start = if line_start {
        line_start_escape(&chars)
    } else {
        None
    };
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let escaped = at_start == Some(i)
            || match c {
                '\\' => next.is_none_or(|n| n.is_ascii_punctuation()),
                '`' => true,
                // Between two spaces these cannot open or close anything.
                '*' | '~' => !(is_blank(prev) && is_blank(next)),
                // Nor can `_` inside a word, as in snake_case.
                '_' => !(is_word(prev) && is_word(next)) && !(is_blank(prev) && is_blank(next)),
                '[' => in_link,
                // A `]` only matters when it could close a link's text.
                ']' => in_link || matches!(next, None | Some('(' | '[' | ':')),
                '<' => {
                    next.is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?'))
                }
                '&' => entity_like(&chars[i + 1..]),
                '|' => mode == Mode::Cell,
                // GitHub and others read `$...$` as inline math. A dollar sign
                // followed by a digit or space never opens it, so prices stay
                // as they are.
                '$' => !next.is_some_and(|n| n.is_ascii_digit() || n.is_whitespace()),
                _ => false,
            };
        if escaped {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Which character to escape so that a line beginning with `chars` is not
/// read as a heading, blockquote, list item, thematic break or setext
/// underline.
fn line_start_escape(chars: &[char]) -> Option<usize> {
    let blank_at = |i: usize| chars.get(i).is_none_or(|c| *c == ' ' || *c == '\t');
    match *chars.first()? {
        '#' => {
            let hashes = chars.iter().take_while(|&&c| c == '#').count();
            (hashes <= 6 && blank_at(hashes)).then_some(0)
        }
        '>' | '=' => Some(0),
        '-' => (blank_at(1) || chars.get(1) == Some(&'-')).then_some(0),
        '+' => blank_at(1).then_some(0),
        '0'..='9' => {
            let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
            (digits <= 9 && matches!(chars.get(digits), Some('.' | ')')) && blank_at(digits + 1))
                .then_some(digits)
        }
        _ => None,
    }
}

/// Whether `&` followed by `rest` would be read as a character reference.
fn entity_like(rest: &[char]) -> bool {
    if rest.first() == Some(&'#') {
        let hex = matches!(rest.get(1), Some('x' | 'X'));
        let from = if hex { 2 } else { 1 };
        let digits = rest[from.min(rest.len())..]
            .iter()
            .take_while(|c| {
                if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                }
            })
            .count();
        digits > 0 && rest.get(from + digits) == Some(&';')
    } else {
        let name = rest
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        name > 0 && rest[0].is_ascii_alphabetic() && rest.get(name) == Some(&';')
    }
}

fn longest_run(text: &str, of: char) -> usize {
    let (mut longest, mut current) = (0, 0);
    for c in text.chars() {
        if c == of {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

fn code_span_len(content: &str, mode: Mode) -> usize {
    code_span(content, mode == Mode::Cell).len()
}

fn code_span(content: &str, cell: bool) -> String {
    // GitHub reads `\|` in a table cell as a pipe even inside a code span.
    let content: Cow<str> = if cell && content.contains('|') {
        content.replace('|', "\\|").into()
    } else {
        content.into()
    };
    let fence = "`".repeat(longest_run(&content, '`') + 1);
    // A backtick at either end would merge with the fence, and a space at
    // both ends would be stripped by the reader.
    let pad = content.starts_with('`')
        || content.ends_with('`')
        || (content.starts_with(' ') && content.ends_with(' ') && content.trim() != "");
    if pad {
        format!("{fence} {content} {fence}")
    } else {
        format!("{fence}{content}{fence}")
    }
}

/// A link destination: parentheses are escaped unless balanced, and spaces,
/// control characters, angle brackets and backslashes are percent-encoded.
fn destination(url: &str, cell: bool) -> String {
    let mut depth = 0i32;
    let mut balanced = true;
    for c in url.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    balanced = false;
                    break;
                }
            }
            _ => {}
        }
    }
    balanced &= depth == 0;

    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            '(' | ')' if !balanced => {
                out.push('\\');
                out.push(c);
            }
            '|' if cell => out.push_str("%7C"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\\' => out.push_str("%5C"),
            c if c.is_whitespace() || c.is_control() => {
                let mut buf = [0; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    let _ = write!(out, "%{byte:02X}");
                }
            }
            c => out.push(c),
        }
    }
    if out.is_empty() {
        out.push_str("<>");
    }
    out
}

fn push_title(out: &mut String, title: &str, cell: bool) {
    out.push_str(" \"");
    for c in title.chars() {
        if c == '"' || c == '\\' || (cell && c == '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
}

/// Whether `url` can be written as `<url>`: CommonMark wants a scheme of two
/// to 32 characters and no spaces or angle brackets. URLs with `%` escapes
/// are left as links too, because some readers decode them in an autolink's
/// text, which would then no longer match the page.
fn is_autolink(url: &str, cell: bool) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    (2..=32).contains(&scheme.len())
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        && !url.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '<' | '>' | '%')
                || (cell && c == '|')
        })
}

// ---------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------

/// Something that prefixes every line written inside it.
enum Kind {
    Quote,
    /// A list item and its marker, such as `- ` or `12. `.
    Item(String),
    /// A list written directly inside another list, without a list item.
    Indent(usize),
}

struct Container {
    kind: Kind,
    /// Whether a line has been written inside it yet.
    started: bool,
}

/// What separates the next block from the previous one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sep {
    Blank,
    /// Only a line break: between list items, and before a list nested in a
    /// list item's text, so that lists stay tight.
    Newline,
}

struct ListState {
    ordered: bool,
    next: i64,
    step: i64,
    /// Whether one of this list's items is open right now.
    item_open: bool,
    /// How many items have been written.
    written: usize,
}

/// Contexts in which block-level elements can't start a block of their own,
/// because Markdown only allows inline content there.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Heading,
    Link,
    Cell,
    Caption,
}

enum TableMode {
    /// Written as rows of blocks: tables used for layout, and tables inside
    /// a heading, link or another table's cell.
    Blocks,
    Grid(Box<Table>),
}

struct Converter<'a> {
    base_url: Option<&'a str>,
    em_style: EmStyle,
    em_used: EmUse,
    out: String,
    containers: Vec<Container>,
    sep: Sep,
    /// The paragraph being collected.
    para: Vec<Run>,
    /// Whether a space may be added to the paragraph, i.e. it is non-empty and
    /// doesn't already end in a space or line break.
    space_ok: bool,
    /// The level of the heading being collected, or 0 for a paragraph.
    heading: u8,
    strong: u32,
    em: u32,
    strike: u32,
    /// How many code elements are open, and the number of the outermost.
    code: u32,
    code_id: u32,
    link: Option<Rc<Link>>,
    /// The list that just ended, as (container depth, ordered, next number),
    /// until something else is written at that depth.
    last_list: Option<(usize, bool, i64)>,
    /// Whether the last block was a code block or table, which always gets a
    /// blank line after it.
    spacious: bool,
    /// Whether the open link has had any whitespace in it.
    link_space: bool,
    inline: Vec<Ctx>,
    lists: Vec<ListState>,
    tables: Vec<TableMode>,
}

impl<'a> Converter<'a> {
    fn new(base_url: Option<&'a str>, em_style: EmStyle) -> Self {
        Converter {
            base_url,
            em_style,
            em_used: EmUse::default(),
            out: String::new(),
            containers: Vec::new(),
            sep: Sep::Blank,
            para: Vec::new(),
            space_ok: false,
            heading: 0,
            strong: 0,
            em: 0,
            strike: 0,
            code: 0,
            code_id: 0,
            link: None,
            last_list: None,
            spacious: false,
            link_space: false,
            inline: Vec::new(),
            lists: Vec::new(),
            tables: Vec::new(),
        }
    }

    /// Start converting `node`. Returns what to do after its children, or
    /// `None` when they are to be skipped.
    fn enter(&mut self, node: &Handle) -> Option<Leave> {
        let (name, attrs) = match &node.data {
            NodeData::Document => return Some(Leave::Nothing),
            NodeData::Text { contents } => {
                self.text(&contents.borrow());
                return None;
            }
            NodeData::Element { name, attrs, .. } => (name, attrs),
            _ => return None,
        };
        // SVG and MathML have no Markdown form.
        if &*name.ns != HTML_NS {
            return None;
        }
        let attrs = attrs.borrow();
        if is_hidden(&attrs) {
            return None;
        }
        if is_skipped(&name.local, &attrs) {
            return None;
        }
        let leave = match &*name.local {
            "br" => {
                self.line_break();
                return None;
            }
            "hr" => {
                self.rule();
                return None;
            }
            "img" => {
                self.image(&attrs);
                return None;
            }
            "pre" | "listing" | "xmp" | "plaintext" => {
                self.pre(node, &attrs);
                return None;
            }
            "h1" => self.heading(1),
            "h2" => self.heading(2),
            "h3" => self.heading(3),
            "h4" => self.heading(4),
            "h5" => self.heading(5),
            "h6" => self.heading(6),
            "blockquote" => self.blockquote(),
            "ul" | "menu" | "dir" => self.list(node, &attrs, false),
            "ol" => self.list(node, &attrs, true),
            "li" => self.item(&attrs),
            "dt" => self.term(),
            "a" => self.link(&attrs),
            "strong" | "b" => {
                self.strong += 1;
                Leave::Strong
            }
            "em" | "i" | "cite" | "dfn" | "var" => {
                self.em += 1;
                Leave::Em
            }
            "s" | "strike" | "del" => {
                self.strike += 1;
                Leave::Strike
            }
            "code" | "kbd" | "samp" | "tt" => {
                self.open_code();
                Leave::Code
            }
            "q" => {
                self.text("\"");
                Leave::Quotation
            }
            "rt" => {
                self.text("(");
                Leave::RubyText
            }
            "table" => self.table(node, &attrs),
            "caption" => self.caption(),
            "thead" => self.section(Section::Head),
            "tbody" => self.section(Section::Body),
            "tfoot" => self.section(Section::Foot),
            "tr" => self.row(),
            "td" => self.cell(&attrs, false),
            "th" => self.cell(&attrs, true),
            "p" | "div" | "address" | "article" | "aside" | "center" | "details" | "summary"
            | "fieldset" | "legend" | "figure" | "figcaption" | "footer" | "header" | "hgroup"
            | "main" | "nav" | "section" | "search" | "form" | "dl" | "dd" | "html" | "body"
            | "dialog" => {
                self.boundary();
                Leave::Block
            }
            _ => Leave::Nothing,
        };
        Some(leave)
    }

    fn leave(&mut self, leave: Leave) {
        match leave {
            Leave::Nothing => {}
            Leave::Block => self.boundary(),
            Leave::Heading => {
                self.inline.pop();
                self.finish_paragraph();
            }
            Leave::Quote(pushed) => {
                self.finish_paragraph();
                if pushed {
                    self.containers.pop();
                    self.forget_inner_list();
                }
            }
            Leave::List(pushed) => {
                self.finish_paragraph();
                let list = self.lists.pop();
                if pushed {
                    self.containers.pop();
                }
                self.last_list = list.map(|l| (self.containers.len(), l.ordered, l.next));
            }
            Leave::Item(pushed) => {
                self.finish_paragraph();
                let written = !pushed || self.containers.pop().is_some_and(|c| c.started);
                self.forget_inner_list();
                if let Some(list) = self.lists.last_mut() {
                    list.item_open = false;
                    // An empty item is left out, along with its number.
                    if written {
                        list.written += 1;
                        list.next = list.next.saturating_add(list.step);
                    }
                }
            }
            Leave::Term => {
                self.finish_paragraph();
                self.strong -= 1;
            }
            Leave::Link(link, label) => {
                let has_content = self
                    .para
                    .iter()
                    .rev()
                    .take_while(|run| {
                        run.marks
                            .link
                            .as_ref()
                            .is_some_and(|l| Rc::ptr_eq(l, &link))
                    })
                    .any(Run::is_content);
                // An icon-only link still says where it goes. Whitespace in it
                // was a space beside the icon, so it follows the label.
                if !has_content {
                    if let Some(label) = label {
                        let spaced = self.link_space;
                        self.text(&label);
                        if spaced {
                            self.text(" ");
                        }
                    }
                }
                self.inline.pop();
                self.link = None;
            }
            Leave::Strong => self.strong -= 1,
            Leave::Em => self.em -= 1,
            Leave::Strike => self.strike -= 1,
            Leave::Code => self.code -= 1,
            Leave::Quotation => self.text("\""),
            Leave::RubyText => self.text(")"),
            Leave::Table => match self.tables.pop() {
                Some(TableMode::Grid(table)) => {
                    if let Some(caption) = &table.caption {
                        self.write_block(caption);
                    }
                    if let Some(table) = table.render() {
                        self.write_block(&table);
                    }
                }
                _ => self.boundary(),
            },
            Leave::Caption => {
                self.inline.pop();
                let text = self.take_inline(Mode::Block);
                if let Some(TableMode::Grid(table)) = self.tables.last_mut() {
                    table.capturing = false;
                    table.caption = Some(text);
                }
            }
            Leave::Section(previous) => {
                if let Some(TableMode::Grid(table)) = self.tables.last_mut() {
                    table.section = previous;
                }
            }
            Leave::Cell => {
                self.inline.pop();
                let text = self.take_inline(Mode::Cell);
                if let Some(TableMode::Grid(table)) = self.tables.last_mut() {
                    table.capturing = false;
                    if let Some(cell) = table.cell.take() {
                        if table.rows.is_empty() {
                            table.rows.push(Row::new(table.section));
                        }
                        if let Some(row) = table.rows.last_mut() {
                            row.cells.push(Cell { text, ..cell });
                        }
                    }
                }
            }
        }
    }

    fn finish(mut self) -> String {
        self.finish_paragraph();
        let end = self.out.trim_end_matches('\n').len();
        self.out.truncate(end);
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        self.out
    }

    fn resolve(&self, url: &str) -> String {
        // Browsers ignore tabs and newlines anywhere in a URL.
        let url: String = url
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
            .collect();
        match self.base_url {
            Some(base) => html2text::resolve_url(base, &url),
            None => url,
        }
    }

    // --- Collecting inline content -------------------------------------

    fn open_code(&mut self) {
        if self.code == 0 {
            self.code_id += 1;
        }
        self.code += 1;
    }

    fn marks(&self) -> Marks {
        Marks {
            strong: self.strong > 0,
            em: self.em > 0,
            strike: self.strike > 0,
            code: if self.code > 0 { self.code_id } else { 0 },
            link: self.link.clone(),
        }
    }

    /// Text between a table's cells is only whitespace, and is dropped.
    fn ignoring_text(&self) -> bool {
        matches!(self.tables.last(), Some(TableMode::Grid(table)) if !table.capturing)
    }

    fn text(&mut self, text: &str) {
        if self.ignoring_text() {
            return;
        }
        if self.link.is_some() && text.chars().any(collapsible) {
            self.link_space = true;
        }
        // Collapse whitespace as a browser does; a no-break space counts too,
        // since Markdown has no use for it.
        let mut collapsed = String::with_capacity(text.len());
        for c in text.chars() {
            if collapsible(c) {
                if self.space_ok {
                    collapsed.push(' ');
                    self.space_ok = false;
                }
            } else {
                collapsed.push(c);
                self.space_ok = true;
            }
        }
        if collapsed.is_empty() {
            return;
        }
        let marks = self.marks();
        if let Some(Run {
            kind: RunKind::Text(last),
            marks: last_marks,
        }) = self.para.last_mut()
        {
            if *last_marks == marks {
                last.push_str(&collapsed);
                return;
            }
        }
        self.para.push(Run {
            kind: RunKind::Text(collapsed),
            marks,
        });
    }

    fn trim_trailing_space(&mut self) {
        if let Some(Run {
            kind: RunKind::Text(text),
            ..
        }) = self.para.last_mut()
        {
            if text.ends_with(' ') {
                text.pop();
                if text.is_empty() {
                    self.para.pop();
                }
            }
        }
    }

    fn push_break(&mut self) {
        let marks = Marks {
            code: 0,
            ..self.marks()
        };
        self.para.push(Run {
            kind: RunKind::Break,
            marks,
        });
    }

    /// `<br>`: a hard line break, and two in a row end the paragraph.
    fn line_break(&mut self) {
        if self.ignoring_text() {
            return;
        }
        if !self.inline.is_empty() {
            self.soft_break();
            return;
        }
        self.trim_trailing_space();
        match self.para.last().map(|run| &run.kind) {
            None => {}
            Some(RunKind::Break) => {
                self.para.pop();
                self.finish_paragraph();
            }
            Some(_) => self.push_break(),
        }
        self.space_ok = false;
    }

    /// Where Markdown allows only inline content, a block boundary becomes a
    /// line break in a table cell and a space anywhere else.
    fn soft_break(&mut self) {
        if self.link.is_some() {
            self.link_space = true;
        }
        if self.inline.last() == Some(&Ctx::Cell) {
            self.trim_trailing_space();
            if self
                .para
                .last()
                .is_some_and(|run| !matches!(run.kind, RunKind::Break))
            {
                self.push_break();
            }
            self.space_ok = false;
        } else if self.space_ok {
            self.text(" ");
        }
    }

    fn image(&mut self, attrs: &[Attribute]) {
        if self.ignoring_text() {
            return;
        }
        let Some(src) = image_source(attrs) else {
            return;
        };
        let src = self.resolve(&src);
        let alt = attr(attrs, "alt").map(collapse).unwrap_or_default();
        let title = attr(attrs, "title")
            .map(collapse)
            .filter(|t| !t.is_empty() && *t != alt);
        let marks = Marks {
            code: 0,
            ..self.marks()
        };
        self.para.push(Run {
            kind: RunKind::Image { alt, src, title },
            marks,
        });
        self.space_ok = true;
    }

    fn link(&mut self, attrs: &[Attribute]) -> Leave {
        // Links can't nest; an inner one keeps only its text.
        if self.link.is_some() {
            return Leave::Nothing;
        }
        let Some(href) = attr(attrs, "href") else {
            return Leave::Nothing;
        };
        let href = href.trim_matches(|c: char| c.is_ascii_whitespace());
        let scripted = href
            .get(..11)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("javascript:"));
        if href.is_empty() || href == "#" || scripted {
            return Leave::Nothing;
        }
        let url = self.resolve(href);
        let title = attr(attrs, "title").map(collapse).filter(|t| !t.is_empty());
        let label = attr(attrs, "aria-label")
            .map(collapse)
            .filter(|t| !t.is_empty())
            .or_else(|| title.clone());
        let link = Rc::new(Link { url, title });
        self.link = Some(link.clone());
        self.link_space = false;
        self.inline.push(Ctx::Link);
        Leave::Link(link, label)
    }

    // --- Writing blocks --------------------------------------------------

    /// A block-level element starts or ends here.
    fn boundary(&mut self) {
        if self.inline.is_empty() {
            self.finish_paragraph();
        } else {
            self.soft_break();
        }
    }

    /// Write out the paragraph (or heading) collected so far.
    fn finish_paragraph(&mut self) {
        let level = mem::take(&mut self.heading);
        self.space_ok = false;
        if !self.para.iter().any(Run::is_content) {
            self.para.clear();
            return;
        }
        let runs = mem::take(&mut self.para);
        let text = if level > 0 {
            heading_line(
                level,
                &serialize(&runs, Mode::Heading, self.em_style, &mut self.em_used),
            )
        } else {
            serialize(&runs, Mode::Block, self.em_style, &mut self.em_used)
        };
        self.write_block(&text);
    }

    /// Serialize and clear the inline content collected for a cell or caption.
    fn take_inline(&mut self, mode: Mode) -> String {
        let runs = mem::take(&mut self.para);
        self.space_ok = false;
        serialize(&runs, mode, self.em_style, &mut self.em_used)
    }

    fn write_block(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self
            .last_list
            .is_some_and(|(depth, ..)| self.containers.len() <= depth)
        {
            self.last_list = None;
        }
        // A code block or table gets a blank line on both sides, even
        // between the items of a tight list.
        let spacious = text.starts_with("```") || text.starts_with('|');
        if spacious || self.spacious {
            self.sep = Sep::Blank;
        }
        self.spacious = spacious;
        self.write_separator();
        for line in text.split('\n') {
            if line.is_empty() {
                let prefix = self.blank_prefix();
                self.out.push_str(&prefix);
            } else {
                self.push_line_prefix();
                self.out.push_str(line);
            }
            self.out.push('\n');
        }
        self.sep = Sep::Blank;
    }

    fn write_separator(&mut self) {
        let sep = mem::replace(&mut self.sep, Sep::Blank);
        if self.out.is_empty() {
            return;
        }
        // A single line break is only safe before a list marker: before
        // anything else it would join the previous paragraph.
        let marker_next = self
            .containers
            .iter()
            .any(|c| !c.started && matches!(c.kind, Kind::Item(_)));
        if sep == Sep::Blank || !marker_next {
            let prefix = self.blank_prefix();
            self.out.push_str(&prefix);
            self.out.push('\n');
        }
    }

    fn push_line_prefix(&mut self) {
        for container in &mut self.containers {
            match &container.kind {
                Kind::Quote => self.out.push_str("> "),
                Kind::Item(marker) if !container.started => self.out.push_str(marker),
                Kind::Item(marker) => push_spaces(&mut self.out, marker.len()),
                Kind::Indent(width) => push_spaces(&mut self.out, *width),
            }
            container.started = true;
        }
    }

    /// The prefix of an empty line: only the containers already started, so
    /// that a blank line before a blockquote isn't part of it, and without
    /// trailing spaces.
    fn blank_prefix(&self) -> String {
        let mut prefix = String::new();
        for container in self.containers.iter().filter(|c| c.started) {
            match &container.kind {
                Kind::Quote => prefix.push_str("> "),
                Kind::Item(marker) => push_spaces(&mut prefix, marker.len()),
                Kind::Indent(width) => push_spaces(&mut prefix, *width),
            }
        }
        prefix.truncate(prefix.trim_end().len());
        prefix
    }

    /// A list that ended inside a container that has now closed is no longer
    /// next to anything.
    fn forget_inner_list(&mut self) {
        if self
            .last_list
            .is_some_and(|(depth, ..)| depth > self.containers.len())
        {
            self.last_list = None;
        }
    }

    fn push_container(&mut self, kind: Kind) -> bool {
        if self.containers.len() >= MAX_DEPTH {
            return false;
        }
        self.containers.push(Container {
            kind,
            started: false,
        });
        true
    }

    // --- Block elements ------------------------------------------------

    fn heading(&mut self, level: u8) -> Leave {
        if !self.inline.is_empty() {
            self.soft_break();
            return Leave::Block;
        }
        self.finish_paragraph();
        self.heading = level;
        self.inline.push(Ctx::Heading);
        Leave::Heading
    }

    fn blockquote(&mut self) -> Leave {
        if !self.inline.is_empty() {
            self.soft_break();
            return Leave::Block;
        }
        self.finish_paragraph();
        Leave::Quote(self.push_container(Kind::Quote))
    }

    fn list(&mut self, node: &Handle, attrs: &[Attribute], ordered: bool) -> Leave {
        if !self.inline.is_empty() {
            self.soft_break();
            return Leave::Block;
        }
        self.finish_paragraph();
        let level = self.containers.len();
        let reversed = ordered && attr(attrs, "reversed").is_some();
        let start = attr(attrs, "start")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or_else(|| if reversed { count_items(node) } else { 1 });
        // Two lists of the same kind with nothing between them would read as
        // one. CommonMark's own way to keep them apart is an empty HTML
        // comment, which leaves every bullet the same. A numbered list that
        // carries on where the last one stopped can simply join it.
        let mut separated = false;
        if let Some((depth, kind, next)) = self.last_list.take() {
            if depth == level && kind == ordered && !(ordered && start == next) {
                self.write_block("<!-- -->");
                separated = true;
            }
        }
        // A list right under a line of text follows it on the next line, but
        // only one that starts at 1 may: `10.` there would read as text.
        let tight = !separated && (!ordered || start == 1);
        let mut indented = false;
        let indented_already = matches!(
            self.containers.last(),
            Some(Container {
                kind: Kind::Indent(_),
                ..
            })
        );
        if let Some(parent) = self.lists.last() {
            // Only one level: a second would reach the four spaces of
            // indentation that make a code block.
            if !parent.item_open && !indented_already {
                // Directly inside another list, without a list item around
                // it: indented as though it were in the previous item.
                let width = if parent.ordered { 3 } else { 2 };
                indented = self.push_container(Kind::Indent(width));
                if tight {
                    self.sep = Sep::Newline;
                }
            }
        }
        if tight
            && self
                .containers
                .last()
                .is_some_and(|c| c.started && matches!(c.kind, Kind::Item(_)))
        {
            self.sep = Sep::Newline;
        }
        self.lists.push(ListState {
            ordered,
            next: start,
            step: if reversed { -1 } else { 1 },
            item_open: false,
            written: 0,
        });
        Leave::List(indented)
    }

    fn item(&mut self, attrs: &[Attribute]) -> Leave {
        if !self.inline.is_empty() {
            self.soft_break();
            return Leave::Block;
        }
        self.finish_paragraph();
        let marker = match self.lists.last_mut() {
            Some(list) => {
                if list.ordered {
                    if let Some(value) = attr(attrs, "value").and_then(|v| v.trim().parse().ok()) {
                        list.next = value;
                    }
                }
                if list.written > 0 {
                    self.sep = Sep::Newline;
                }
                list.item_open = true;
                if list.ordered {
                    // CommonMark allows at most nine digits.
                    format!("{}. ", list.next.clamp(0, 999_999_999))
                } else {
                    "- ".to_owned()
                }
            }
            None => "- ".to_owned(),
        };
        Leave::Item(self.push_container(Kind::Item(marker)))
    }

    /// `<dt>`: the term is written in bold, the `<dd>` after it as a paragraph.
    fn term(&mut self) -> Leave {
        if !self.inline.is_empty() {
            self.soft_break();
            return Leave::Block;
        }
        self.finish_paragraph();
        self.strong += 1;
        Leave::Term
    }

    fn rule(&mut self) {
        if !self.inline.is_empty() {
            self.soft_break();
            return;
        }
        self.finish_paragraph();
        // Not `---`: as a list item's first line, `- ---` reads as a rule
        // rather than an item, and at the top of a document `---` starts
        // front matter for Hugo, Jekyll and others.
        self.write_block("***");
    }

    fn pre(&mut self, node: &Handle, attrs: &[Attribute]) {
        let mut code = String::new();
        raw_text(node, &mut code);
        if !self.inline.is_empty() {
            self.soft_break();
            self.open_code();
            self.text(&code);
            self.code -= 1;
            self.soft_break();
            return;
        }
        self.finish_paragraph();
        let code = tidy_code(&code);
        if code.is_empty() {
            return;
        }
        let fence = "`".repeat(longest_run(&code, '`').max(2) + 1);
        let language = code_language(node, attrs);
        self.write_block(&format!("{fence}{language}\n{code}\n{fence}"));
    }

    fn table(&mut self, node: &Handle, attrs: &[Attribute]) -> Leave {
        if !self.inline.is_empty() || is_layout_table(node, attrs) {
            self.boundary();
            self.tables.push(TableMode::Blocks);
        } else {
            self.finish_paragraph();
            self.tables.push(TableMode::Grid(Box::default()));
        }
        Leave::Table
    }

    fn caption(&mut self) -> Leave {
        match self.tables.last_mut() {
            Some(TableMode::Grid(table)) => {
                table.capturing = true;
                self.space_ok = false;
                self.inline.push(Ctx::Caption);
                Leave::Caption
            }
            _ => {
                self.boundary();
                Leave::Block
            }
        }
    }

    fn section(&mut self, section: Section) -> Leave {
        match self.tables.last_mut() {
            Some(TableMode::Grid(table)) => {
                Leave::Section(mem::replace(&mut table.section, section))
            }
            _ => {
                self.boundary();
                Leave::Block
            }
        }
    }

    fn row(&mut self) -> Leave {
        match self.tables.last_mut() {
            Some(TableMode::Grid(table)) => {
                table.rows.push(Row::new(table.section));
                Leave::Nothing
            }
            _ => {
                self.boundary();
                Leave::Block
            }
        }
    }

    fn cell(&mut self, attrs: &[Attribute], header: bool) -> Leave {
        let Some(TableMode::Grid(table)) = self.tables.last_mut() else {
            self.boundary();
            return Leave::Block;
        };
        let span = |name: &str, max: usize| {
            attr(attrs, name)
                .and_then(|v| v.trim().parse::<usize>().ok())
                .map_or(1, |n| n.clamp(1, max))
        };
        table.cell = Some(Cell {
            text: String::new(),
            colspan: span("colspan", MAX_COLSPAN),
            rowspan: span("rowspan", MAX_ROWSPAN),
            header,
            align: cell_align(attrs),
        });
        table.capturing = true;
        self.space_ok = false;
        self.inline.push(Ctx::Cell);
        Leave::Cell
    }
}

/// Whitespace that HTML collapses, plus the no-break space, which Markdown
/// has no use for.
fn collapsible(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\u{a0}')
}

fn push_spaces(out: &mut String, count: usize) {
    out.extend(std::iter::repeat_n(' ', count));
}

/// `# Heading`, escaping a final run of `#` that would read as a closing
/// sequence and be dropped.
fn heading_line(level: u8, text: &str) -> String {
    let mut text = text.to_owned();
    let without = text.trim_end_matches('#').len();
    if without < text.len() && (without == 0 || text[..without].ends_with(' ')) {
        text.insert(without, '\\');
    }
    format!("{} {text}", "#".repeat(level.into()))
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Section {
    Head,
    #[default]
    Body,
    Foot,
}

#[derive(Clone, Copy, Default)]
enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

struct Cell {
    text: String,
    colspan: usize,
    rowspan: usize,
    header: bool,
    align: Align,
}

struct Row {
    section: Section,
    cells: Vec<Cell>,
}

impl Row {
    fn new(section: Section) -> Self {
        Row {
            section,
            cells: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Table {
    rows: Vec<Row>,
    section: Section,
    caption: Option<String>,
    /// The cell being collected.
    cell: Option<Cell>,
    /// Whether text belongs to a cell or caption right now.
    capturing: bool,
}

impl Table {
    fn render(mut self) -> Option<String> {
        // Rows in display order: the head first and the foot last.
        self.rows.sort_by_key(|row| match row.section {
            Section::Head => 0,
            Section::Body => 1,
            Section::Foot => 2,
        });
        let rows = self.rows;
        if rows
            .iter()
            .all(|row| row.cells.iter().all(|cell| cell.text.is_empty()))
        {
            return None;
        }
        // GitHub tables must start with a header row. When the HTML has none,
        // an empty one is written rather than promoting the first data row.
        let has_header = rows.first().is_some_and(|row| {
            !row.cells.is_empty()
                && (row.section == Section::Head || row.cells.iter().all(|cell| cell.header))
        });
        let mut grid = expand(&rows).unwrap_or_else(|| {
            rows.iter()
                .map(|row| row.cells.iter().map(|cell| cell.text.clone()).collect())
                .collect()
        });
        let width = grid.iter().map(Vec::len).max().unwrap_or(0);
        if width == 0 {
            return None;
        }
        if width.saturating_mul(grid.len()) <= MAX_GRID_CELLS {
            for row in &mut grid {
                row.resize(width, String::new());
            }
        }
        let mut aligns = vec![Align::None; width];
        let mut header = vec![String::new(); width];
        let mut body = grid.into_iter();
        if has_header {
            let mut col = 0;
            for cell in &rows[0].cells {
                for slot in aligns.iter_mut().skip(col).take(cell.colspan) {
                    *slot = cell.align;
                }
                col += cell.colspan;
            }
            for (slot, text) in header.iter_mut().zip(body.next().unwrap_or_default()) {
                *slot = text;
            }
        }

        let mut out = String::new();
        write_row(&mut out, &header);
        out.push('\n');
        for align in &aligns {
            out.push_str(match align {
                Align::None => "| --- ",
                Align::Left => "| :--- ",
                Align::Center => "| :---: ",
                Align::Right => "| ---: ",
            });
        }
        out.push('|');
        for row in body {
            out.push('\n');
            write_row(&mut out, &row);
        }
        Some(out)
    }
}

fn write_row(out: &mut String, cells: &[String]) {
    for cell in cells {
        out.push_str("| ");
        if !cell.is_empty() {
            out.push_str(cell);
            out.push(' ');
        }
    }
    out.push('|');
}

/// Lay the rows out on a grid: a cell spanning several columns or rows is
/// followed by, or has below it, empty cells. `None` when that would exceed
/// `MAX_GRID_CELLS`.
fn expand(rows: &[Row]) -> Option<Vec<Vec<String>>> {
    let mut budget = MAX_GRID_CELLS;
    let mut take = |n: usize| -> Option<()> {
        budget = budget.checked_sub(n)?;
        Some(())
    };
    // For each column, how many more rows a cell above still covers.
    let mut covered: Vec<usize> = Vec::new();
    let mut grid = Vec::with_capacity(rows.len());
    for row in rows {
        let blocked: Vec<bool> = covered.iter().map(|&n| n > 0).collect();
        for n in &mut covered {
            *n = n.saturating_sub(1);
        }
        // Columns no longer covered cost nothing on the rows that follow.
        while covered.last() == Some(&0) {
            covered.pop();
        }
        let mut line = Vec::new();
        let mut col = 0;
        for cell in &row.cells {
            while blocked.get(col).copied().unwrap_or(false) {
                take(1)?;
                line.push(String::new());
                col += 1;
            }
            take(cell.colspan)?;
            line.push(cell.text.clone());
            line.extend(std::iter::repeat_n(String::new(), cell.colspan - 1));
            if cell.rowspan > 1 {
                if covered.len() < col + cell.colspan {
                    covered.resize(col + cell.colspan, 0);
                }
                for n in &mut covered[col..col + cell.colspan] {
                    *n = (*n).max(cell.rowspan - 1);
                }
            }
            col += cell.colspan;
        }
        while blocked.get(col).copied().unwrap_or(false) {
            take(1)?;
            line.push(String::new());
            col += 1;
        }
        grid.push(line);
    }
    Some(grid)
}

fn cell_align(attrs: &[Attribute]) -> Align {
    let value = attr(attrs, "align")
        .map(|v| v.trim().to_ascii_lowercase())
        .or_else(|| {
            let style = attr(attrs, "style")?.to_ascii_lowercase();
            let rest = &style[style.find("text-align")? + "text-align".len()..];
            let value = rest.trim_start().strip_prefix(':')?.trim_start();
            Some(value.split([';', ' ', '!']).next()?.to_owned())
        });
    match value.as_deref() {
        Some("left" | "start") => Align::Left,
        Some("center") => Align::Center,
        Some("right" | "end") => Align::Right,
        _ => Align::None,
    }
}

/// Whether a table is better written as plain blocks than as a Markdown
/// table: one marked as presentational, one with a single column, or one
/// holding another table, which is how pages are laid out with tables.
fn is_layout_table(table: &Handle, attrs: &[Attribute]) -> bool {
    if attr(attrs, "role").is_some_and(|role| {
        let role = role.trim();
        role.eq_ignore_ascii_case("presentation") || role.eq_ignore_ascii_case("none")
    }) {
        return true;
    }
    let mut widest = 0;
    for child in table.children.borrow().iter() {
        match html_name(child) {
            Some("tr") => widest = widest.max(count_cells(child)),
            Some("thead" | "tbody" | "tfoot") => {
                for row in child.children.borrow().iter() {
                    if html_name(row) == Some("tr") {
                        widest = widest.max(count_cells(row));
                    }
                }
            }
            _ => {}
        }
    }
    widest < 2 || contains_table(table)
}

fn count_cells(row: &Handle) -> usize {
    row.children
        .borrow()
        .iter()
        .filter(|cell| matches!(html_name(cell), Some("td" | "th")))
        .count()
}

fn contains_table(table: &Handle) -> bool {
    let mut stack: Vec<Handle> = table.children.borrow().iter().cloned().collect();
    while let Some(node) = stack.pop() {
        if html_name(&node) == Some("table") {
            return true;
        }
        stack.extend(node.children.borrow().iter().cloned());
    }
    false
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn html_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } if &*name.ns == HTML_NS => Some(&name.local),
        _ => None,
    }
}

fn attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|a| a.name.ns.is_empty() && &*a.name.local == name)
        .map(|a| &*a.value)
}

/// Collapse runs of whitespace to single spaces and trim the ends.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Elements with no text a reader sees: metadata, scripts, embedded media,
/// form controls, and a dialog that isn't open.
fn is_skipped(tag: &str, attrs: &[Attribute]) -> bool {
    match tag {
        "head" | "script" | "style" | "template" | "noscript" | "title" | "meta" | "link"
        | "base" | "iframe" | "object" | "embed" | "video" | "audio" | "canvas" | "map"
        | "area" | "input" | "select" | "textarea" | "datalist" | "option" | "optgroup"
        | "source" | "track" | "param" | "frame" | "frameset" | "noframes" | "colgroup" | "col"
        | "rp" => true,
        "dialog" => attr(attrs, "open").is_none(),
        _ => false,
    }
}

fn is_hidden(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| match &*a.name.local {
        "hidden" => true,
        "aria-hidden" => a.value.trim().eq_ignore_ascii_case("true"),
        "style" => {
            let style: String = a
                .value
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            style.contains("display:none") || style.contains("visibility:hidden")
        }
        _ => false,
    })
}

/// The image's URL. Lazy-loading pages put a placeholder (often a `data:`
/// URI) in `src` and the real image in an attribute that a script swaps in.
fn image_source(attrs: &[Attribute]) -> Option<String> {
    fn first_candidate(srcset: &str) -> Option<&str> {
        srcset.split(',').next()?.split_whitespace().next()
    }
    let get = |name: &str| attr(attrs, name).map(str::trim).filter(|v| !v.is_empty());
    let src = get("src");
    if let Some(src) = src.filter(|s| !s.starts_with("data:")) {
        return Some(src.to_owned());
    }
    get("data-src")
        .or_else(|| get("data-lazy-src"))
        .or_else(|| get("data-original"))
        .or_else(|| get("data-srcset").and_then(first_candidate))
        .or_else(|| get("srcset").and_then(first_candidate))
        .or(src)
        .map(str::to_owned)
}

/// The text inside a `<pre>`, with `<br>` as a line break.
fn raw_text(node: &Handle, out: &mut String) {
    let mut stack: Vec<Handle> = node.children.borrow().iter().rev().cloned().collect();
    while let Some(node) = stack.pop() {
        match &node.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Element { name, attrs, .. } => {
                let attrs = attrs.borrow();
                if &*name.ns != HTML_NS || is_skipped(&name.local, &attrs) || is_hidden(&attrs) {
                    continue;
                }
                if &*name.local == "br" {
                    out.push('\n');
                }
                stack.extend(node.children.borrow().iter().rev().cloned());
            }
            _ => {}
        }
    }
}

/// Drop trailing whitespace from each line, and blank lines at either end.
fn tidy_code(code: &str) -> String {
    let lines: Vec<&str> = code.lines().map(str::trim_end).collect();
    let Some(first) = lines.iter().position(|line| !line.is_empty()) else {
        return String::new();
    };
    let last = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .unwrap_or(first);
    lines[first..=last].join("\n")
}

/// The language of a code block, from the `language-*` / `lang-*` class or
/// `data-lang` that highlighters put on the `<pre>` or the `<code>` in it.
fn code_language(pre: &Handle, attrs: &[Attribute]) -> String {
    fn from_attrs(attrs: &[Attribute]) -> Option<String> {
        let class = attr(attrs, "class").unwrap_or("");
        class
            .split_ascii_whitespace()
            .find_map(|token| {
                token
                    .strip_prefix("language-")
                    .or_else(|| token.strip_prefix("lang-"))
            })
            .or_else(|| attr(attrs, "data-lang"))
            .map(|lang| {
                lang.chars()
                    .filter(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '#' | '.' | '-')
                    })
                    .collect()
            })
            .filter(|lang: &String| !lang.is_empty())
    }
    from_attrs(attrs)
        .or_else(|| {
            pre.children
                .borrow()
                .iter()
                .find_map(|child| match &child.data {
                    NodeData::Element { name, attrs, .. } if &*name.local == "code" => {
                        from_attrs(&attrs.borrow())
                    }
                    _ => None,
                })
        })
        .unwrap_or_default()
}

/// `<ol reversed>` without a `start` counts down from its number of items.
fn count_items(list: &Handle) -> i64 {
    let items = list
        .children
        .borrow()
        .iter()
        .filter(|child| html_name(child) == Some("li"))
        .count();
    i64::try_from(items).unwrap_or(i64::MAX)
}
