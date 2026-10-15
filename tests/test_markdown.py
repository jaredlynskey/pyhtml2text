import pathlib
import platform
import sys
import time

import pytest

from pyhtml2text import html2markdown as md

PAGE = "https://example.com/dir/page.html"


# Blocks


def test_headings_and_paragraphs():
    assert md("<h1>Title</h1><p>One</p><p>Two</p>") == "# Title\n\nOne\n\nTwo\n"


def test_lists():
    assert md("<ul><li>a</li><li>b<ul><li>c</li></ul></li></ul>") == "- a\n- b\n  - c\n"
    assert md('<ol start="3"><li>x</li><li>y</li></ol>') == "3. x\n4. y\n"


def test_adjacent_lists_stay_separate():
    # Side by side, two lists would read as one: CommonMark's separator.
    assert md("<ul><li>a</li></ul><ul><li>b</li></ul>") == "- a\n\n<!-- -->\n\n- b\n"
    # A numbered list that carries on from the last one simply joins it.
    assert md('<ol><li>a</li></ol><ol start="2"><li>b</li></ol>') == "1. a\n\n2. b\n"


def test_blockquote():
    assert md("<blockquote><p>a</p><p>b</p></blockquote>") == "> a\n>\n> b\n"


def test_code_blocks():
    assert md('<pre><code class="language-py">x = 1\n</code></pre>') == "```py\nx = 1\n```\n"
    assert md("<pre>has ``` inside</pre>") == "````\nhas ``` inside\n````\n"


def test_rule():
    assert md("<p>a</p><hr><p>b</p>") == "a\n\n***\n\nb\n"


def test_table():
    html = (
        "<table><thead><tr><th>A</th><th align=right>B</th></tr></thead>"
        "<tr><td>1</td><td>x|y</td></tr></table>"
    )
    assert md(html) == "| A | B |\n| --- | ---: |\n| 1 | x\\|y |\n"


def test_table_without_header_row_gets_an_empty_one():
    assert md("<table><tr><td>a</td><td>b</td></tr></table>") == "| | |\n| --- | --- |\n| a | b |\n"


def test_table_spans_become_empty_cells():
    html = (
        "<table><tr><th>A</th><th>B</th></tr>"
        "<tr><td rowspan=2>tall</td><td>1</td></tr><tr><td>2</td></tr></table>"
    )
    assert md(html) == "| A | B |\n| --- | --- |\n| tall | 1 |\n| | 2 |\n"


def test_layout_tables_become_blocks():
    assert md("<table><tr><td>only</td></tr><tr><td>column</td></tr></table>") == "only\n\ncolumn\n"


# Inline


def test_emphasis():
    html = "<p><b>bold</b> <i>it</i> <del>gone</del> <code>x</code></p>"
    assert md(html) == "**bold** _it_ ~~gone~~ `x`\n"


def test_emphasis_inside_a_word_uses_asterisks():
    assert md("<p>un<i>frigging</i>believable</p>") == "un*frigging*believable\n"


def test_bold_before_a_korean_particle():
    # `**타이타닉(Titanic)**의` would stay literal: CommonMark won't close
    # emphasis after punctuation when a letter follows.
    assert md("<p><b>타이타닉(Titanic)</b>의 줄거리</p>") == "**타이타닉(Titanic**)의 줄거리\n"


def test_spaces_move_outside_markers():
    assert md("<p>a<b> b </b>c</p>") == "a **b** c\n"


def test_links_and_images_are_resolved():
    html = '<p><a href="/x" title="T">link</a> <img src="i.png" alt="pic"></p>'
    expected = '[link](https://example.com/x "T") ![pic](https://example.com/dir/i.png)\n'
    assert md(html, base_url=PAGE) == expected


def test_relative_links_unchanged_without_base():
    assert md('<a href="/login">Sign in</a>') == "[Sign in](/login)\n"


def test_link_whose_text_is_its_url_is_an_autolink():
    assert md('<a href="https://x.org/">https://x.org/</a>') == "<https://x.org/>\n"


def test_icon_link_uses_its_label():
    assert md('<a href="/t" aria-label="Twitter"><svg></svg></a>') == "[Twitter](/t)\n"


def test_link_stays_whole_when_formatting_changes_inside_it():
    assert md('<a href="/f"><b>296</b> forks</a>') == "[**296** forks](/f)\n"


def test_link_around_blocks_becomes_one_link():
    assert md('<a href="/p"><h3>Title</h3><p>Desc</p></a>') == "[Title Desc](/p)\n"


def test_script_links_keep_only_their_text():
    assert md('<a href="javascript:void(0)">x</a> <a href="#">y</a>') == "x y\n"


def test_image_lazy_loading_source():
    html = '<img src="data:image/gif;base64,R0" data-src="/real.jpg" alt="a">'
    assert md(html) == "![a](/real.jpg)\n"


# Escaping


@pytest.mark.parametrize(
    "html, expected",
    [
        ("<p>1. not a list</p>", "1\\. not a list\n"),
        ("<p># not a heading</p>", "\\# not a heading\n"),
        ("<p>- not a bullet</p>", "\\- not a bullet\n"),
        ("<p>&gt; not a quote</p>", "\\> not a quote\n"),
        ("<p>*star* _under_ snake_case</p>", "\\*star\\* \\_under\\_ snake_case\n"),
        ("<p>5 * 3</p>", "5 * 3\n"),
        ("<p>[x](y) and [1] here</p>", "[x\\](y) and [1] here\n"),
        ("<p>&lt;div&gt; &amp;copy;</p>", "\\<div> \\&copy;\n"),
        ("<p>wow!<a href='/x'>link</a></p>", "wow\\![link](/x)\n"),
        ("<p>$5 or $x$</p>", "$5 or \\$x\\$\n"),
        ("<p><code>a`b</code></p>", "``a`b``\n"),
    ],
)
def test_escaping(html, expected):
    assert md(html) == expected


def test_line_breaks():
    assert md("<p>one<br>two<br><br>three</p>") == "one  \ntwo\n\nthree\n"


def test_hidden_content_is_dropped():
    html = (
        '<p>a<span hidden>H</span><span style="display: none">D</span>'
        '<span aria-hidden="true">A</span>b</p><script>s</script><template>t</template>'
    )
    assert md(html) == "ab\n"


def test_empty_and_input_types():
    assert md("") == ""
    assert md("  <p> </p> ") == ""
    assert md("<p>café</p>".encode()) == "café\n"
    with pytest.raises(TypeError, match="str or bytes"):
        md(123)


def test_hostile_input_stays_linear():
    cases = [
        "<ul><li>" * 100_000 + "x",
        "<blockquote>" * 100_000 + "x",
        "<table><tr><td>" * 10_000 + "x",
        "<table><tr><td rowspan=65534 colspan=1000>a</td><td>b</td></tr>" + "<tr></tr>" * 200_000 + "</table>",
    ]
    for html in cases:
        start = time.perf_counter()
        out = md(html)
        elapsed = time.perf_counter() - start
        assert len(out) < len(html)
        # Only timed where tests run natively; emulated CI runners are slow.
        if platform.machine().lower() in ("x86_64", "amd64") or sys.platform == "darwin":
            assert elapsed < 5, f"took {elapsed:.1f}s"


# Round trips: render the Markdown back to HTML and compare with the page.

markdown_check = pytest.importorskip("markdown_check", reason="needs markdown-it-py and html5lib")
CORPUS = sorted((pathlib.Path(__file__).parent.parent / "pgo" / "corpus").glob("*.html.gz"))


@pytest.mark.parametrize("path", CORPUS, ids=[p.name for p in CORPUS])
def test_real_pages_round_trip(path):
    import gzip

    html = gzip.decompress(path.read_bytes()).decode()
    _, problems = markdown_check.compare(html)
    assert not problems, markdown_check.describe(problems)


def test_random_documents_round_trip():
    failures = []
    for html, base in markdown_check.random_cases(seed=0, count=500):
        _, problems = markdown_check.compare(html, base)
        if problems:
            failures.append(f"{html!r}\n{markdown_check.describe(problems)}")
    assert not failures, failures[0]
