# Vendored crates

## html5ever 0.39.0

Copied from the crates.io release (`src/`, licences, README and manifest;
examples, tests and benchmarks removed). Dual-licensed MIT / Apache-2.0,
see `html5ever/LICENSE-*`.

Patched (see git history of this directory) so that checking for an open `<p>`
element "in button scope" is O(1) instead of walking the whole stack of open
elements. That check runs for most block-level start tags, so deeply nested
input (e.g. tens of thousands of nested `<ul>`/`<div>`/`<blockquote>`) took
quadratic time to parse. Parse results are unchanged.
