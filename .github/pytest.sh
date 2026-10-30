#!/usr/bin/env bash
# Installs the wheel built into dist/ into a fresh virtualenv for each Python
# version given (or the default Python, with none) and runs the tests.
set -euo pipefail

for py in "${@:-}"; do
  rm -rf .venv-test
  uv venv -q ${py:+-p "$py"} .venv-test
  if [ -d .venv-test/Scripts ]; then python=.venv-test/Scripts/python.exe; else python=.venv-test/bin/python; fi
  uv pip install -q -p "$python" pyhtml2text --no-index --no-deps --find-links dist --reinstall
  uv pip install -q -p "$python" pyhtml2text pytest markdown-it-py html5lib
  "$python" -c 'import sys; print(sys.version)'
  "$python" -m pytest
done
