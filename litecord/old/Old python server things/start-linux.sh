#!/bin/sh
# Start LiteCord. Run: ./start-linux.sh   (or double-click and choose "Run in terminal")
cd "$(dirname "$0")"
if command -v python3 >/dev/null 2>&1; then
  exec python3 server.py "$@"
else
  echo "LiteCord needs Python 3. Install it with your package manager, e.g.: sudo apt install python3"
fi
