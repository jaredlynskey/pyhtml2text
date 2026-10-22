# Vendored crates

## html5ever 0.39.0

Copied from the crates.io release (`src/`, licences, README and manifest;
examples, tests and benchmarks removed). Dual-licensed MIT / Apache-2.0,
see `html5ever/LICENSE-*`.

Patched with [servo/html5ever#800](https://github.com/servo/html5ever/pull/800)
by Josef Guenther ("Fix quadratic parsing of deeply nested markup", commit
`67c7e7c423c5d1acab1872c630cae61e2bc45211`), which is not merged yet. The
PR is against html5ever 0.40.1; its `src/` changes apply unchanged to
0.39.0, apart from line offsets. Like the rest of html5ever, they are
available under MIT or Apache-2.0.

Without it, tree building is quadratic in nesting depth (see
[servo/html5ever#788](https://github.com/servo/html5ever/issues/788)): most
start and end tags search the stack of open elements from the top, and on
deeply nested input each search walks the whole stack. The PR keeps the
positions of open elements by name and by the sets those searches look for,
so "in scope" checks and the other lookups are O(1). Two cases stay
quadratic: the adoption agency removing elements from the middle of the
stack, and searches of the list of active formatting elements.

This replaces an earlier, narrower patch of ours that only covered the
"`p` in button scope" check. That patch left unmatched end tags and end tags
in foreign content quadratic: at 32,000 levels, `html2text()` took 3.4 s on
nested `<span>` followed by unmatched `</em>`, and 4.5 s on SVG `<g>`
followed by unmatched `</x>`. Both now take 0.02 s or less.

Checked against stock 0.39.0 before vendoring:

- html5ever's own tests pass, including the PR's new test, and so do all
  3,515 html5lib tree-construction tests, in release and debug builds.
- The tree, quirks mode and exact parse errors are identical to stock 0.39.0
  on every html5lib input (documents and fragments), 300,000 random tag-soup
  records (30% of them fragments, in 23 contexts) and 28 real pages,
  including the single-page HTML standard.
- pyhtml2text's output is byte-for-byte unchanged on the benchmark corpus
  (`bench/bench.py --check`).

The extra bookkeeping costs about 0.4 µs per call, and 0–2% on real pages.

Once the PR is in an html5ever release, move the `html5ever` dependency in
`Cargo.toml` to it and drop this directory and the `[patch.crates-io]`
entry. Upgrading html5ever without updating this copy silently drops the
patch (cargo only warns that it is unused), so `test_deep_nesting_is_linear`
checks that it is still in effect.
