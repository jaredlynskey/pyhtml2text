# pyhtml2text

Fast HTML to plain text and to Markdown for Python, written in Rust.

Relative links can be resolved against the page's URL, so the output's links
work on their own.

## Markdown

```python
from pyhtml2text import html2markdown

html = """<h2>Before you start</h2>
<p>Read the <a href="setup.html">setup guide</a> and <b>back up</b> your data.</p>
<img src="/img/diagram.png" alt="How it fits together">
<table><tr><th>Plan</th><th>Price</th></tr><tr><td>Basic</td><td>Free</td></tr></table>"""

print(html2markdown(html, base_url="https://example.com/docs/intro.html"))
```

```markdown
## Before you start

Read the [setup guide](https://example.com/docs/setup.html) and **back up** your data.

![How it fits together](https://example.com/img/diagram.png)

| Plan | Price |
| --- | --- |
| Basic | Free |
```

`html2markdown(html, *, base_url=None)` writes
[CommonMark](https://commonmark.org/) with GitHub's tables and
strikethrough, in the usual conventions:

- `#` headings, `-` bullets, `_emphasis_` and `**strong**`, `~~strikethrough~~`.
- Inline links and images, `[text](url)` and `![alt](src)`, made absolute when
  `base_url` is given. A link whose only content is an icon uses its
  `aria-label` or `title` as its text.
- Fenced code blocks, tagged with the language when the page names one
  (`class="language-python"`).
- Pipe tables. Cells spanning rows or columns become empty cells, a table
  without a header row gets an empty one, and tables used for page layout are
  written as plain paragraphs.
- Characters are escaped only where Markdown would otherwise read them as
  syntax, so `snake_case`, `5 * 3` and `$5` stay as they are.
- Lines aren't wrapped. Hidden content (`hidden`, `aria-hidden`,
  `display: none`), scripts, styles and form controls are left out.

The output is checked by rendering it back with a CommonMark parser and
comparing every word, link and image with the original page, on real sites
and on tens of thousands of random documents (`tests/markdown_check.py`).

## Plain text

```python
from pyhtml2text import html2text

html = '<p>Please <a href="/login">sign in</a> first.</p>'

print(html2text(html, base_url="https://example.com/docs/page.html"))
# Please [sign in][1] first.
#
# [1]: https://example.com/login
```

`html2text(html, width=80, *, ...)`

| option | default | |
|---|---|---|
| `base_url` | `None` | resolve relative `href`s against this URL (RFC 3986). Absolute and `mailto:` etc. links are left alone |
| `decorate` | `True` | markdown-ish `*em*`, `**strong**`, `` `code` `` |
| `link_footnotes` | `True` | list link targets as `[1]: ...` footnotes |
| `table_borders` | `True` | draw table borders |
| `wrap_links` | `True` | allow long links to wrap |
| `max_wrap_width` | `None` | wrap paragraphs narrower than `width` |
| `raw` | `False` | flatten tables so cell text stays together |

Both functions take a `str` or UTF-8 `bytes` (e.g. `response.content`).
Requires Python 3.10 or later.

## Performance

Median time on an Apple Silicon Mac. To Markdown:

| | Wikipedia article (1 MB) | BBC News front page (370 KB) | Python docs page (245 KB) |
|---|---|---|---|
| [markdownify](https://pypi.org/project/markdownify/) | 184 ms | 28 ms | 97 ms |
| [html2text](https://pypi.org/project/html2text/) (pure Python) | 73 ms | 13 ms | 37 ms |
| [html-to-markdown](https://pypi.org/project/html-to-markdown/) | 44 ms | 5.6 ms | 15 ms |
| **pyhtml2text** | **14 ms** | **1.7 ms** | **4.5 ms** |

To plain text:

| | Wikipedia article (780 KB) | BBC News front page (370 KB) |
|---|---|---|
| [html2text](https://pypi.org/project/html2text/) (pure Python) | 71.5 ms | 14.9 ms |
| [html2text-rs](https://pypi.org/project/html2text-rs/) | 15.2 ms | 2.5 ms |
| **pyhtml2text** | **7.0 ms** | **1.3 ms** |

- The GIL is released while converting, so threads scale across cores. On
  free-threaded Python (3.14t) the module runs without re-enabling the GIL.
- Wheels for Linux (x86_64, aarch64), macOS and Windows x64 are built with
  profile-guided optimisation, which makes them about 25% faster. Other
  platforms get the same code without it.
- Deeply nested HTML is parsed in linear time, so hostile input can't stall a
  worker: 40,000 nested lists take 0.14 s, against 6 s with stock html5ever,
  and 100,000 levels of nesting followed by unmatched end tags take under
  0.1 s.

## Credits

HTML is parsed by a copy of [html5ever](https://github.com/servo/html5ever)
(MIT/Apache-2.0) with
[servo/html5ever#800](https://github.com/servo/html5ever/pull/800) by Josef
Guenther applied, which makes parsing deeply nested HTML linear (see
`vendor/`). Text is measured with
[unicode-width](https://github.com/unicode-rs/unicode-width) (MIT/Apache-2.0)
and Markdown output is checked with
[pulldown-cmark](https://github.com/raphlinus/pulldown-cmark) (MIT). This
package isn't affiliated with or endorsed by any of these projects, so please
report issues here rather than upstream.
