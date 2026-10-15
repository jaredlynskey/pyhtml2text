"""Exercise pyhtml2text so the CI can build profile-guided (PGO) wheels.

CI builds an instrumented wheel, installs it, runs this script, and uses the
resulting profile to optimise the real build. Run as:

    python pgo/train.py [ROUNDS]
"""

import gzip
import pathlib
import sys

from pyhtml2text import html2text

CORPUS = pathlib.Path(__file__).parent / "corpus"
OPTION_SETS = [
    {},
    {"width": 40},
    {"width": 120, "base_url": "https://example.com/docs/page.html"},
    {"decorate": False, "link_footnotes": False},
    {"table_borders": False, "wrap_links": False},
    {"max_wrap_width": 60},
    {"raw": True},
]
SNIPPETS = [
    "<p>Hello <b>world</b>, <a href='/x'>link</a></p>",
    "<ul><li>one<li>two <em>three</em></ul>",
    "<table><tr><th>a<th>b<tr><td>1<td>2</table>",
    "<h1>Title</h1><p>Some <code>code</code> and text.<br>More.</p>",
    "",
]


def main():
    rounds = int(sys.argv[1]) if len(sys.argv) > 1 else 3
    pages = [gzip.decompress(p.read_bytes()) for p in sorted(CORPUS.glob("*.html.gz"))]
    if not pages:
        sys.exit(f"no training pages found in {CORPUS}")
    for _ in range(rounds):
        for page in pages:
            text = page.decode("utf-8")
            for opts in OPTION_SETS:
                html2text(text, **opts)
            html2text(page)
    for _ in range(5000):
        for snippet in SNIPPETS:
            html2text(snippet)
    html2text("<ul><li>" * 2000 + "x")
    html2text("<blockquote>" * 2000 + "x")
    print(f"trained on {len(pages)} pages x {len(OPTION_SETS) + 1} option sets x {rounds} rounds")


if __name__ == "__main__":
    main()
