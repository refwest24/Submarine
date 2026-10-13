# Project Plan — Built-in S3 sync backend

Branch `feat/s3-sync-backend`. Machine-specific details (bucket name, machines, local paths) live in
`TODO.local.md`, which is not committed.

Goal: personal profiles sync record-by-record through an S3-compatible bucket (Linode Object Storage, prefix
`submarine/sync`) so the user's Mac and Linux machine converge without the old whole-file `s3cmd` sync script.
Credentials come from the s3cmd-format file `~/.config/submarine-sync/s3cfg` (read at sync time, never copied).
Shared profiles stay on the HTTP cloud.

- [x] 0. Setup: branch, rustup (no PATH edits), `npm ci`, baseline `cargo test --lib` + `npm run build`
- [x] 1. Engine: `sync_fk_pending` (late-arriving FK repair), per-uuid max in `profile_sync_stats`
- [x] 2. Transport seam `sync_backend.rs` + `HttpTransport`; swap 6 call sites in `lib.rs` (HTTP traffic unchanged)
- [x] 3. `ObjectSync` over `ObjectStore` + `MemStore`: key codec, push/pull planner, escrow/meta, GC, list counts
- [x] 4. `S3Store` + s3cfg reader + error mapping; integration tests vs Linode (throwaway prefix)
- [x] 5. Commands: `sync_backend_get`/`set`, `s3_test_connection`; `personal_sync_ready` in share status
- [x] 6. Frontend: `SyncStorageSection` (CloudPanel + Settings), gating, error tags
- [ ] 7. E2E: two-copy run on scratch prefix `submarine/sync-test` ✔ (2026-10-12, see below); Mac + Linux acceptance still to do
- [x] 8. Docs: README section, `cloud.rs` header, Review below (done ahead of step 7; revisit after E2E)

- [x] 9. Case-sensitive profile names (same PR; found in E2E — see "Found in E2E" below)
  - [x] 9.1 Stop OS auto-capitalisation: `autoCapitalize="off"`, `autoCorrect="off"`, `spellCheck={false}` on
        every profile-name input (create, import)
  - [x] 9.2 Picker matches local ↔ cloud by EXACT name; never merges two cloud profiles; if several cloud
        profiles share one exact name, none is attached to the local row, so "Delete from cloud" can't hit
        the wrong one
  - [x] 9.3 One Rust helper for "is this name free?" used by create / import / restore / accept-invite:
        exact match → "already exists"; case-only match → policy below, with a message that says why
  - [x] 9.4 Rust unit tests for the helper; `cargo test`, `npm run typecheck`/`build`; rebuild E2E copies
  - [x] 9.5 Manual tests for 9.1–9.3 added to the manual-test doc (step 10, test 8)
- [x] 10. Manual-test doc `docs/manual-tests.md` (generic wording — repo is public): automated commands, the
      full S3 E2E checklist incl. steps not yet run, profile-name cases, error paths, cleanup, Linux acceptance
- [x] 12. Error/warning log file, secrets kept out (approved 2026-10-05: small built-in logger, no new crates, same PR, before resuming E2E)
  - [x] 12.1 Small logger: append to `submarine.log` in the OS log folder (`app_log_dir`: macOS
        `~/Library/Logs/<identifier>/`), `<UTC time> <LEVEL> [AREA] message`, rotate at 1 MB (keep one old file),
        never fails the caller
  - [x] 12.2 Redaction on every line (defence in depth): PEM private-key blocks, `Authorization`/`Bearer`,
        SigV4 `Signature=`/`Credential=`, `password|passphrase|secret_key|access_key|token=…`, long hex/base64
        runs → `[redacted]`; unit tests feed real-looking secrets and assert none survive
  - [x] 12.3 Remove the SSH "Key Header: <first line of private key>" debug line (log the key *type* only);
        route the 18 `eprintln!` and the session/connection `println!` lines through the logger
  - [x] 12.4 New log points the manual tests need: sync start/finish (backend, pushed/pulled/skipped, time) and
        failures; S3 region switch, test-connection result, config load/save errors; restore OK / wrong
        password; name refused (case clash); picker cloud-list failure (now silent); auto-sync failures
  - [x] 12.5 Frontend → file: an `app_log` command so Activity Log warnings/errors and the errors shown by the
        picker, Cloud panel, Profile panel and storage settings are written too (same redaction);
        Settings → Activity Log shows the log file's path
  - [x] 12.6 Docs: README FAQ "Where are the logs?"; `docs/manual-tests.md` gets "Log shows…" per test and a
        secrets check (`grep -iE 'PRIVATE KEY|secret_key|Signature=|Bearer ' submarine.log` finds nothing)
  - [x] 12.7 Verify: unit tests (redaction, rotation), 72+ Rust tests, typecheck/build, rebuild E2E copies,
        then the manual tests with the log open
- [x] 11. Resume E2E (step 6 error paths onward) on the rebuilt copies, then cleanup (re-run from scratch 2026-10-12)
- [ ] 13. Bring in upstream `SinaXhpm/Submarine` main (`75678e5`, 77 commits past v0.3.8) — same PR (#1 on the fork)
  - [x] 13.1 Merge upstream main into this branch (merge, no rebase/force push); resolve the 8 conflicts
        (Cargo.toml/lock, lib.rs, tunnel.rs, CloudPanel, ProfilePanel, ProfileSelectPage, SettingsPanel)
  - [x] 13.2 Build fixes: `HttpTransport` carries upstream's `cloud::Session` (#121); `hmac` 0.13 (sha2 0.11);
        keep `async-trait` as a direct dependency; take upstream's lock and let cargo add only edges
  - [x] 13.3 S3 without sign-in (decision 7, user 2026-10-12): personal profiles on S3 need no cloud session
        and skip upstream's per-profile server link (`check_profile_server` / `link_profile_server`, the
        "syncs elsewhere" warning, Move here); sync / restore / force push / stats / list / delete check the
        S3 settings and use S3. Shared profiles and the HTTP backend keep upstream's behaviour
  - [x] 13.4 Picker on upstream's redesign: one row identity (`row.key`) for selection, the ⋯ menu and React
        keys; exact-name local ↔ cloud linking; storage-aware cloud bar; log-file calls inside upstream's
        error places; `cleanErr` strips `[S3]`
  - [x] 13.5 Names (decision 8, user 2026-10-12): refuse a new profile whose name matches an existing one in
        ANY letter case, on every platform — create / import (incl. upstream's Android `import_profile_bytes`)
        / restore / accept-invite, local and cloud names; replaces the 10-05 "allowed on Linux" rule
  - [x] 13.6 S3 client honours the Cloud panel's proxy choice (upstream turned on reqwest `system-proxy`)
  - [x] 13.7 Upstream's new `eprintln!`/`println!` lines through applog; `[SYNC] OTHER_SERVER` message
        without its code; README sentences made untrue by upstream (sync server wording, telemetry FAQ,
        portable log folder, s3cfg not carried by portable mode)
  - [x] 13.8 Tests: unit tests for the new name rule and the session-free S3 paths; `cargo test` (incl. live
        S3 ITs), typecheck, build; manual-test doc revised (names, sync without sign-in, picker ⋯ menu,
        proxy, self-hosted server coexistence, portable log location)
  - [x] 13.9 Fast-forward the fork's `main` to upstream (`refwest24/Submarine` only), push this branch;
        PR #1 shows only this branch's changes
  - [x] 13.10 Key manual checks re-run on fresh copies of the merged code (2026-10-13, after PR #2): tests 1–4,
        6.1–6.2, 8.3–8.5, 9, 10.1 (connection test), 11.1, 11.3, 12 pass — see the progress note; three
        polish items added to the list below

- [ ] 14. Test review after the upstream merge (2026-10-13, branch `fix/test-review-after-upstream-merge`, PR #2)
  - [x] 14.1 `docs/manual-tests.md` corrected where it disagreed with the code: §9.4 (HTTP + signed out hides
        **Sync now**, no "not signed in" toast), §10.1 (dead proxy → `ENDPOINT_UNREACHABLE` after retries, not
        `NETWORK`; the **Change** step and the **None** proxy choice), §10.3 (the real warning text; restored
        profiles don't link), the secrets grep (it matched the `[redacted private key]` placeholder and four
        harmless SSH lines, so it failed exactly when redaction worked), the `it_` filter note, log-level
        spacing in every quote (`INFO  [`, `WARN  [`), §1 and §6.3 details, the log-area list
  - [x] 14.2 Code: `handleCloudSync` classifies `[S3]` errors before the sign-in check and matches `401` as a
        word (a bucket error carrying "401" inside an id or path was reported as "not signed in"); config
        load/save errors no longer log as `[S3] [S3] …`; the applog fixture uses the decision-8 message
  - [x] 14.3 Tests written (not yet run — see 14.5): the server gate skips the link for a bucket and leaves
        the row alone (`gate_profile_server`, lib.rs); `personal_sync_ready` keeps S3 ready with an unreadable
        credentials file; a missing / key-less credentials file is a `CREDENTIALS_FILE` error from
        `s3_transport`; `expand_home`; restore refusals vs faults (`restore_was_refused`). The first, second
        and last needed a small pure helper split out of the command code
  - [x] 14.4 `CLAUDE.md` added for agent sessions (toolchain, CRLF rule, test commands, what never to commit)
  - [x] 14.5 Run `cargo test --lib` on the branch (the review ran from a sandbox without Rust): 264 passed,
        7 failed — all `archive::tests::sh_runs::*` (upstream's macOS `tar`/`unzip` failures), so the 5 new
        tests are green; `npm run typecheck`/`build` clean (user, 2026-10-13)
  - [ ] 14.6 Still untested after this (next, in order): region auto-discovery and 503 retry/backoff and
        `NETWORK` vs `ENDPOINT_UNREACHABLE` (need a local mock HTTP server); S3 client goes through the Cloud
        panel proxy (only `build_client` is tested); `list_profiles` skipping empty partitions and the 8 MiB
        body cap; the name rule at its 5 call sites; escrow kept across GC and removed with the profile;
        `profile_sync_stats` comparable flag; `view()` with a missing file. Frontend: no test runner at all —
        decide whether to add vitest for `buildRows`, `nameProblem`, `handleCloudSync`, `cleanErr`

- [ ] 15. Linux run (Mint 22) — brief for the session: `docs/linux-test-session.md`
  - [ ] 15.1 Toolchain; `cargo test --lib` (first run on Linux: do the 7 macOS archive failures pass here?),
        typecheck, build
  - [ ] 15.2 Test copy built with `--no-bundle` + identifier override; data/log paths verified; the Linux
        recipe written into `docs/manual-tests.md` §0
  - [ ] 15.3 Manual tests on the scratch folder: §1, §2, §8.7 (the Linux-only case test), §11.1, §6.1–6.2,
        §10.1, §11.3, §12; §3–§4 if a second device is available
  - [ ] 15.4 Linux-specific differences recorded; small Linux-only fixes made
  - [ ] 15.5 §13 acceptance with the real folder on the Linux machine and the Mac (backups first)

Decision (9.3, user 2026-10-05), **superseded 2026-10-12**: names that differ only in case were allowed where the
disk is case-sensitive. Now (decision 8, step 13.5): a new name matching an existing profile in any letter case
is refused on every platform; names are still kept and matched exactly as typed.

## Progress notes

- **Step 1–2:** engine fix + transport seam; HTTP wire traffic unchanged (6 call sites swapped).
- **Step 3–4 deviation:** MinIO images are no longer public → integration tests run against Linode under
  throwaway `submarine-it/<id>/` prefixes (user approved). `rusty-s3` dropped: Linode's gateway rejected its
  presigned *listing* requests (`403 failed authentication`), so requests are now SigV4 **header**-signed by
  `sigv4.rs` (matches AWS's published example signatures). Linode wants region `default` — discovered
  automatically from the error, like s3cmd. Added retries with backoff (Linode answers bursts with
  `503 SlowDown`) and a per-store HTTP client. Net: no new crates.
- **Step 5–6:** commands + UI; `npm run typecheck`/`build` clean; 71 Rust tests pass, incl. the 3 Linode ITs (`it_*`; skipped without `SUBMARINE_S3_IT=1`).
- **Step 7 (paused 2026-10-02):** not started. The GUI can't be tested from Chrome/Playwright (no Tauri IPC in
  a plain browser; `tauri-driver` has no macOS support), so it will be driven with desktop computer use from
  the Claude desktop app. Nothing launched yet; no E2E app data exists; the bucket holds only the user's
  existing objects. Branch pushed to origin (no PR).
- **2026-10-05:** E2E copies A/B rebuilt from `834738c` (location in `TODO.local.md`); still not launched.
- **Step 12 (2026-10-05):** `applog.rs` writes `submarine.log` (OS log folder, 0600, 1 MB rotation, one old
  file) with a redaction backstop; sync, restore, cloud delete, profile create/import/remove, name refusals,
  storage settings, connection tests, S3 retries/region switches, SSH session steps and the Activity Log all
  land there; the picker's previously silent cloud failures are logged. The SSH debug line that echoed the
  private key's first line now names the format only. Every log call reviewed for secrets. 75 Rust tests
  (3 new: redaction, kept-content, rotation/0600), typecheck/build clean; E2E copies rebuilt (v3) and the log
  verified on launch.
- **Step 9 (2026-10-05):** profile names are case-sensitive. Name inputs no longer let macOS auto-capitalise;
  the picker links local ↔ cloud by exact name and gives every row its own key (`local:<name>` /
  `cloud:<id>`), so neither a case variant nor two same-named cloud profiles can make "Delete from cloud" hit
  the wrong profile; `ensure_profile_name_free` replaces the four "already exists" checks and explains a
  case-only clash on disks that ignore case. Unit test passes on this Mac (ignores case) and on a temporary
  case-sensitive APFS image (Linux behaviour). 72 Rust tests, typecheck, build clean; E2E copies rebuilt (v2).
- **E2E resumed (2026-10-12):** §6.1 bad bucket → readable `BUCKET_NOT_FOUND` (panel + log) ✔; §8.2 Get `e2e` on
  B refused with the case-clash message (log) ✔. §6.2 failed: with an unreadable credentials file the Profile
  panel hid **Sync now** and said "S3 sync isn't set up yet", so no specific error appeared anywhere. Fixed:
  S3 counts as set up once a bucket is set (like HTTP = signed in), so the sync runs and fails with the
  readable `CREDENTIALS_FILE` error; the panel now shows sync failures itself, not only the Activity Log.
  75 Rust tests, typecheck, build clean; E2E copies need a rebuild before re-running §6.2.
  Also seen: B's `E2e` (no servers) synced but stayed **This device** — the picker hides cloud profiles with no
  live items (`live_records > 0`), so test profiles need a server each (noted in manual-tests §8.3). Restore
  now logs a name clash as WARN (like a wrong password), not ERROR. `npm run typecheck` clean after both
  changes; `cargo test` + rebuild still to run on the Mac. Decision (user, 2026-10-12): restart step 7 from
  scratch on fresh copies with an empty `submarine/sync-test`.
- **Step 7 re-run from scratch (2026-10-12, fresh copies built 13:14 incl. the fixes above):** §1 storage
  setup A+B ✔ (folder box defaults to `submarine/sync` — had to change it to the scratch folder); create
  `e2e` + saved login + linked server, sync ✔; B lists it In cloud ✔; wrong password → wrong-password
  message (WARN) ✔; restore → server + login link present ✔; edit on B → A ✔; delete on A → B, "in sync" ✔;
  §6.1 bad bucket ✔; §6.2 missing credentials → readable error in panel + Activity Log + both log lines ✔;
  §8.2 create `E2e` beside `e2e` refused ✔, Get `e2e` beside `E2e` refused (now WARN) ✔; §8.3 `E2e` Synced +
  `e2e` In cloud as two rows ✔; §8.4 Delete `e2e` from cloud left `E2e` alone ✔; §9 both deleted, log
  secrets grep empty ✔ (55 lines, no `[redacted]` hits needed). Not run: §8.1 auto-capitalise with the
  macOS setting on, §8.5 same-name `dup`, §7 concurrent edits.
  Cleanup done 2026-10-12: bucket `submarine/sync-test/` empty, both copies' app data and logs removed. The
  per-password log grep was run with a literal placeholder, so it didn't check the real test passwords —
  do it properly on the next run (the pattern-based secrets grep did pass).
- **Found in the re-run (open):**
  - *Delete from cloud is undone by any device that still has the profile open:* A's auto-sync re-uploaded
    `e2e` 15 s after B deleted it (new partition, `pushed 2`). The confirm dialog says copies on devices
    are not touched, but not that they will re-publish. Needs a design call (tombstone the partition /
    warn in the dialog / stop auto-sync for a deleted id).
  - *Polish:* Activity Log says "Node added/updated" (UI word is server); the CREDENTIALS_FILE banner says
    fix it "from the Cloud panel on the profile picker" though Settings → Cloud Sync works too; a stale
    "bucket doesn't exist" message stays under the form after the bucket name is corrected; Settings shows
    the can't-read warning and a green "Saved — personal profiles now sync" together; the picker hides
    cloud profiles with no live items, so an empty profile never shows In cloud. From 13.10: the picker's
    "Couldn't load your cloud profiles" banner shows the `CREDENTIALS_FILE:` code and outlives the fix (it
    stays until another banner replaces it); the dead-proxy message is reqwest's raw error chain rather than
    "can't reach the endpoint through the proxy".
- **Step 13.10 — re-run on the merged code (2026-10-13, copies rebuilt from `e3cc909` = PR #2 merged, app data
  and `submarine/sync-test/` empty at the start):** §1 storage setup A+B (connection test OK, bar shows the
  folder) ✔; §2 `e2e` on A with a saved login and a linked server, each change pushed (`pushed 1`), **Sync now**
  "Cloud sync complete" ✔; §3 Get on B: wrong password → `ESCROW_UNSEAL_FAILED` as WARN, right password →
  `restored 'e2e' … pulled 2`, servers + login link present ✔; §4 edit on B → A `pulled 1`, delete on A → B
  `pulled 1` (counts match in both logs) ✔; §6.1 `<bucket>-nope` → `BUCKET_NOT_FOUND` ✔; §6.2 missing
  credentials file → panel and Activity Log say "Sync storage isn't set up: can't read …/missing … — fix it in
  Settings → Cloud Sync." (the 14.2 message), `ERROR [SYNC] … [S3] CREDENTIALS_FILE` in the log ✔; §9.4 storage
  on **Sync server** while signed out: **Sync now** hidden, "Not signed in — lock this profile…" shown, auto-sync
  logs `[CLOUD] NOT_SIGNED_IN` twice; back on **S3 bucket** `E2e` syncs signed out (`pushed 1`) ✔ (§9.1–9.3 are
  covered by §3, §12 and this); §8.3 Get `e2e` beside `E2e` refused with the decision-8 message, both WARN lines
  ✔; §8.4 `E2e` **Synced** + `e2e` **In your cloud** as two rows ✔; §8.5 Delete `e2e` from cloud (7 records, A
  quit first) left `E2e` synced ✔; §10.1 Manual proxy `http://127.0.0.1:9` → four `retrying after: [S3]
  ENDPOINT_UNREACHABLE` lines then the failed connection-test line, proxy back → connection OK ✔ (the **Sync
  now** half wasn't run: the profile was locked); §11.3 Refresh from cloud with the missing credentials path →
  "Couldn't load your cloud profiles" banner + `WARN [PICKER] listing cloud profiles failed` ✔; §11.1 wrong
  password on **Unlock** → "Wrong password." + `WARN [PICKER] opening 'E2e' failed: wrong password` ✔; §12
  Delete `E2e` from cloud (3 records) ✔, pattern secrets grep over both logs empty (83 lines) ✔; both copies
  quit. Cleanup confirmed (user): `s3cmd ls -r` of the test folder prints nothing, e2e app data and logs
  removed. The per-password grep was run with the placeholder text again, so it proved nothing — but both
  logs were read in full (83 lines) before deletion and every line is one of the structured messages the doc
  quotes, with no free-text field that could hold a password or key. Next run: substitute the real values
  before pressing Enter, or skip that grep (the pattern grep is the one that matters). Not run this time: §5, §6.3, §7, §8.1, §8.2 (ran 10-12),
  §8.6–8.7, §10.2–10.3, §11.2's right-click/keyboard paths, §13.
  Polish seen (added to the list below): the picker's "Couldn't load your cloud profiles" banner keeps the
  `CREDENTIALS_FILE:` code (`cleanErr` strips `[S3]` only) and stays on screen after the path is fixed and
  saved, until some other message replaces it; the dead-proxy error shows reqwest's raw chain ("error sending
  request: client error (Connect): tunnel error: … Connection refused (os error 61)").
- **Found in the test review (2026-10-13, open):**
  - *A profile restored from the bucket has no `cloud_server` row.* Restore links a server only when the
    transport has one (`transport.server()`), and `profile_server()` reads a missing row as "linked to Submarine
    Cloud". Switch such an install to a self-hosted sync server and that profile refuses with `OTHER_SERVER`
    instead of linking on its first sync like a profile created on the device. Fix candidates: write an empty
    `cloud_server` row on restore (= "link on first HTTP sync"), or treat a missing row as unlinked when the
    profile has `cloud_profile` set.
  - *Decision 8 isn't enforced against empty cloud profiles:* the picker filters `live_records > 0` before the
    name check, and the backend checks the local disk only. An empty cloud `E2e` doesn't block a new local `e2e`.
    Either check names against the unfiltered cloud list, or have the backend list the bucket's names on create.
- **Step 13 (2026-10-12): upstream merged in.** Upstream main `75678e5` (77 commits) merged into this branch
  (merge commit, no rebase). Conflicts: Cargo.toml/lock (upstream's lock + 5 edges, no new crates; `hmac` 0.13
  for sha2 0.11; `async-trait` kept as a direct dep), lib.rs (19 blocks, CRLF kept), tunnel.rs, and four UI
  files. Sync: `HttpTransport` carries upstream's `cloud::Session`; `SyncTransport::server()` is the server a
  transport is tied to (`None` for S3); `sync_with(…, session: Option<&Session>)` needs a sign-in only for a
  shared profile or the HTTP backend, and runs upstream's `check_profile_server`/`link_profile_server` only
  when there is a server (decision 7); restore / force push / stats / list / delete take the same route;
  every sync is logged from `sync_with`. ProfilePanel hides upstream's "syncs with another server" warning
  and Move button on S3. Picker: rebuilt on upstream's redesign (#123) with one row identity (`row.key`) for
  selection, the ⋯ menu and React keys, exact-name linking, S3 cloud bar, log-file calls in upstream's error
  places; import also refuses cloud names that differ only in case. Names (decision 8):
  `ensure_profile_name_free` refuses any-case matches on every platform (also upstream's Android
  `import_profile_bytes`). S3 client built by upstream's `cloud_config::build_client` (Cloud panel proxy; none
  for a loopback endpoint). Upstream's new prints and the portable-mode decision go to the log; README
  sentences fixed (sync server wording, telemetry FAQ incl. the GitHub update check, portable logs/s3cfg).
  Verified: `cargo check --all-targets`; `cargo test --lib` 258 passed incl. the 3 live S3 ITs and 2 new tests
  (`a_profile_name_taken_in_any_letter_case_is_refused`, `an_s3_transport_needs_no_sign_in_and_no_server`);
  8 failed — all upstream `archive::tests::sh_runs::*` (#120), "Abort trap: 6" from macOS `tar`/`unzip`; the
  same tests fail on pristine upstream `75678e5` too (9 of 12), so not caused by the merge. `npm ci` (upstream
  moved to Vite 6), typecheck, build clean. Manual-test doc: tests 8 (names), 9 (bucket sync signed out),
  10 (proxy, self-hosted server), 11 (picker rows/menu) new or rewritten; cleanup is now 12, acceptance 13.
- **Step 8 (2026-10-05):** README gains "Sync to your own S3 bucket" (s3cfg format, setup, bucket layout, what
  the bucket can see, conflict rule, 30-day retention of losing edits), a feature bullet, and an S3 mention in
  the telemetry FAQ. `cloud.rs` header now describes record-level sync and points at `sync_backend.rs`.
  Written ahead of step 7, so re-check the setup steps against the GUI during E2E.

## Resume here (2026-10-13, after the 13.10 re-run)

- **PR #1 and PR #2 are merged** into the fork's `main` (`e3cc909`); the manual re-run on that code passed
  (13.10, progress note above). Upstream has moved on since `75678e5` (11 fixes on 2026-10-13, three files
  conflict either way) — parked, not needed for anything here.
- **On the Linux machine:** start with `docs/linux-test-session.md` (step 15). The design calls below don't
  block it.
- **Next, in order:** step 15 (Linux run) → design calls:
  "Delete from cloud" vs. a device that still has the profile open, and the two items under "Found in the
  test review" → §13 Mac + Linux acceptance (15.5) → not run yet: §5, §6.3, §7, §8.1, §8.6, §10.2–10.3 (need
  MinIO / a self-hosted server) → 14.6 → polish list (the two 13.10 items are small UI fixes).
- Upstream's archive tests fail on macOS (see step 13 note) — report upstream or ignore; not this PR's.
- Screenshots `Image1–9.png` in the repo root are untracked and show the bucket/host — never commit them.

## Step 7 — E2E checklist (resume here)

The full, generic test procedure (incl. step-9 name tests) is `docs/manual-tests.md`; this list tracks the run.

Toolchain: Rust in `~/.cargo/bin` (not on PATH — prefix `PATH="$HOME/.cargo/bin:$PATH"`); `node_modules` installed.

Build two isolated test copies (own identifier ⇒ own app data ⇒ separate "devices"; the real
`com.submarine.app` data is never touched). Rebuild if the copies listed in `TODO.local.md` are gone:

```
for X in A B; do x=$(echo $X | tr AB ab)
  PATH="$HOME/.cargo/bin:$PATH" npx tauri build --debug --bundles app \
    --config "{\"identifier\":\"com.submarine.e2e.$x\",\"productName\":\"Submarine E2E $X\"}"
  cp -R "src-tauri/target/debug/bundle/macos/Submarine E2E $X.app" <somewhere>/
done
```

Use the user's bucket (see `TODO.local.md`), folder **`submarine/sync-test`** (never `submarine/sync` or the old
script's prefix), credentials file `~/.config/submarine-sync/s3cfg`, a throwaway profile `e2e` with a
test-only password.

- [x] A: picker → Cloud → S3 bucket tab → fill bucket/folder → Test connection (expect "Connected …"; region `us-east-1` or `default` — the message shows whichever Linode accepted) → Save; bar shows `<bucket>/submarine/sync-test`
- [x] A: create profile, add server, add a saved login (key icon **Logins** → Add Password), link it (hover server → pencil **Edit** → How to log in → **Use a saved login**) → Profile panel → Sync now; bucket has `r/servers/…`, `r/credentials/…`, `escrow/…`, `meta.json`
- [x] B: same S3 setup → picker lists the profile from the bucket → restore with the password → servers + saved-login link present (done with `E2e-cred`; A's `e2e` was hidden by the picker bug below)
- [x] B: edit a server → sync; A: sync → sees it. Delete a server on A → gone on B after sync (bucket: B's `.rec`, then A's empty `.del`)
- [x] Wrong password on restore says wrong password, not a network error (shows `ESCROW_UNSEAL_FAILED: wrong password for this profile`)
- [x] Profile panel diff says "in sync with your bucket"
- [x] Error path: set folder to a bucket the key can't use / bad credentials path → readable message in panel + Activity log
- [x] Picker → "Delete from cloud" on `e2e` → `submarine/sync-test/` empty (only once A was quit — see "Found in the re-run")
- [x] Cleanup: quit both, remove `~/Library/{Application Support,Caches,WebKit}/com.submarine.e2e.{a,b}` (ask user before deleting)

Then user acceptance (manual-tests §13): release build on the Mac and the Linux machine pointed at
`submarine/sync`, retire the old sync script (keep its old object in the bucket as a backup).

Re-run the Linode integration tests any time (throwaway `submarine-it/<id>/` prefix, self-cleaning):
`SUBMARINE_S3_IT=1 SUBMARINE_S3_IT_CFG=$HOME/.config/submarine-sync/s3cfg SUBMARINE_S3_IT_BUCKET=<bucket> cargo test --lib it_`

## Review

**Built.** Personal profiles can sync record-by-record through any S3-compatible bucket, chosen per install
(`<app_data>/sync_backend.json`, no secrets in it). Shared profiles stay on the HTTP cloud.
- `sync_backend.rs`: `SyncTransport` seam; `HttpTransport` keeps the old wire traffic byte-for-byte.
- `object_sync.rs`: client-side merge over a bucket. One immutable object per record version, everything the
  merge needs is in the key (one LIST = whole index), append-only writes so no locks; newest HLC stamp wins;
  superseded versions (incl. a conflict's loser) kept 30 days, then removed by any device's next sync.
- `s3_store.rs` + `sigv4.rs`: plain REST with SigV4 header signing (no new crates), s3cmd-format credentials
  read per sync and zeroized, region auto-discovery, retries with backoff, redirects off, size caps.
- Engine fixes found on the way: late-arriving FK repair (`sync_fk_pending`), per-uuid max in stats.
- Commands `sync_backend_get`/`set`, `s3_test_connection`; `SyncStorageSection` UI in CloudPanel + Settings;
  personal sync gated on the active backend (S3: a bucket is set), sharing still on sign-in; sync failures
  shown in the Profile panel as well as the Activity Log.
- Profile names are kept exactly as typed but a new name matching an existing one in any letter case is
  refused (steps 9 and 13.5, decision 8); `applog.rs` writes a redacted `submarine.log` (step 12);
  `docs/manual-tests.md` holds the manual test procedure.

**Verified.** 75 Rust tests pass (2026-10-12), including two-device scenarios on an in-memory store and the 3
Linode integration tests run live (10 s, throwaway prefix left empty); `npm run typecheck`/`build` clean. The
two-copy GUI E2E passed (2026-10-12): every manual test except §7, §8.1 and §8.5; log secrets grep empty.

**Not yet verified.** The Mac + Linux acceptance run; manual tests §7, §8.1, §8.5. The README setup steps were
written from the code; the E2E click-through followed the manual-test doc.

**Known limits.** No S3 path on Android (needs a credentials file on disk). The bucket sees metadata: key
names (type, record id, stamp), sizes, upload times, and the profile name. Losing edits are only recoverable
with bucket tooling — no in-app restore. The README's Security section still describes whole-blob upload, and its
telemetry FAQ names `api.sinaxhpm.com` while the code uses `submarine.sinaxhpm.com`; both predate this branch
and were left alone.

**Found in E2E (2026-10-05; open unless marked fixed — the 2026-10-12 findings are under "Found in the re-run"):**
- *Fixed in step 9.* *Bug (upstream, `6d5aaae`, any backend):* the profile picker merges local and cloud profiles by name,
  case-insensitively (`ProfileSelectPage.tsx` `buildRows`). Two profiles whose names differ only in case
  (`e2e` from A, `E2e` created on B) collapse into one row: one is hidden, the row's cloud link points at
  whichever partition was listed last, so **Delete from cloud can delete the other profile**, and the
  SYNCED badge is wrong. Needs a design call: the picker can't see a local vault's partition id before
  unlock, so matching by id isn't free.
- *Question:* escrows are kept one per vault salt and only removed with the whole profile (by design: each
  device publishes its own). Check whether a password change leaves an escrow sealed under the OLD
  password in the bucket — if so, the old password can still restore the profile from the bucket. Seen in
  E2E: every *restore* also adds one (new local vault ⇒ new salt), so `E2e-cred` had 3 after A + 2 restores on B.
- *Polish (upstream wording, seen with S3):* the Profile panel subtitle says "This profile lives only on your
  devices." for any unshared profile, though it is also in the bucket; "N items" counts tombstones (by design,
  `total_records` = live + tombstones) so a deleted server still counts; edits show "unattributed" without a
  cloud account.
- *Polish:* restore's wrong-password error shows the code (`ESCROW_UNSEAL_FAILED: …`); unlocking a local
  copy says "Wrong password." — make them match.

**UX follow-up (upstream, not this branch).** Linking a saved login is hard to find: the Edit pencil only
appears on hover (next to Clone), logins live on a separate page, and the Logins page can't attach a login to
a server. Reported during E2E 2026-10-05.
