from typing import Optional

def html2text(
    html: str,
    width: int = 80,
    *,
    base_url: Optional[str] = None,
    decorate: bool = True,
    link_footnotes: bool = True,
    table_borders: bool = True,
    wrap_links: bool = True,
    max_wrap_width: Optional[int] = None,
    raw: bool = False,
) -> str: ...
