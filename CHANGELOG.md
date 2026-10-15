# Changelog

## 0.2.0 (2026-10-15)

- 1.5–1.7× faster than 0.1.0 on real web pages, and 2× faster on short
  snippets. Output is unchanged.
  - Wheels for Linux x86_64/aarch64, macOS and Windows x64 are built with
    profile-guided optimisation.
  - Uses mimalloc as the allocator (except on glibc armv7, ppc64le and s390x).
  - Style rules are shared rather than rebuilt and copied for every call.
- Deeply nested HTML no longer takes quadratic time to parse: 40,000 nested
  `<ul>` lists went from 6s to 0.14s (patched html5ever, see `vendor/`).
- `html` can be UTF-8 `bytes` as well as `str`.
- An internal error now raises `ValueError` rather than `PanicException`
  (which `except Exception` doesn't catch).
- Supports free-threaded Python without re-enabling the GIL.
- Requires Python 3.10 or later (3.9 is end-of-life).

## 0.1.0 (2026-10-15)

- First release: `html2text()` with `base_url` support.
