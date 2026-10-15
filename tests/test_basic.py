from pyhtml2text import html2text

PAGE = "https://example.com/dir/page.html"


def test_plain():
    assert html2text("<p>Hello <b>world</b></p>") == "Hello **world**\n"


def test_relative_links_unchanged_without_base():
    out = html2text('<p><a href="/login">Sign in</a></p>')
    assert out == "[Sign in][1]\n\n[1]: /login\n"


def test_base_url_resolves():
    html = (
        '<p><a href="/login">a</a> <a href="../other.html">b</a> '
        '<a href="next.html?p=2#top">c</a> <a href="https://other.example/x">d</a> '
        '<a href="mailto:x@example.com">e</a></p>'
    )
    out = html2text(html, base_url=PAGE)
    assert "[1]: https://example.com/login\n" in out
    assert "[2]: https://example.com/other.html\n" in out
    assert "[3]: https://example.com/dir/next.html?p=2#top\n" in out
    assert "[4]: https://other.example/x\n" in out
    assert "[5]: mailto:x@example.com\n" in out


def test_no_footnotes():
    assert html2text('<a href="/x">x</a>', link_footnotes=False) == "[x]\n"


def test_width_wraps():
    out = html2text("<p>" + "word " * 30 + "</p>", width=20)
    assert all(len(line) <= 20 for line in out.splitlines())


def test_bad_base_url_leaves_links_alone():
    for base in ("not a url", ""):
        assert html2text('<a href="/x">x</a>', base_url=base) == "[x][1]\n\n[1]: /x\n"


def test_zero_width_raises():
    import pytest

    with pytest.raises(ValueError):
        html2text("<p>hi</p>", width=0)


def test_tables():
    html = "<table><tr><td>a</td><td>b</td></tr></table>"
    assert html2text(html) == "─┬─\na│b\n─┴─\n"
    assert html2text(html, table_borders=False) == "a b\n"


def test_unicode_and_empty():
    assert html2text("<p>한국어 텍스트 é</p>") == "한국어 텍스트 é\n"
    assert html2text("") == ""
