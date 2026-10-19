# Linux test session — brief for a Claude Code session on the Linux machine

Load this in Claude Code (VS Code) on the Linux Mint 22 machine: *"Read `docs/linux-test-session.md` and start at
Phase 0."* It is written for an agent that has not seen this repository before.

## Where things stand

- The fork's `main` has the S3 sync backend merged (PRs #1–#3). It is unit-tested and was click-tested on
  macOS with two isolated copies of the app (2026-10-12 and 2026-10-13: `docs/manual-tests.md` tests 1–4,
  6.1–6.2, 8.2–8.5, 9, 10.1, 11.1, 11.3, 12 pass). **Nothing has run on Linux yet** — not the Rust tests, not
  the build, not the GUI.
- Read first, in this order: `CLAUDE.md` (rules), `TODO.md` ("Resume here" and the step 13.10 progress note),
  `LESSONS.md`, `docs/manual-tests.md` (§0 and the tests named below).
- Linux Mint 22.x is built on Ubuntu 24.04 (`noble`), but `lsb_release -c` gives Mint's own codename (`wilma` on
  22.0, `zena` on 22.3); `UBUNTU_CODENAME` in `/etc/os-release` says `noble`. Stock Ubuntu packages are fine; a
  third-party apt repository needs `noble` written into its sources line by hand.

## How this session works

- There is no desktop automation here: you prepare, the user clicks and types in the app, you check the log
  file and the app-data folder and report. **Never ask for a password** — the user types every password into
  the app themselves; they never go through chat, files or the clipboard.
- The bucket name and the credentials-file path are deliberately not in the repository (`TODO.local.md` is
  untracked and probably doesn't exist on this machine). Ask the user for them and keep them out of every
  committed file — this repository is public.
- Scratch folder only: **`submarine/sync-test`**. Never point a test copy at `submarine/sync` (the real folder)
  and never touch the real app's data in `~/.local/share/com.submarine.app`.
- Everything in `CLAUDE.md` applies: `src-tauri/src/lib.rs` is CRLF (edit it with the Edit tool only), never
  commit `Image*.png` / `TODO.local.md` / `PROMPT-*.md`, log through `applog`, use the app's words (saved
  login, server), and say what you ran versus what you only wrote.
- Not this session's job: catching up with upstream `SinaXhpm/Submarine`, or the macOS-only failures of
  `archive::tests::sh_runs::*`.

## Phase 0 — Toolchain

```sh
sudo apt update && sudo apt install -y build-essential curl file patchelf \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  libsoup-3.0-dev libjavascriptcoregtk-4.1-dev            # the list CI uses (.github/workflows/release.yml)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # then: . "$HOME/.cargo/env"
cargo --version      # Cargo.toml says rust-version = "1.90"; anything current is fine
node --version       # Vite 6.4 runs on Node 18, 20 or 22+; use a current LTS from nvm (apt's Node 18 is EOL)
npm ci
```

If `which cargo` fails in your shell, prefix commands with `PATH="$HOME/.cargo/bin:$PATH"` (same gotcha as
on the Mac — `CLAUDE.md`). Make `cargo` available to future shells too: `. "$HOME/.cargo/env"` in
`~/.profile` and `~/.bashrc` (rustup adds it to `~/.bashrc`; check `~/.profile`).

## Phase 1 — Automated tests, first run on Linux

```sh
( cd src-tauri && cargo test --lib )
npm run typecheck && npm run build
```

- There are 271 tests. On macOS 264 pass and 7 `archive::tests::sh_runs::*` fail ("Abort trap: 6" from the
  system `tar`/`unzip`, on upstream's own code too). **Expected here: 271 passed, 0 failed.** Either way,
  record the exact numbers in `TODO.md` (step 15.1). If the archive tests fail on Linux as well, that is new
  information — note it next to the macOS note; still don't fix them in this PR.
- The three live bucket tests are opt-in and self-cleaning; run them if the user's credentials file is on this
  machine: `SUBMARINE_S3_IT=1 SUBMARINE_S3_IT_CFG=<path> SUBMARINE_S3_IT_BUCKET=<bucket> cargo test --lib it_`
  (the `it_` filter also matches other tests; the three `it_…` names are the live ones).

## Phase 2 — Build and launch a test copy

`docs/manual-tests.md` §0 builds the two "devices" with `--bundles app`, which is macOS-only. On Linux build one
copy with its own identifier and no bundle, copy the binary out of `target/` under its own name, and run that:

```sh
npx tauri build --debug --no-bundle \
  --config '{"identifier":"com.submarine.e2e.a","productName":"Submarine E2E A"}'
cp src-tauri/target/debug/submarine <somewhere>/submarine-e2e-a   # Cargo's name, not productName
<somewhere>/submarine-e2e-a                                        # from a terminal, always by its path
```

**Always start a test copy by its path.** Tauri 2 doesn't rename the binary to `productName` (only
`mainBinaryName` in the config would), so every build is `src-tauri/target/debug/submarine`: the next build
overwrites it, and its name is the same as an installed Submarine's. If the `.deb` is installed here
(`/usr/bin/submarine`; check with `dpkg -l submarine`), a bare `submarine` starts the real app on the real data.

Run it from a terminal: the WebKit lines below go to stderr, not to the log file. The app already sets
`WEBKIT_DISABLE_HARDWARE_ACCELERATION`, `WEBKIT_DISABLE_DMABUF_RENDERER`, `WEBKIT_DISABLE_COMPOSITING_MODE`,
`GDK_BACKEND=x11` and `LIBGL_ALWAYS_SOFTWARE` on Linux unless they are already exported (`run()` in `lib.rs`),
so exporting them yourself changes nothing. What is Linux-only is the web-content sandbox (`webkit_sandbox.rs`):
note which `[webkit] web content sandbox: …` line appears. `ON (bubblewrap)` is expected where `/usr/bin/bwrap`
works; `OFF, <reason>` means it doesn't, and the app runs unsandboxed. If the window is blank or black, try once
with `SUBMARINE_DISABLE_WEBKIT_SANDBOX=1`; if that was needed, note it in `LESSONS.md` and the §0 recipe.

Check, and then write the verified Linux recipe into `docs/manual-tests.md` §0 next to the macOS one:

- the identifier override worked: app data is in `~/.local/share/com.submarine.e2e.a/` (not under
  `com.submarine.app`); the webview's own data lands under the same identifier (also look in
  `~/.config/com.submarine.e2e.a` and `~/.cache/com.submarine.e2e.a`);
- the log file is `~/.local/share/com.submarine.e2e.a/logs/submarine.log`, mode `0600` (`stat -c %a`), first
  line `INFO  [APP] Submarine <version> started (com.submarine.e2e.a)`; **Settings → Activity Log** shows that
  same path.

Second "device": the Mac copies from 2026-10-13 are gone. Cross-machine tests (§3, §4) need the user to rebuild
one on the Mac (§0 recipe there) — ask whether they want that, or build a second Linux copy with
`com.submarine.e2e.b`, copy it out as `submarine-e2e-b` the same way, and run the two side by side (never two
copies with the same identifier).

## Phase 3 — Manual tests on Linux (scratch folder)

The user drives the app; you give the click-through in the app's words (grep the component for the exact
label first — `LESSONS.md`), then check the log and the data folder after each step. Secrets grep is in §0.

| Test | What to run | What you check |
|---|---|---|
| §1 storage setup | credentials file path as the user keeps it on this machine (it may be `~/.s3cfg`); bucket; folder `submarine/sync-test` | `sync_backend.json` in the data folder has the path and no keys; both `[S3]` log lines; cloud bar shows the folder |
| §2 first sync | `e2e`, one server, a saved login linked to it, **Sync now** | `created profile`, `synced: pushed N`, `Cloud sync complete`; bucket objects if `s3cmd` is here |
| **§8.7 Linux case rule** | with `e2e` here, **New profile** `E2e`; **Import** under `E2e` | refused inline, no profile created (ext4 is case-sensitive — this is the test only Linux can run); `WARN  [PROFILE] Can't use …` if the backend was reached |
| §11.1 | unlock `e2e` with a wrong, then the right password | "Wrong password." and `WARN  [PICKER] opening 'e2e' failed: wrong password`; the right one opens `e2e` |
| §6.1, §6.2 | bad bucket name; missing credentials file (`~/…/missing` — `~` must expand on Linux too) | §6.1: a readable message, `BUCKET_NOT_FOUND` in the log. §6.2: panel and Activity Log say "Sync storage isn't set up: can't read …/missing …", the log has `ERROR [SYNC] … [S3] CREDENTIALS_FILE` |
| §10.1 | **Manual** proxy `http://127.0.0.1:9` → **Test connection** | four `retrying after: [S3] ENDPOINT_UNREACHABLE` lines then the failed line; back to **System** works. Also note (don't fix) whether an `http_proxy` / `https_proxy` variable in the launching shell changes the **System** result |
| §11.3 | **Refresh from cloud** with the missing credentials path | banner + `WARN  [PICKER] listing cloud profiles failed` |
| §3, §4 | only with a second device (see Phase 2) | as the doc says |
| §12 cleanup | **Delete from cloud…** per test profile (quit the other device first); secrets grep on the log; quit | `deleted cloud profile <id> (N records)`; bucket folder empty; then remove `~/.local/share`, `~/.config`, `~/.cache` `/com.submarine.e2e.*` |

For the per-password grep in §12, either put the real values in (and delete that line from the shell history
afterwards) or skip it — the pattern grep is the one that matters. Don't run it with placeholder text and
call it done; that happened twice on the Mac.

## Phase 4 — Linux-specific differences and fixes

Record every place the Linux run differs from the macOS run (wording, layout, behaviour, paths), then fix
only what is small and clearly Linux-only; everything else goes to `TODO.md` with a one-line description.
Places worth a look even if the tests pass:

- `~` in the credentials-file path (`expand_home` in `src-tauri/src/s3_store.rs`) and the path the picker
  shows when the file can't be read;
- the proxy choice reaching the S3 client (`cloud_config::build_client`) — on Linux reqwest's system proxy
  comes from environment variables, so a user who exports `https_proxy` may see different behaviour from
  the Mac;
- the log path shown in **Settings → Activity Log** and in `README.md`'s "Where are the logs?" (it claims
  `~/.local/share/<app id>/logs/`);
- the GTK file dialogs for **Import** / **Export** (the macOS run used the native sheet);
- the picker's row layout and fonts (only ever looked at on macOS);
- packaging for Phase 5: `npx tauri build --bundles deb` (the AppImage bundler downloads `linuxdeploy` and
  needs network; `.rpm` needs the `rpm` package — neither is needed for acceptance).

## Phase 5 — §13 acceptance with the real folder (only when the user says go)

This is the step that touches real data. Before anything else, back up the real app data on both machines
(`~/.local/share/com.submarine.app` here, `~/Library/Application Support/com.submarine.app` on the Mac) and
leave the old sync script's last object in the bucket as it is. Then a release build here
(`npx tauri build --bundles deb` → `sudo apt install --reinstall ./src-tauri/target/release/bundle/deb/*.deb`)
and on the Mac, both pointed at `submarine/sync`. The `--reinstall` matters when upstream's `.deb` is already
installed: the fork's package has the same name and, unless the version was bumped, the same version, so plain
`apt install` says it's already the newest version and installs nothing. Then restore each profile on the
machine that doesn't have it, edit on one and sync the other, and only then retire the old script. Don't use
**Delete from cloud** on a real profile during acceptance — a device that still has the profile open uploads it
again (open design item in `TODO.md`).

## Reporting and the PR

- `TODO.md`: tick the step 15 items as they really happen, add a progress note "Linux run (date)" in the
  style of the 13.10 note (what passed, what didn't, what wasn't run, polish seen), and update "Resume here".
- `docs/manual-tests.md`: the verified Linux build/launch/paths recipe in §0; the §8.7 outcome; anything in
  "Known open issues" that Linux changed.
- `LESSONS.md`: any gotcha that cost real time (WebKitGTK variables, Node on Mint, …).
- Branch from `main`, e.g. `test/linux-run`; subjects like `test(linux): …`, `docs(todo): …`, `fix(linux): …`;
  PR against `main` on `refwest24/Submarine` with `gh pr create` (if GraphQL is blocked on this network, the
  REST route works: `gh api -X POST repos/refwest24/Submarine/pulls --input <json>`). The user merges on
  GitHub.
- "Tests written" and "tests run" are different claims. Say which one each result is.
