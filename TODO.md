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
- [ ] 7. E2E: two isolated app copies on scratch prefix `submarine/sync-test`; then Mac + Linux acceptance
- [ ] 8. Docs: README section, `cloud.rs` header, Review below

## Progress notes

- **Step 1–2:** engine fix + transport seam; HTTP wire traffic unchanged (6 call sites swapped).
- **Step 3–4 deviation:** MinIO images are no longer public → integration tests run against Linode under
  throwaway `submarine-it/<id>/` prefixes (user approved). `rusty-s3` dropped: Linode's gateway rejected its
  presigned *listing* requests (`403 failed authentication`), so requests are now SigV4 **header**-signed by
  `sigv4.rs` (matches AWS's published example signatures). Linode wants region `default` — discovered
  automatically from the error, like s3cmd. Added retries with backoff (Linode answers bursts with
  `503 SlowDown`) and a per-store HTTP client. Net: no new crates.
- **Step 5–6:** commands + UI; `npm run typecheck`/`build` clean; 71 Rust tests + 4 Linode ITs pass.
- **Step 7 (paused 2026-10-02):** not started. The GUI can't be tested from Chrome/Playwright (no Tauri IPC in
  a plain browser; `tauri-driver` has no macOS support), so it will be driven with desktop computer use from
  the Claude desktop app. Nothing launched yet; no E2E app data exists; the bucket holds only the user's
  existing objects. Branch pushed to origin (no PR).

## Step 7 — E2E checklist (resume here)

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

- [ ] A: picker → Cloud → S3 bucket tab → fill bucket/folder → Test connection (expect "Connected … region default") → Save; bar shows `<bucket>/submarine/sync-test`
- [ ] A: create profile `e2e`, add node `web` (host h1) with a credential → Profile panel → Sync now; bucket has `r/…`, `escrow/…`, `meta.json`
- [ ] B: same S3 setup → picker lists `e2e` from the bucket → restore with the password → `web` + credential link present
- [ ] B: edit `web` host → sync; A: sync → sees it. Delete a node on A → gone on B after sync
- [ ] Profile panel diff says "in sync with your bucket"; wrong password on restore says wrong password (not a network error)
- [ ] Error path: set folder to a bucket the key can't use / bad credentials path → readable message in panel + Activity log
- [ ] Picker → "Delete from cloud" on `e2e` → `submarine/sync-test/` empty
- [ ] Cleanup: quit both, remove `~/Library/{Application Support,Caches,WebKit}/com.submarine.e2e.{a,b}` (ask user before deleting)

Then user acceptance: release build on the Mac and the Linux machine pointed at `submarine/sync`, retire the
old sync script (keep its old object in the bucket as a backup). Then step 8 (docs).

Re-run the Linode integration tests any time (throwaway `submarine-it/<id>/` prefix, self-cleaning):
`SUBMARINE_S3_IT=1 SUBMARINE_S3_IT_CFG=$HOME/.config/submarine-sync/s3cfg SUBMARINE_S3_IT_BUCKET=<bucket> cargo test --lib it_`
