# Changelog

## Unreleased

- Deeply nested lists, definitions, paragraphs and emphasis are rendered in
  linear time again. In 0.4.0 each level cost time for every level around
  it: 400,000 nested lists took 49 s, and now take 0.5 s. Output is
  unchanged.
- Character widths from Unicode 18 (unicode-width 0.2.3). The halfwidth
  katakana sound marks ﾞ and ﾟ count one column, as terminals show them,
  rather than none, and characters new in Unicode 18 (emoji, kana,
  ideographic characters and combining marks) get their own widths. Other
  text wraps as before.
- html5ever 0.40.1, still with
  [servo/html5ever#800](https://github.com/servo/html5ever/pull/800).
  Output is unchanged. Building from source needs Rust 1.85 or later.

## 0.4.0 (2026-10-22)

- `html2text()` is rendered by this package's own code. The library it used
  before came from a fork that can no longer be fetched, so the source
  distribution and fresh builds of 0.3.1 fail. The new renderer is a
  clean-room implementation: it was written from 0.3.1's output, the HTML
  standard and RFC 3986, without reference to the old library's source.
- Output is the same as 0.3.1's, byte for byte, with every option set: on
  the benchmark corpus, on 38 of 39 other real pages (the last differs at
  width 40 only), and on 1,500 random documents without tables. With tables,
  99.2% of 2,000 random documents match under each option set. The known
  differences are all in unusual markup:
  - A cell with nothing in it, beside a cell spanning rows: 0.3.1 draws the
    span against the wrong cell, so borders and line breaks land a column
    off. They now follow the spanning cell.
  - A table with nothing to show whose cells span into further empty rows,
    an empty `<pre>` or heading directly inside `<pre>`, and a table with
    nothing to show in a paragraph: 0.3.1 writes an extra blank line.
  - Headings, quotes and list items squeezed to fewer than three columns
    break `**` and other markers in different places.
  - Some emoji sequences (joined with U+200D) and Arabic lam-alef pairs, cut
    at the end of a line in a narrow table cell.
- `html2text()` is 1.2–2× faster on real pages (7.0 ms against 10.8 ms for
  a 780 KB Wikipedia article) and 2× faster on short snippets.
- `html2markdown()` output is unchanged.
- PyO3 0.30. Building from source needs maturin 1.9.4 or later.

## 0.3.1 (2026-10-19)

- The vendored html5ever now carries
  [servo/html5ever#800](https://github.com/servo/html5ever/pull/800) by Josef
  Guenther in place of our narrower patch. Every "in scope" check and the
  other searches of the stack of open elements are now O(1), so unmatched
  end tags and end tags inside SVG or MathML no longer make deeply nested
  HTML quadratic: 32,000 nested `<span>` followed by as many unmatched
  `</em>` went from 3.4 s to 0.02 s. Output is unchanged.
- About 0.4 µs more per call, and 0–2% on real pages.

## 0.3.0 (2026-10-15)

- New `html2markdown(html, *, base_url=None)`: HTML to CommonMark with
  GitHub's tables and strikethrough. Links and images keep their URLs (made
  absolute with `base_url`), tables become pipe tables, code blocks are
  fenced with their language, and characters are escaped only where Markdown
  would misread them. 3–13× faster than markdownify, html2text and
  html-to-markdown on real pages.
- `html2text()` output is unchanged.

## 0.2.0 (2026-10-15)

- 1.5–1.7× faster than 0.1.0 on real web pages, and 2× faster on short
  snippets. Output is unchanged.
  - Wheels for Linux x86_64/aarch64, macOS and Windows x64 are built with
    profile-guided optimisation.
  - Uses mimalloc as the allocator (except on glibc armv7, ppc64le and s390x).
  - Style rules are shared rather than rebuilt and copied for every call.
- Deeply nested HTML no longer takes quadratic time to parse: 40,000 nested
  `<ul>` lists went from 6s to 0.14s (patched html5ever, see `vendor/`).
- `html` can be UTF-8 `bytes` as well as `str`.
- An internal error now raises `ValueError` rather than `PanicException`
  (which `except Exception` doesn't catch).
- Supports free-threaded Python without re-enabling the GIL.
- Requires Python 3.10 or later (3.9 is end-of-life).

## 0.1.0 (2026-10-15)

- First release: `html2text()` with `base_url` support.
