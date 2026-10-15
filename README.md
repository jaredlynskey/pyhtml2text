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

## Credits

The rendering is done by rust-html2text by Chris Emerson (MIT). This package
uses [a fork](https://github.com/jaredlynskey/rust-html2text/tree/base-url-resolution)
that adds `base_url`. It isn't affiliated with or endorsed by the upstream project,
so please report issues here rather than upstream.
