# Changelog

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
