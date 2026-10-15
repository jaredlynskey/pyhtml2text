"""Benchmark and output-regression harness.

    python bench/bench.py CORPUS_DIR                 # timings
    python bench/bench.py CORPUS_DIR --save FILE     # record output hashes
    python bench/bench.py CORPUS_DIR --check FILE    # compare against them

CORPUS_DIR holds *.html files (not committed - fetch your own pages).
"""

import argparse
import hashlib
import json
import pathlib
import statistics
import sys
import time

import pyhtml2text
from pyhtml2text import html2text

BASE = "https://example.com/dir/page.html"
OPTION_SETS = {
    "default": {},
    "w40": {"width": 40},
    "w120": {"width": 120},
    "base_url": {"base_url": BASE},
    "no_decorate": {"decorate": False},
    "no_footnotes": {"link_footnotes": False},
    "no_borders": {"table_borders": False},
    "no_wrap_links": {"wrap_links": False},
    "max_wrap_40": {"max_wrap_width": 40},
    "raw": {"raw": True},
    "everything": {
        "width": 100,
        "base_url": BASE,
        "decorate": False,
        "table_borders": False,
        "wrap_links": False,
    },
}
EDGE_CASES = {
    "empty": "",
    "text": "plain text, no tags",
    "unicode": "<p>한국어 텍스트 é 🎉 <b>漢字</b></p>",
    "nested_ul": "<ul><li>" * 300 + "x" + "</li></ul>" * 300,
    "nested_bq": "<blockquote>" * 300 + "x" + "</blockquote>" * 300,
    "nested_div_p": "<p>a<div>" * 300 + "x",
    "table": "<table><tr><td>a</td><td rowspan=2>b</td></tr><tr><td>c</td></tr></table>",
    "rowspan0": "<table><tr><td rowspan=0>a</td></tr></table>",
    "links": '<a href="/x">x</a> <a href="../y?q=1#f">y</a> <a href="mailto:a@b">m</a>',
    "unclosed": "<p>one<p>two<li>three<li>four<div>five",
}


def inputs(corpus):
    for path in sorted(pathlib.Path(corpus).glob("*.html")):
        yield path.stem, path.read_text(encoding="utf-8", errors="replace")
    yield from EDGE_CASES.items()


def run(html, opts):
    opts = dict(opts)
    width = opts.pop("width", 80)
    try:
        return html2text(html, width, **opts)
    except ValueError as e:
        return f"ValueError: {e}"


def run_markdown(html, base_url=None):
    try:
        return pyhtml2text.html2markdown(html, base_url=base_url)
    except (AttributeError, ValueError) as e:  # AttributeError: before 0.3.0
        return f"{type(e).__name__}: {e}"


def hashes(corpus):
    out = {}
    for name, html in inputs(corpus):
        for oname, opts in OPTION_SETS.items():
            out[f"{name}/{oname}"] = hashlib.sha256(run(html, opts).encode()).hexdigest()
        if hasattr(pyhtml2text, "html2markdown"):
            for oname, base in (("markdown", None), ("markdown_base_url", BASE)):
                digest = hashlib.sha256(run_markdown(html, base).encode()).hexdigest()
                out[f"{name}/{oname}"] = digest
    return out


def median_ms(fn, html, repeat):
    fn(html)
    xs = []
    for _ in range(repeat):
        t = time.perf_counter()
        fn(html)
        xs.append(time.perf_counter() - t)
    return statistics.median(xs) * 1000


def timings(corpus, repeat):
    markdown = hasattr(pyhtml2text, "html2markdown")
    print(f"{'input':22s} {'KiB':>6s} {'text ms':>10s}" + (f" {'markdown ms':>12s}" if markdown else ""))
    for name, html in inputs(corpus):
        if name in EDGE_CASES:
            continue
        line = f"{name:22s} {len(html.encode()) / 1024:6.0f} {median_ms(lambda h: run(h, {}), html, repeat):10.2f}"
        if markdown:
            line += f" {median_ms(run_markdown, html, repeat):12.2f}"
        print(line)
    tiny = "<p>Hello <b>world</b>, <a href='/x'>link</a></p>"
    n = 20000
    t = time.perf_counter()
    for _ in range(n):
        html2text(tiny)
    print(f"{'tiny snippet':22s} {'':6s} {(time.perf_counter() - t) / n * 1e6:9.2f}µs")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("--save")
    ap.add_argument("--check")
    ap.add_argument("--repeat", type=int, default=15)
    a = ap.parse_args()
    if a.save:
        h = hashes(a.corpus)
        pathlib.Path(a.save).write_text(json.dumps(h, indent=1, sort_keys=True))
        print(f"saved {len(h)} hashes")
    elif a.check:
        want = json.loads(pathlib.Path(a.check).read_text())
        got = hashes(a.corpus)
        bad = sorted(k for k in want if got.get(k) != want[k])
        print(f"{len(want) - len(bad)}/{len(want)} identical")
        for k in bad:
            print("  DIFFERS:", k)
        sys.exit(1 if bad else 0)
    else:
        timings(a.corpus, a.repeat)


if __name__ == "__main__":
    main()
