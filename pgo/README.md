# PGO training data

`train.py` is run against an instrumented build in CI to produce a profile for
profile-guided optimisation (about 25% faster on pages outside this set).

The pages in `corpus/` are unmodified (gzipped) copies of:

- Python documentation pages from https://docs.python.org/3/ —
  Copyright © 2001 Python Software Foundation, licensed under the Python
  Software Foundation License Version 2 (see the notice in each page).
- RFC 3986, RFC 8259 and RFC 9110 from https://www.rfc-editor.org/ —
  Copyright © IETF Trust and the persons identified as the document authors,
  reproduced in full as permitted by the IETF Trust Legal Provisions.

They are only used at build time and are not included in the wheels or sdist.
