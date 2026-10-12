# Manual tests

Click-through checks for profile sync, S3 storage and profile names — the parts the automated tests can't
reach (the real app, the real bucket, two devices). Run them before merging changes to sync, storage or the
profile picker, and before a release.

Placeholders: `<bucket>` is your bucket, `<folder>` a **scratch** folder in it (for example
`submarine/sync-test` — never the folder your real profiles sync to), `<cfg>` your s3cmd-format credentials
file (default `~/.config/submarine-sync/s3cfg`).

## 0. Before you start

### Automated tests first

```sh
cd src-tauri
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib            # offline unit tests, a few seconds

# live tests against a real bucket; they use a throwaway submarine-it/<id>/ folder and clean it up
SUBMARINE_S3_IT=1 SUBMARINE_S3_IT_CFG=<cfg> SUBMARINE_S3_IT_BUCKET=<bucket> \
  PATH="$HOME/.cargo/bin:$PATH" cargo test --lib it_

cd .. && npm run typecheck && npm run build
```

- Without `SUBMARINE_S3_IT=1` the three live tests (`it_store_…`, `it_connection_test_…`,
  `it_two_devices_converge_…`) return early and still print `ok` — check they took seconds, not
  milliseconds. The `it_` filter also matches two offline tests (`edit_…`), so it reports 5.
- `profile_names_are_case_sensitive_where_the_disk_allows` checks whichever kind of disk the temp folder is on.
  To cover the other kind on a Mac, run it on a case-sensitive disk image:
  `hdiutil create -size 20m -fs "Case-sensitive APFS" -volname subcs cs.dmg`, `hdiutil attach cs.dmg`, then
  `TMPDIR=/Volumes/subcs/ cargo test --lib profile_names`, then `hdiutil detach /Volumes/subcs`.

### Two "devices" on one machine

Build two copies with their own identifier. Each gets its own app-data folder and device id, so they behave
like two machines, and neither touches your real Submarine data:

```sh
for X in A B; do x=$(echo $X | tr AB ab)
  PATH="$HOME/.cargo/bin:$PATH" npx tauri build --debug --bundles app \
    --config "{\"identifier\":\"com.submarine.e2e.$x\",\"productName\":\"Submarine E2E $X\"}"
  cp -R "src-tauri/target/debug/bundle/macos/Submarine E2E $X.app" <somewhere>/
done
open "<somewhere>/Submarine E2E A.app"
```

Never run two copies with the same identifier at once — they would share one app-data folder.
Use throwaway profile passwords. Servers can point at a made-up host unless a test says to connect.

### The log file

Every test below also leaves a trail in `submarine.log` — easier to read than a screenshot. Each copy has its
own: `~/Library/Logs/com.submarine.e2e.a/submarine.log` (and `…e2e.b…`) for the test copies, or the path shown in
**Settings → Activity Log**. Keep one open while testing:

```sh
tail -f ~/Library/Logs/com.submarine.e2e.a/submarine.log
```

Lines look like `2026-10-05T23:59:59.123Z WARN  [S3] retrying after: …`. Areas: `APP`, `SYNC`, `S3`, `PROFILE`,
`PICKER`, `ACTIVITY` (everything the Activity Log shows), `SSH`, `SFTP`, `TUNNEL`, `MONITOR`, `SHARE`.

**Secrets check — after every run, this must print nothing:**

```sh
grep -iE 'PRIVATE KEY|Private-Lines|secret_key *[:=] *[^[ ]|Signature=[0-9a-f]|Bearer [^[]|password *[:=] *[^[ ]' \
  ~/Library/Logs/com.submarine.e2e.*/submarine.log
```

and neither should a grep for the test profile passwords, the bucket's access key or any saved login's password.

### The app's words for things

| In the code | In the app |
|---|---|
| credential | a **saved login** — key icon **Logins** → **Add Password** |
| node | a **server** — **Servers** tab |
| `credential_id` link | server editor → **How to log in** → **Use a saved login** → **Saved login** |
| edit / clone / delete a server | hover the server row: **Clone**, **pencil (Edit)**, **trash (Delete)** appear at the right end |
| sync backend | **Profile sync storage** (cloud bar on the profile picker, or **Settings → Cloud Sync**) |
| activity log | **Settings → Activity Log → View Activity Log** |

### Looking inside the bucket

```sh
s3cmd -c <cfg> ls -r s3://<bucket>/<folder>/
s3cmd -c <cfg> get --quiet s3://<bucket>/<folder>/<profile-id>/meta.json -     # the profile's name
```

Layout per profile: `r/<type>/<record-id>/<stamp>.rec` (a version), `….del` (a deletion marker, 0 bytes),
`escrow/<salt>`, `meta.json`. `<stamp>` is hex: decode it (`python3 -c "print(bytes.fromhex('<hex>').decode())"`)
to get `<unix-ms>:<counter>:<device-id>`; each copy's device id is `node_id` in its `sync_device.json`.

## 1. Storage setup

On A: profile picker → cloud bar → **S3 bucket** tab. Credentials file `<cfg>`, Bucket `<bucket>`, Folder
`<folder>` → **Test connection** → **Save**.

- **Expect:** "Connected to bucket '<bucket>' at <endpoint> (region …) — write, read, list and delete all
  work." The region is whichever the server accepted (Linode may take `us-east-1` or ask for `default`).
  **Keys found** shows the endpoint. After Save: "Saved — personal profiles now sync through your bucket.";
  the picker's cloud bar shows `<bucket>/<folder>` with an **S3** badge.
- **Check:** the bucket folder is still empty (the test object was removed); A's `sync_backend.json` holds
  bucket, folder and file path only — no keys.
- **Log:** `INFO [S3] connection test OK: Connected to bucket …`, then `INFO [S3] sync storage set to bucket
  '<bucket>', folder '<folder>', credentials file <cfg>`. If the server asked for another region: `INFO [S3]
  server wants region 'default', not 'us-east-1' — retrying with it`.

## 2. First sync, with a saved login

On A: **New profile** (type the name in lower case, e.g. `e2e`) → add a server → **Logins** → **Add Password**
→ edit the server: **How to log in** → **Use a saved login** → pick it → **Save changes** → **Profile** panel →
**Sync now**.

- **Expect:** "Last synced <time>".
- **Log:** `INFO [PROFILE] created profile 'e2e'`, then `INFO [SYNC] 'e2e' synced: pushed N, pulled 0 (… ms)`
  for each sync (auto-sync included) and `INFO [ACTIVITY] Cloud sync complete — …`.
- **Bucket:** one new `<profile-id>/` folder with `escrow/…`, `meta.json` (the name exactly as typed),
  `r/credentials/…` and `r/servers/…`. Editing a record again adds a second version; the old one stays.

## 3. Restore on a second device

On B: same storage setup as test 1. The picker lists A's profile with an **In cloud** badge (**Refresh from
cloud** if not).

1. Select it → **Get on this device** → a **wrong** password. **Expect:** a wrong-password message
   (currently `ESCROW_UNSEAL_FAILED: wrong password for this profile`), not a network error.
2. Again with the right password. **Expect:** `"<name>" is now on this device — pulled N items…`; badge
   **Synced**.
3. Open it. **Expect:** same servers and saved logins as A; the server's editor still says **Use a saved
   login** with the same login picked; connecting works if the server is real.
4. Back on the picker, a wrong password on the now-local copy says "Wrong password."
5. **Bucket:** one more `escrow/…` object (each restore makes a new local vault with its own salt); no
   records uploaded again.
6. **Log:** `WARN [SYNC] restore of '<name>' from cloud profile <id> refused: … ESCROW_UNSEAL_FAILED …` for
   step 1; `INFO [SYNC] restored '<name>' … pulled N` for step 2; `WARN [PICKER] opening '<name>' failed:
   wrong password` for step 4. The passwords themselves never appear.

## 4. Edits and deletes travel both ways

1. B: edit a server (e.g. rename it) → **Sync now**. A: **Sync now** → shows the new name.
   **Bucket:** a new `.rec` version stamped with B's device id.
2. A: delete that server (trash) → **Sync now**. B: **Sync now** → it's gone.
   **Bucket:** a 0-byte `.del` stamped with A's device id, newer than every `.rec` of that record.
3. **Log:** each sync line's pushed/pulled counts match: 1 pushed on the device that changed, 1 pulled on the
   other.

## 5. Sync status

**Profile** panel on either device after syncing. **Expect:** "Everything is in sync with your bucket."
("N items" counts deletion markers too.)

## 6. Error messages

Use **Settings → Cloud Sync → Profile sync storage** on one device.

1. **Bucket the key can't use:** Bucket `<bucket>-nope` → **Test connection** (don't save). **Expect:** a
   readable reason (no such bucket / access denied), not raw XML. Put the bucket name back.
2. **Missing credentials file during a sync:** Credentials file `~/.config/submarine-sync/missing` →
   **Save** (**Keys found** turns into a warning) → **Profile** → **Sync now**. **Expect:** a readable error
   in the panel, and the same error in the **Activity Log**. Restore the path → **Save** → **Sync now** works.
3. **Log:** for 1, `WARN [S3] connection test for bucket '<bucket>-nope', folder '<folder>' failed: [S3] …`;
   for 2, `ERROR [SYNC] '<name>' sync failed after … ms: [S3] CREDENTIALS_FILE: can't read …/missing: …` plus the
   `ERROR [ACTIVITY] Sync storage isn't set up: can't read …/missing: …` line. A broken `sync_backend.json` logs
   `ERROR [S3] CONFIG_INVALID: …` and, on the picker, `WARN [PICKER] sync storage settings unreadable: …`.

## 7. Concurrent edits (optional)

Edit the same server differently on A and on B **without syncing in between**. Sync A, then B, then A.
**Expect:** both show the edit made last. **Bucket:** both versions exist; the older one is kept for 30 days.

## 8. Profile names are case-sensitive

Profile names keep exactly what was typed and are matched exactly: `e2e` and `E2e` are two profiles. A disk
that ignores case (macOS and Windows by default) can't hold both files, so there the second is refused.

1. **No auto-capitalisation:** with macOS *Capitalize words automatically* on (System Settings → Keyboard →
   Text Input), type `lowercase` into **New profile** and into the name box of **Import**. **Expect:** it stays
   lower case, and `meta.json` in the bucket matches.
2. **Case-only clash on macOS/Windows:** with `e2e` on the device, create `E2e`. **Expect:** "Can't use 'E2e'
   here: this device's disk ignores letter case, so it would be the same file as your profile 'e2e'. Pick
   another name." Same for **Import** under that name and for **Get on this device** of a cloud profile
   named `E2e`. **Log:** `WARN [PROFILE] Can't use 'E2e' here: …` for each refusal.
3. **Separate rows:** device B has local `E2e`; the bucket has `e2e` (from A) and `E2e` (from B). **Expect:**
   two rows — `E2e` **Synced** and `e2e` **In cloud** — never merged.
   Give each test profile at least one server first: the picker hides cloud profiles with no live items, so
   an empty `E2e` stays **This device** even after it syncs, and an empty `e2e` never shows as **In cloud**.
4. **Delete from cloud hits only its own profile:** on that `e2e` **In cloud** row → **Delete** →
   confirm. **Bucket:** only A's `e2e` folder is gone; B's `E2e` folder is untouched.
5. **Same exact name from two devices:** create `dup` independently on A and on B (don't restore), sync
   both. **Expect** on A: local `dup` shows **This device** (not linked — the picker can't tell which cloud copy
   is its own), plus two `dup` **In cloud** rows. **Get** on either says it already exists; **Delete** on one
   removes only that folder (check the bucket).
6. **Linux (case-sensitive disk):** create `e2e` and `E2e` on one machine. **Expect:** both allowed, both sync,
   both listed in a stable order, each opens its own data.

## 9. Delete from cloud and clean up

1. Picker → each test profile → **Delete from cloud** (or **Delete** on an **In cloud** row) → confirm.
   **Expect:** `Removed "<name>" from the cloud (N records).` **Bucket:** `<folder>/` is empty.
   **Log:** `INFO [SYNC] deleted cloud profile <id> (N records)` per profile.
2. Run the secrets check above on both log files.
3. Quit both copies and remove their data:
   `~/Library/{Application Support,Caches,WebKit,Logs}/com.submarine.e2e.{a,b}`.

## 10. Acceptance on real machines

Release builds on each real machine (e.g. a Mac and a Linux box), pointed at the real folder. Restore each
profile on the machine that doesn't have it, edit on one and sync the other, then retire any older sync
script — keep its last copy in the bucket as a backup.

## Not reproducible on demand

- **"Replace the cloud copy with this device"** (Profile panel) appears only when items keep failing to come
  down after a sync — records this device can't decrypt. Covered by unit tests.
- **Throttling** (`503 SlowDown`) retries: only when the provider throttles; watch for it in the live tests.

## Known open issues to re-check

- Restore's wrong-password message shows the error code; unlocking a local copy says "Wrong password."
- Every restore adds an `escrow/…` object; none are removed until the profile is deleted. Open question:
  after a password change, does the old password still restore the profile from the bucket?
- Profile panel wording with S3: "This profile lives only on your devices", "N items" includes deletion
  markers, edits show "unattributed" without a cloud account.
