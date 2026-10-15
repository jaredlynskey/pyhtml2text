"""Checks for html2markdown, used by the tests and runnable on their own.

The check is a round trip: convert HTML to Markdown, render that back to HTML
with markdown-it (CommonMark plus GitHub's tables and strikethrough), and
compare the visible words, link targets and image sources of the two. Text
eaten by syntax that wasn't escaped, a link that didn't parse or a stray `*`
all show up as differences.

"Visible" follows the converter's rules: hidden elements, scripts, form
controls, SVG and MathML are dropped; <q> adds quotes and <rt> parentheses;
a link with nothing visible in it shows its aria-label or title.

    python tests/markdown_check.py pages FILE... [--base URL]
    python tests/markdown_check.py fuzz [N] [--seed S] [--shrink]

Needs markdown-it-py and html5lib.
"""

import argparse
import difflib
import gzip
import html as htmlmod
import pathlib
import random
import sys
import time
from urllib.parse import unquote, urljoin

import html5lib
from html5lib import serialize
from markdown_it import MarkdownIt

from pyhtml2text import html2markdown

NS = "{http://www.w3.org/1999/xhtml}"
SKIP = set(
    "head script style template noscript title meta link base iframe object embed video audio "
    "canvas map area input select textarea datalist option optgroup source track param frame "
    "frameset noframes colgroup col rp".split()
)
BLOCK = set(
    "p div address article aside center details summary fieldset legend figure figcaption footer "
    "header hgroup main nav section search form dl dd dt html body dialog h1 h2 h3 h4 h5 h6 "
    "blockquote ul ol menu dir li table caption thead tbody tfoot tr td th pre listing xmp "
    "plaintext hr br".split()
)
PRE = {"pre", "listing", "xmp", "plaintext"}

markdown = MarkdownIt("commonmark", {"html": True}).enable("table").enable("strikethrough")


def is_hidden(attrib):
    if "hidden" in attrib:
        return True
    if attrib.get("aria-hidden", "").strip().lower() == "true":
        return True
    style = "".join(attrib.get("style", "").split()).lower()
    return "display:none" in style or "visibility:hidden" in style


def image_source(attrib):
    def get(name):
        return attrib.get(name, "").strip() or None

    def first(srcset):
        parts = srcset.split(",")[0].split() if srcset else []
        return parts[0] if parts else None

    src = get("src")
    if src and not src.startswith("data:"):
        return src
    return (
        get("data-src")
        or get("data-lazy-src")
        or get("data-original")
        or first(get("data-srcset"))
        or first(get("srcset"))
        or src
    )


def link_href(attrib):
    href = attrib.get("href")
    if href is None:
        return None
    href = href.strip(" \t\n\r\x0c")
    if not href or href == "#" or href[:11].lower() == "javascript:":
        return None
    return href


def collapse(text):
    return " ".join(text.split())


def extract(root, mirror, in_link=False):
    """Visible text, link targets and image sources of a parsed document.

    `mirror` applies the converter's rules; it is off for markdown-it's
    output, which is plain HTML.
    """
    text, links, images = [], [], []
    stack = [(root, False, in_link, False, None)]
    while stack:
        el, leaving, in_link, in_pre, label = stack.pop()
        tag = el.tag[len(NS):] if isinstance(el.tag, str) and el.tag.startswith(NS) else None
        if leaving:
            if label is not None:
                text.append(label)
            if mirror and tag == "q":
                text.append('"')
            if mirror and tag == "rt":
                text.append(")")
            if tag in BLOCK:
                text.append(" ")
            if el.tail:
                text.append(el.tail)
            continue
        if tag is None or (
            mirror and (tag in SKIP or is_hidden(el.attrib) or (tag == "dialog" and "open" not in el.attrib))
        ):
            if el.tail:
                text.append(el.tail)
            continue
        if tag in BLOCK:
            text.append(" ")
        if mirror and tag == "q":
            text.append('"')
        if mirror and tag == "rt":
            text.append("(")
        child_in_link = in_link
        label = None
        if tag == "a" and not in_link and not in_pre:
            href = link_href(el.attrib) if mirror else el.attrib.get("href")
            if href is not None:
                if mirror:
                    inner, _, inner_images = extract(_detached(el), True, in_link=True)
                    visible = "".join(inner).split()
                else:
                    visible = "".join(el.itertext()).split()
                    inner_images = [d for d in el.iter() if d.tag == NS + "img" and d.get("src")]
                name = collapse(el.get("aria-label", "")) or collapse(el.get("title", ""))
                if visible or inner_images or (mirror and name):
                    links.append(href)
                    child_in_link = True
                    if mirror and not visible and not inner_images:
                        # The converter writes the label after whatever
                        # whitespace was in the link, and a space after it
                        # if there was any.
                        label = name + (" " if any(inner) else "")
        if tag == "img" and not in_pre:
            src = image_source(el.attrib) if mirror else el.get("src")
            if src:
                images.append(src)
        if el.text:
            text.append(el.text)
        stack.append((el, True, in_link, in_pre, label))
        child_in_pre = in_pre or tag in PRE
        for child in reversed(list(el)):
            stack.append((child, False, child_in_link, child_in_pre, None))
    return text, links, images


def _detached(el):
    """The element without its tail, so that only its own content is read."""
    import copy

    clone = copy.copy(el)
    clone.tail = None
    return clone


def words(text_parts):
    return "".join(text_parts).replace("\xa0", " ").split()


def parse(html):
    return html5lib.parse(html, treebuilder="etree", namespaceHTMLElements=True)


def compare(html, base=None):
    """Convert `html` and return (markdown, problems), where each problem is
    (kind, expected, got) for kind in words, links, images."""
    md = html2markdown(html, base_url=base)
    t1, l1, i1 = extract(parse(html), True)
    t2, l2, i2 = extract(parse(markdown.render(md)), False)
    resolve = lambda u: unquote(urljoin(base, "".join(c for c in u if c not in "\t\n\r")) if base else u)
    l1, i1 = [resolve(u) for u in l1], [resolve(u) for u in i1]
    l2, i2 = [unquote(u) for u in l2], [unquote(u) for u in i2]
    problems = [
        (kind, a, b)
        for kind, a, b in (("words", words(t1), words(t2)), ("links", l1, l2), ("images", i1, i2))
        if a != b
    ]
    return md, problems


def describe(problems, limit=3):
    lines = []
    for kind, a, b in problems:
        ops = [op for op in difflib.SequenceMatcher(a=a, b=b, autojunk=False).get_opcodes() if op[0] != "equal"]
        lines.append(f"{kind}: {len(ops)} difference(s)")
        for op, a0, a1, b0, b1 in ops[:limit]:
            lines.append(f"    {op}: html={a[max(0, a0 - 2):a1 + 2]} md={b[max(0, b0 - 2):b1 + 2]}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Random documents
# ---------------------------------------------------------------------------

TOKENS = [
    "foo", "bar", "baz", "snake_case", "__init__", "*star*", "_under_", "**", "__", "`tick`",
    "[x](y)", "a]b", "[1]", "<tag>", "&copy;", "1.", "2)", "-", "+", "#", "##", ">", "=", "---",
    "|", "a|b", "\\", "\\*", "~~", "~", "한국어", "(괄호)", '"인용"', "의", "를", ".", ",", "!", "?",
    ":", ";", "x", "http://example.com/a_b", "%20", "C:\\path", "…", "—", "\u00a0", "\t", "\n",
    "  ", "'", "(", ")", "*", "_", "`", "[", "]", "<", "&", "&amp;", "123.", "9)", "!!", "x*y",
    "a_b_c", "**bold**", "<!--", "</p>", "&#42;", "[^1]", "![img]", "@", "$5", "%", "^", "{}",
]
HREFS = ["/x", "https://e.com/p?q=1&r=2", "#frag", "", "#", "javascript:void(0)", "/a b",
         "/(x)", "/a)b", "rel/path", "mailto:a@b.co", "https://e.com/", "/%7Ex", "/한국"]
SRCS = ["/i.png", "https://e.com/a.jpg", "", "data:image/gif;base64,R0", "/a b.png"]
INLINE = ["b", "strong", "i", "em", "code", "kbd", "a", "del", "s", "span", "sup", "sub",
          "abbr", "mark", "u", "small", "cite", "var", "samp", "q"]
BLOCKS = ["p", "div", "h1", "h2", "h3", "h6", "ul", "ol", "blockquote", "pre", "table", "dl",
          "hr", "section", "figure", "details"]


def _text(r):
    out = []
    for _ in range(r.randint(1, 4)):
        token = r.choice(TOKENS)
        out.append(htmlmod.escape(token, quote=False) if r.random() < 0.9 else token)
        if r.random() < 0.6:
            out.append(r.choice([" ", " ", "", "\n", "  "]))
    return "".join(out)


def _hidden(r):
    x = r.random()
    return " hidden" if x < 0.02 else ' aria-hidden="true"' if x < 0.04 else ' style="display: none"' if x < 0.06 else ""


def _inline(r, depth):
    parts = []
    for _ in range(r.randint(1, 4)):
        x = r.random()
        if x < 0.45 or depth > 4:
            parts.append(_text(r))
        elif x < 0.5:
            parts.append("<br>")
        elif x < 0.56:
            alt = htmlmod.escape(r.choice(TOKENS), quote=True)
            parts.append(f'<img src="{r.choice(SRCS)}" alt="{alt}">')
        else:
            tag = r.choice(INLINE)
            attrs = _hidden(r)
            if tag == "a":
                attrs += f' href="{htmlmod.escape(r.choice(HREFS), quote=True)}"'
                if r.random() < 0.2:
                    attrs += f' title="{htmlmod.escape(r.choice(TOKENS), quote=True)}"'
            parts.append(f"<{tag}{attrs}>{_inline(r, depth + 1)}</{tag}>")
    return "".join(parts)


def _table(r, depth):
    columns = r.randint(1, 4)
    rows = []
    if r.random() < 0.5:
        rows.append("<thead><tr>" + "".join(f"<th>{_inline(r, depth + 1)}</th>" for _ in range(columns)) + "</tr></thead>")
    body = []
    for _ in range(r.randint(1, 4)):
        cells = []
        for _ in range(r.randint(1, columns)):
            span = ""
            if r.random() < 0.15:
                span += f' colspan="{r.randint(0, 3)}"'
            if r.random() < 0.15:
                span += f' rowspan="{r.randint(0, 3)}"'
            content = _inline(r, depth + 1) if r.random() < 0.8 else _block(r, depth + 1)
            cells.append(f"<td{span}>{content}</td>")
        body.append("<tr>" + "".join(cells) + "</tr>")
    rows.append("<tbody>" + "".join(body) + "</tbody>")
    caption = f"<caption>{_text(r)}</caption>" if r.random() < 0.2 else ""
    return f"<table>{caption}{''.join(rows)}</table>"


def _block(r, depth):
    if depth > 5:
        return f"<p>{_inline(r, depth)}</p>"
    tag = r.choice(BLOCKS)
    attrs = _hidden(r)
    if tag in ("ul", "ol"):
        start = f' start="{r.randint(0, 12)}"' if tag == "ol" and r.random() < 0.3 else ""
        items = "".join(
            "<li>"
            + (_inline(r, depth + 1) if r.random() < 0.6 else _block(r, depth + 1))
            + (_block(r, depth + 1) if r.random() < 0.3 else "")
            + "</li>"
            for _ in range(r.randint(1, 4))
        )
        return f"<{tag}{attrs}{start}>{items}</{tag}>"
    if tag == "pre":
        language = f' class="language-{r.choice(["py", "rust", "c++"])}"' if r.random() < 0.3 else ""
        code = "".join(r.choice(TOKENS + ["\n", "```", "    ", "\n\n"]) for _ in range(r.randint(1, 12)))
        return f"<pre{attrs}><code{language}>{htmlmod.escape(code, quote=False)}</code></pre>"
    if tag == "table":
        return _table(r, depth)
    if tag == "dl":
        return f"<dl><dt>{_inline(r, depth + 1)}</dt><dd>{_inline(r, depth + 1)}</dd></dl>"
    if tag == "hr":
        return "<hr>"
    if tag == "figure":
        return f'<figure><img src="{r.choice(SRCS)}" alt="f"><figcaption>{_inline(r, depth + 1)}</figcaption></figure>'
    if tag == "details":
        return f"<details><summary>{_inline(r, depth + 1)}</summary>{_block(r, depth + 1)}</details>"
    if tag in ("blockquote", "div", "section"):
        inner = "".join(_block(r, depth + 1) if r.random() < 0.6 else _inline(r, depth + 1) for _ in range(r.randint(1, 3)))
        return f"<{tag}{attrs}>{inner}</{tag}>"
    return f"<{tag}{attrs}>{_inline(r, depth + 1)}</{tag}>"


def random_document(r):
    """A random HTML fragment built to stress Markdown escaping and nesting."""
    return "".join(_block(r, 0) if r.random() < 0.7 else _inline(r, 0) for _ in range(r.randint(1, 5)))


def random_cases(seed, count):
    """(html, base_url) pairs, the same for the same seed."""
    r = random.Random(seed)
    for _ in range(count):
        html = random_document(r)
        yield html, ("https://e.com/d/p" if r.random() < 0.5 else None)


# ---------------------------------------------------------------------------
# Shrinking a failing document
# ---------------------------------------------------------------------------


def _kinds(html, base):
    try:
        return {kind for kind, _, _ in compare(html, base)[1]}
    except Exception:  # noqa: BLE001 - any failure is worth shrinking
        return {"exception"}


def _body(html):
    return html5lib.parse(html, treebuilder="etree", namespaceHTMLElements=False).find("body")


def _serialize(body):
    out = body.text or ""
    for child in body:
        out += serialize(child, tree="etree", omit_optional_tags=False, quote_attr_values="always")
    return out


def _without(html, n, unwrap):
    body = _body(html)
    elements = [e for e in body.iter() if e is not body]
    el = elements[n]
    parent = {c: p for p in body.iter() for c in p}[el]
    i = list(parent).index(el)
    kids = list(el) if unwrap else []
    text = (el.text or "") if unwrap else ""
    tail = el.tail or ""
    parent.remove(el)
    for k, kid in enumerate(kids):
        parent.insert(i + k, kid)
    if kids:
        kids[-1].tail = (kids[-1].tail or "") + tail
        tail = ""
    if text + tail:
        if i > 0:
            parent[i - 1].tail = (parent[i - 1].tail or "") + text + tail
        else:
            parent.text = (parent.text or "") + text + tail
    return _serialize(body)


def _smaller(html):
    count = sum(1 for _ in _body(html).iter()) - 1
    for n in range(count):
        for unwrap in (False, True):
            yield _without(html, n, unwrap)
    body = _body(html)
    for el in body.iter():
        for attr in ("text", "tail"):
            value = getattr(el, attr) if (attr == "text" or el is not body) else None
            if not value:
                continue
            parts = value.split(" ")
            options = [" ".join(parts[:i] + parts[i + 1:]) for i in range(len(parts))] if len(parts) > 1 else [value[: len(value) // 2], value[len(value) // 2:]]
            for option in options:
                setattr(el, attr, option)
                yield _serialize(body)
            setattr(el, attr, value)


def shrink(html, base):
    """The smallest document found that still fails the same way."""
    kinds = _kinds(html, base)
    progress = True
    while progress:
        progress = False
        for candidate in _smaller(html):
            if len(candidate) < len(html) and _kinds(candidate, base) & kinds:
                html, progress = candidate, True
                break
    return html


# ---------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    pages = sub.add_parser("pages", help="round-trip HTML files")
    pages.add_argument("files", nargs="+")
    pages.add_argument("--base", default=None)
    fuzz = sub.add_parser("fuzz", help="round-trip random documents")
    fuzz.add_argument("count", nargs="?", type=int, default=2000)
    fuzz.add_argument("--seed", type=int, default=1)
    fuzz.add_argument("--shrink", action="store_true", help="shrink and print failing documents")
    args = parser.parse_args()

    if args.command == "pages":
        clean = 0
        for path in args.files:
            raw = pathlib.Path(path).read_bytes()
            html = (gzip.decompress(raw) if path.endswith(".gz") else raw).decode("utf-8", "replace")
            start = time.perf_counter()
            _, problems = compare(html, args.base)
            took = (time.perf_counter() - start) * 1000
            clean += not problems
            print(f"{'OK  ' if not problems else 'FAIL'} {path}  ({len(html) // 1024} KB, {took:.0f} ms with checking)")
            if problems:
                print("     " + describe(problems).replace("\n", "\n     "))
        print(f"\n{clean}/{len(args.files)} pages round-trip clean")
        sys.exit(clean != len(args.files))

    failures, shown, seen = 0, 0, set()
    start = time.time()
    for i, (html, base) in enumerate(random_cases(args.seed, args.count)):
        md, problems = compare(html, base)
        if not problems:
            continue
        failures += 1
        if args.shrink and shown < 20:
            small = shrink(html, base)
            if small in seen:
                continue
            seen.add(small)
            shown += 1
            md, problems = compare(small, base)
            print(f"#{i} base={base}\n  html: {small!r}\n  md:   {md!r}\n  " + describe(problems).replace("\n", "\n  "))
    print(f"\n{args.count - failures}/{args.count} random documents round-trip clean (seed {args.seed}, {time.time() - start:.1f}s)")
    sys.exit(failures != 0)


if __name__ == "__main__":
    main()
