def html2text(
    html: str | bytes,
    width: int = 80,
    *,
    base_url: str | None = None,
    decorate: bool = True,
    link_footnotes: bool = True,
    table_borders: bool = True,
    wrap_links: bool = True,
    max_wrap_width: int | None = None,
    raw: bool = False,
) -> str: ...
