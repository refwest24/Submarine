# Working in this repository

Submarine is a Tauri 2 desktop app: a Rust backend in `src-tauri/` and a React + TypeScript UI in `src/`.
This fork adds personal-profile sync through an S3-compatible bucket. The plan, decisions and progress live
in `TODO.md` (start at its "Resume here" section); hard-won gotchas are in `LESSONS.md`; the click-through
test procedure is `docs/manual-tests.md`. On the Linux machine, `docs/linux-test-session.md` is the brief for
the test session there (toolchain, how to build a test copy without bundling, which tests only Linux can run).

## Toolchain and checks

- Rust comes from rustup. In a shell started by an agent, `cargo` may not be on `PATH`; if `which cargo`
  fails, prefix commands with `PATH="$HOME/.cargo/bin:$PATH"`.
- Before claiming a change works, run all of:
  ```sh
  ( cd src-tauri && cargo test --lib )          # offline unit tests
  npm run typecheck && npm run build            # npm ci first after a dependency change
  ```
- Live tests against a real bucket are opt-in and self-cleaning:
  `SUBMARINE_S3_IT=1 SUBMARINE_S3_IT_CFG=<s3cmd config> SUBMARINE_S3_IT_BUCKET=<bucket> cargo test --lib it_`
  (the `it_` filter also matches other tests whose path contains it; the three `it_…` names are the live ones).
- On macOS, upstream's `archive::tests::sh_runs::*` fail with "Abort trap: 6" from the system `tar`/`unzip`,
  on upstream's own code too. Not related to sync; don't try to fix them in a sync PR.
- The GUI can't be driven from a browser (no Tauri IPC) and `tauri-driver` has no macOS support. Two isolated
  copies of the app are built per `docs/manual-tests.md` §0 and driven by hand or with desktop automation.

## Rules that have bitten before

- **`src-tauri/src/lib.rs` uses CRLF line endings** and ends without a final newline. Never round-trip it
  through a text-mode script; edit with a tool that preserves bytes, or in binary mode keeping `\r\n`. Check
  with `file src-tauri/src/lib.rs` ("with CRLF line terminators") and `git diff --stat` after any scripted
  edit — a rewrite shows as a diff of every line. Everything else in the repo is LF.
- **Never commit** `Image*.png` in the repo root (screenshots of a private bucket and host), `TODO.local.md`,
  or `PROMPT-*.md`; they're local-only on purpose. Keep bucket names, hosts and LAN addresses out of
  committed files — this repository is public.
- Log through `applog::{info,warn,error}("AREA", msg)`, never `eprintln!`. Errors from `sync_backend.rs`
  already carry a `[S3]` tag; when logging them under area `S3`, strip it (see `log_error` there) or the
  tag shows twice. Every log line is redacted for secrets, but don't rely on that: never log a password,
  key, token or signature at all.
- Write user-facing steps in the app's words, not the code's: a *credential* is a **saved login**, a *node*
  is a **server** (`docs/manual-tests.md` has the table). Grep the component for the exact label first.
- Profile names: kept exactly as typed, matched exactly, but a new name that matches an existing one in any
  letter case is refused on every platform (`ensure_profile_name_free`; decision 8 in `TODO.md`). A personal
  profile on S3 needs no sync-server sign-in and is tied to no server (decision 7).

## Conventions

- Commit subjects follow the existing history: `feat(sync): …`, `fix(ux): …`, `docs(todo): …`.
- Record what you verified and what you didn't in `TODO.md`'s progress notes; "tests written" and "tests
  run" are different claims.
- Upstream is `SinaXhpm/Submarine`; this fork merges it in with merge commits, never a rebase or force push.
