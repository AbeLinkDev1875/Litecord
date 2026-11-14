# LiteCord: notes for Claude

LiteCord is a small Discord-style app for a group of friends. Two people work on it, each with
their own Claude, so keep `main` working and never rewrite its history.

## What's where

- `litecord/cloudflare/`: the web app and its server, hosted on Cloudflare Workers.
  - `public/index.html` is the whole web app (one big file).
  - `src/worker.js` is the server: Durable Objects `Room` (a server or call room, `/relay`)
    and `Inbox` (each person's mailbox for friends and DMs, `/inbox`).
- `litecord/desktop/`: the desktop app, in Rust with GPUI. Its `README.md` explains how it's
  laid out. It talks to the same Cloudflare server.
- `litecord/old/`: the old Python version. Not used.
- `.github/workflows/desktop.yml` builds the desktop app for Linux and Windows on every push
  that touches it. `release.yml` publishes releases (see below).

The web app and the desktop app speak the same protocol: mailbox messages (`freq`, `facc`,
`dm`, `typing`, `ring`, ...), room messages and call signaling. If you change a message in
`index.html` or `worker.js`, change the desktop app to match (`desktop/src/store.rs`,
`desktop/src/call/room.rs`, `desktop/src/call/media.rs`), or browser and desktop users stop
seeing each other. Old clients stay in use for a while, so prefer adding fields over renaming
or removing them.

## Working on it

Use git, not GitHub's "upload files" page, and don't commit zip files of the project.

```
git pull            # always first: the other person may have pushed
# ...make changes...
git add -A && git commit -m "Say what changed"
git pull --rebase && git push
```

Setup on Windows: Git, the GitHub CLI (`gh auth login`), Node.js LTS (for Cloudflare's
`wrangler`), and for the desktop app Rust (rustup, from https://rustup.rs) with the
Visual Studio Build Tools' "Desktop development with C++". The first desktop build takes a
while, because it compiles GPUI and downloads a prebuilt libwebrtc.

- Desktop app: `cd litecord/desktop`, then `cargo run --release`. `cargo test` runs the tests.
- Web app locally: `cd litecord/cloudflare`, then `npx wrangler dev`.

## Putting the web app live

`.github/workflows/cloudflare.yml` deploys the web app to Cloudflare by itself whenever files
in `litecord/cloudflare/` change on `main`. So pushing to `main` is putting it live: everyone
using LiteCord gets the change right away. Test in a browser (`npx wrangler dev`) before you
push. After pushing, check it worked: `gh run list --workflow cloudflare.yml --limit 1`.

It needs the repository secrets `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. If the run
says they're missing or the token is wrong, ask the person to set them. Never ask them to paste
a token into the chat. To deploy `main` again without a change: `gh workflow run cloudflare.yml`.

Backup, if GitHub Actions is down: `litecord/cloudflare/deploy.cmd` (Windows, double-click it)
or `deploy.sh`. They update from GitHub, refuse if there are changes or commits that aren't on
GitHub yet, then run `wrangler deploy` (it asks to sign in to Cloudflare the first time). Don't
run `wrangler deploy` straight from a folder that isn't up to date with `main`. It would
replace the site with that older copy and take the other person's work off it.

## Releasing the desktop app

Releases are made by `.github/workflows/release.yml`. Never make a release by hand on GitHub's
website, and never move, reuse or delete a published tag.

1. Pick the next version, `X.Y.Z`: bump Z for fixes, Y for new features. The current version is
   `version` in `litecord/desktop/Cargo.toml`. Check `gh release list` and `git tag` so you
   don't reuse one.
2. Set `version = "X.Y.Z"` in `litecord/desktop/Cargo.toml`. Run `cargo build` once so
   `Cargo.lock` picks it up.
3. Commit, push to `main`, and wait for the "Desktop app" workflow to pass:
   `gh run list --workflow desktop.yml --limit 1`, then `gh run watch <id>`.
4. Tag that same commit and push the tag:
   ```
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
5. Watch the "Release" run: `gh run list --workflow release.yml --limit 1`, then
   `gh run watch <id>`. When it passes, `gh release view vX.Y.Z` shows the release with
   `LiteCord-X.Y.Z-windows-x86_64.exe`, `LiteCord-X.Y.Z-linux-x86_64.tar.gz` and `SHA256SUMS.txt`.

The workflow refuses to publish if the tag isn't exactly `v` + the Cargo.toml version, if the
tagged commit isn't on `main`, or if a release with that name already exists.

If a release run fails, nothing was published, so you can redo the tag:
`git push --delete origin vX.Y.Z` and `git tag -d vX.Y.Z`, fix it, push to main, and tag again.
Once a release is published, don't redo it. Make a new version instead.

To test the release builds without publishing anything, run "Release" from the Actions tab
(or `gh workflow run release.yml`). It does the same checks and builds but makes no release.

Old tags `Release`, `v1.0`, `v1.1` and `v2.0` are from before this workflow. Leave them alone.

The web app goes live from `main` on its own, before the release is tagged. New desktop
features that need server changes will find them already there.

## Style

Code comments and docs are short and plain. Match the code around you. The desktop UI copies
Discord's look and the web app's layout. Check both themes and a few zoom levels.
