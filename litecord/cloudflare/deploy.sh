#!/usr/bin/env bash
# Puts the web app live from the latest main on GitHub. Normally GitHub does this by itself
# (.github/workflows/cloudflare.yml); this is the backup. The first time, wrangler opens the
# browser to sign in to the LiteCord Cloudflare account.
set -e
cd "$(dirname "$0")"
git fetch -q origin main
if [ -n "$(git status --porcelain -- .)" ]; then
  echo "You have changes in litecord/cloudflare that aren't committed. Commit and push them first."; exit 1
fi
git pull -q --ff-only origin main || { echo "Couldn't update from GitHub. Push or sort out your commits first."; exit 1; }
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
  echo "You have commits that aren't on GitHub yet. Push them first (git push)."; exit 1
fi
echo "Deploying $(git log -1 --format='%h %s')"
npx --yes wrangler@4.158.0 deploy
