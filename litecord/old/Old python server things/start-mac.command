#!/bin/bash
# Double-click to start LiteCord. (First time: right-click > Open, because macOS blocks downloaded scripts.)
cd "$(dirname "$0")"
if command -v python3 >/dev/null 2>&1; then
  python3 server.py "$@"
else
  echo "LiteCord needs Python 3. Get it free from https://www.python.org/downloads/"
  read -r -p "Press Enter to close."
fi
