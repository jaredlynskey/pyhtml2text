# pyhtml2text

Fast HTML to plain text for Python, built on
[rust-html2text](https://github.com/jugglerchris/rust-html2text).

It adds one thing the upstream crate doesn't have yet: relative links can be
resolved against the page's URL, so footnote links are usable on their own.

```python
from pyhtml2text import html2text

html = '<p>Please <a href="/login">sign in</a> first.</p>'

print(html2text(html, base_url="https://example.com/docs/page.html"))
# Please [sign in][1] first.
#
# [1]: https://example.com/login
```

`html` can be a `str` or UTF-8 `bytes` (e.g. `response.content`). Requires
Python 3.10 or later.

## Options

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

## Performance

Median time to render a page, on an Apple Silicon Mac:

| | Wikipedia article (780 KB) | BBC News front page (370 KB) |
|---|---|---|
| [html2text](https://pypi.org/project/html2text/) (pure Python) | 71.5 ms | 14.9 ms |
| [html2text-rs](https://pypi.org/project/html2text-rs/) | 15.2 ms | 2.5 ms |
| **pyhtml2text** | **10.9 ms** | **1.6 ms** |

- The GIL is released while rendering, so threads scale across cores. On
  free-threaded Python (3.14t) the module runs without re-enabling the GIL.
- Wheels for Linux (x86_64, aarch64), macOS and Windows x64 are built with
  profile-guided optimisation, which makes them about 25% faster. Other
  platforms get the same code without it.
- Deeply nested HTML is parsed in linear time, so hostile input can't stall a
  worker: 40,000 nested lists take 0.14 s, against 6 s with stock html5ever.

## Credits

The rendering is done by rust-html2text by Chris Emerson (MIT). This package
uses [a fork](https://github.com/jaredlynskey/rust-html2text/tree/pyhtml2text-perf)
that adds `base_url` and some performance changes, and a patched copy of
[html5ever](https://github.com/servo/html5ever) (MIT/Apache-2.0, see
`vendor/`). It isn't affiliated with or endorsed by either upstream project, so
please report issues here rather than upstream.
