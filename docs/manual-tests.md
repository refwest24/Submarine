# Manual tests

Click-through checks for profile sync, S3 storage, the sync server settings and the profile picker — the
parts the automated tests can't reach (the real app, the real bucket, two devices). Run them before merging changes to sync, storage or the
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

cd .. && npm ci && npm run typecheck && npm run build     # npm ci after pulling new dependencies
```

- Without `SUBMARINE_S3_IT=1` the three live tests (`it_store_…`, `it_connection_test_…`,
  `it_two_devices_converge_…`) return early and still print `ok` — check they took seconds, not
  milliseconds. The `it_` filter matches any test whose full path contains it (`edit_`, `bit_`,
  `live_edit_tests::`, …), so expect several extra offline tests in the run; the three above are the ones
  that matter.
- `a_profile_name_taken_in_any_letter_case_is_refused` checks the name rule (test 8) on whatever disk the temp
  folder is on; on a case-sensitive one it also checks that an exact name wins over a case variant left from
  before the rule. To run that part on a Mac, use a case-sensitive disk image:
  `hdiutil create -size 20m -fs "Case-sensitive APFS" -volname subcs cs.dmg`, `hdiutil attach cs.dmg`, then
  `TMPDIR=/Volumes/subcs/ cargo test --lib a_profile_name`, then `hdiutil detach /Volumes/subcs`.
- `an_s3_transport_needs_no_sign_in_and_no_server` checks that bucket sync needs no sync-server account and
  isn't tied to a server (test 9 checks it in the app).
- Known on macOS: 8–9 of upstream's archive tests (`archive::tests::sh_runs::…`) fail with "Abort trap: 6"
  from the system `tar`/`unzip`, on upstream's own code too. Not related to sync; see "Known open issues".

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

Lines look like `2026-10-05T23:59:59.123Z WARN  [S3] retrying after: …` — the level is padded to five characters,
so `INFO` and `WARN` are followed by two spaces and `ERROR` by one (the quotes below are written that way; copy them
as they are when grepping). Areas: `APP`, `SYNC`, `S3`, `PROFILE`, `PICKER`, `ACTIVITY` (everything the Activity
Log shows), `SSH`, `SFTP`, `TUNNEL`, `MONITOR`, `SHARE`, `DATABASE`, and `UI` for anything the app logs
without naming an area.
In portable mode the file is in `submarine-data/logs/`, and the first lines after `APP … started` say so.

**Secrets check — after every run, this must print nothing:**

```sh
grep -iE -- '-----BEGIN [A-Z ]*PRIVATE KEY|Private-Lines|secret_key *[:=] *[^[ ]|Signature=[0-9a-f]|Bearer [^[]|password *[:=] *[^[ ]' \
  ~/Library/Logs/com.submarine.e2e.*/submarine.log
```

A redacted secret leaves `[redacted]` or `[redacted private key]` behind; those are fine and don't match. Neither
should a grep for each test profile password, the bucket's access key or any saved login's password — run that
with the real values, one at a time, before the logs are deleted in test 12.

### The app's words for things

| In the code | In the app |
|---|---|
| credential | a **saved login** — key icon **Logins** → **Add Password** |
| node | a **server** — **Servers** tab |
| `credential_id` link | server editor → **How to log in** → **Use a saved login** → **Saved login** |
| edit / clone / delete a server | hover the server row: **Clone**, **pencil (Edit)**, **trash (Delete)** appear at the right end |
| sync backend | **Profile sync storage**, tabs **Sync server** / **S3 bucket** (profile picker → **Connect cloud sync** / **Manage**, or **Settings → Cloud Sync**) |
| `cloud_config` (server, proxy) | Cloud panel → **Sync server** (Submarine Cloud or your own) and its **Proxy** choice |
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

Both copies start **signed out** of any sync server: bucket sync needs no account (test 9 checks that).

On A: profile picker → **Connect cloud sync** (signed out) or the cloud bar's **Manage** → **S3 bucket** tab. Credentials file `<cfg>`, Bucket `<bucket>`, Folder
`<folder>` → **Test connection** → **Save**.

- **Expect:** "Connected to bucket '<bucket>' at <endpoint> (region …) — write, read, list and delete all
  work." The region is whichever the server accepted (Linode may take `us-east-1` or ask for `default`).
  **Keys found** shows the endpoint. After Save: "Saved — personal profiles now sync through your bucket.";
  once the panel is closed, the picker's cloud bar shows `<bucket>/<folder>` and **S3 bucket** (**S3 bucket ·
  Needs setup** while the credentials file can't be read).
- **Check:** the bucket folder is still empty (the test object was removed); A's `sync_backend.json` holds the
  backend, bucket, `prefix` (the folder), the credentials file path and the endpoint/region/path-style
  settings — no keys.
- **Log:** `INFO  [S3] connection test OK: Connected to bucket …`, then `INFO  [S3] sync storage set to bucket
  '<bucket>', folder '<folder>', credentials file <cfg>`. If the server asked for another region: `INFO  [S3]
  server wants region 'default', not 'us-east-1' — retrying with it`.

## 2. First sync, with a saved login

On A: **New profile** (type the name in lower case, e.g. `e2e`) → add a server → **Logins** → **Add Password**
→ edit the server: **How to log in** → **Use a saved login** → pick it → **Save changes** → **Profile** panel →
**Sync now**.

- **Expect:** "Last synced <time>".
- **Log:** `INFO  [PROFILE] created profile 'e2e'`, then `INFO  [SYNC] 'e2e' synced: pushed N, pulled 0 (… ms)`
  for each sync (auto-sync included) and `INFO  [ACTIVITY] Cloud sync complete — …`.
- **Bucket:** one new `<profile-id>/` folder with `escrow/…`, `meta.json` (the name exactly as typed),
  `r/credentials/…` and `r/servers/…`. Editing a record again adds a second version; the old one stays.

## 3. Restore on a second device

On B: same storage setup as test 1. The picker lists A's profile as **In your cloud · N items** (**Refresh
from cloud** if not).

1. Select it → **Get on this device** → a **wrong** password. **Expect:** a wrong-password message
   (currently `ESCROW_UNSEAL_FAILED: wrong password for this profile`), not a network error.
2. Again with the right password. **Expect:** `"<name>" is now on this device — pulled N items…`; the row
   reads **On this device · Synced**.
3. Open it. **Expect:** same servers and saved logins as A; the server's editor still says **Use a saved
   login** with the same login picked; connecting works if the server is real.
4. Back on the picker, a wrong password on the now-local copy says "Wrong password."
5. **Bucket:** one more `escrow/…` object (each restore makes a new local vault with its own salt); no
   records uploaded again.
6. **Log:** `WARN  [SYNC] restore of '<name>' from cloud profile <id> refused: … ESCROW_UNSEAL_FAILED …` for
   step 1; `INFO  [SYNC] restored '<name>' … pulled N` for step 2; `WARN  [PICKER] opening '<name>' failed:
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
3. **Log:** for 1, `WARN  [S3] connection test for bucket '<bucket>-nope', folder '<folder>' failed: [S3] …`;
   for 2, `ERROR [SYNC] '<name>' sync failed after … ms: [S3] CREDENTIALS_FILE: can't read …/missing: …` plus the
   `ERROR [ACTIVITY] Sync storage isn't set up: can't read …/missing: …` line. A broken `sync_backend.json` logs
   `ERROR [S3] CONFIG_INVALID: sync_backend.json: …` and, on the picker, `WARN  [PICKER] sync storage settings
   unreadable: …`.

## 7. Concurrent edits (optional)

Edit the same server differently on A and on B **without syncing in between**. Sync A, then B, then A.
**Expect:** both show the edit made last. **Bucket:** both versions exist; the older one is kept for 30 days.

## 8. Profile names

Names keep exactly what was typed, and the picker links a local profile to a cloud one only by the exact name.
But a **new** name that matches an existing profile in **any letter case** — on this device or in the cloud —
is refused on every platform: `e2e` and `E2e` can't both be created.

1. **No auto-capitalisation:** with macOS *Capitalize words automatically* on (System Settings → Keyboard →
   Text Input), type `lowercase` into **New profile** and into the name box of **Import**. **Expect:** it stays
   lower case, and `meta.json` in the bucket matches.
2. **Refused while typing:** with `e2e` on the device, type `E2e` into **New profile**. **Expect:** under the
   name, `There's already a profile named "e2e".`, and the profile isn't created. With `e2e` only in the
   cloud (an **In your cloud** row), typing `E2e`: `"e2e" is in your cloud. Get it from the list, or pick another
   name.` **Import** under `E2e` is refused the same ways; importing under the exact cloud name `e2e` is
   allowed (it may be that very profile).
3. **Refused on restore:** **Get on this device** of a cloud profile `e2e` while the device has `E2e`.
   **Expect:** "Can't use 'e2e': you already have a profile called 'E2e', and names that differ only in
   letter case aren't allowed. Pick another name." **Log:** `WARN  [PROFILE] Can't use 'e2e': …` and
   `WARN  [SYNC] restore of 'e2e' … refused: …`.
   To set this up now that the picker refuses it: create `E2e` on B **before** B's storage is set up (so B
   can't see A's `e2e`), add a server, then set up storage and sync.
4. **Separate rows:** in that setup B lists `E2e` (**On this device · Synced**) and `e2e` (**In your cloud**)
   as two rows, never merged.
   Give each test profile at least one server first: the picker hides cloud profiles with no live items.
5. **Delete from cloud hits only its own row:** on the `e2e` **In your cloud** row → **⋯** → **Delete from
   cloud…** → confirm. **Bucket:** only A's `e2e` folder is gone; B's `E2e` folder is untouched.
6. **Same exact name from two devices:** create `dup` on A and sync; on B create `dup` before setting up
   storage, then set it up and sync. **Expect** on A: local `dup` shows **On this device** without **Synced** (not
   linked — the picker can't tell which cloud copy is its own), plus two `dup` **In your cloud** rows. **Get** on either says it
   already exists; **Delete from cloud…** on one removes only that folder (check the bucket).
7. **Linux:** create `e2e`, then try `E2e`. **Expect:** refused as in 2 — case-sensitive disks follow the same
   rule now.

## 9. Bucket sync without a sync-server account

Both copies signed out (Cloud panel shows **Sign in**), storage on **S3 bucket**.

1. **Sync now** and auto-sync work. **Expect:** no "not signed in"; the Profile panel says "Syncing through
   your S3 bucket." and shows no **Sync server:** line and no "This profile syncs with …, not with …"
   warning or **Sync with …** button.
2. **Get on this device** on B works signed out (test 3), and its Profile panel says "Everything is in sync
   with your bucket."
3. **Delete from cloud** works signed out (test 12).
4. Storage back to **Sync server** while signed out → **Profile** panel. **Expect:** **Sync now** is gone and the
   panel says "Not signed in — lock this profile and sign in from the Cloud button on the picker." (the sync
   server still needs an account). Auto-sync keeps trying quietly and logs `ERROR [SYNC] '<name>' sync failed
   after … ms: [CLOUD] NOT_SIGNED_IN`. Back to **S3 bucket**: **Sync now** returns.
5. **Log:** `INFO  [SYNC] '<name>' synced: …` lines and no `[CLOUD] NOT_SIGNED_IN` while on S3.

## 10. Sync server and proxy settings

1. **Proxy applies to the bucket:** Cloud panel → **Sync server:** line → **Change** → **Proxy** → **Manual**
   `http://127.0.0.1:9` (nothing listens there) → **Save** → storage **Test connection**. **Expect:** an
   "endpoint unreachable" error after a few seconds of retries, not "Connected". **Sync now** fails with "Sync
   to your bucket failed: …". Proxy back to **System** → both work again. **Log:** four `WARN  [S3] retrying
   after: [S3] ENDPOINT_UNREACHABLE: …` lines, then `WARN  [S3] connection test … failed: [S3]
   ENDPOINT_UNREACHABLE: …`. (`[S3] NETWORK:` is reserved for failures that are neither a refused connection
   nor a timeout.)
2. **A bucket endpoint on this machine is never proxied** (only with a local MinIO at `http://127.0.0.1:9000`):
   with the dead proxy above, **Test connection** to it still works.
3. **Another sync server doesn't touch bucket profiles** (needs a reachable self-hosted server): set it in the
   Cloud panel. **Expect:** S3 profiles keep syncing to the bucket, with no "This profile syncs with …, not with
   …" warning. Switch storage to **Sync server** and sign in: a profile that synced with the old server shows
   that warning and the **Sync with …** button; one **created on this device** and never synced over HTTP links
   to the new server on its first sync. A profile that reached this device by **Get on this device** from the
   bucket behaves as if linked to Submarine Cloud instead (see "Known open issues"). Set everything back.

## 11. Profile picker

1. Click a row, type the password, **Unlock**. **Expect:** that profile opens (never another of a similar
   name). Wrong password: "Wrong password." under the box, `WARN  [PICKER] opening '<name>' failed: wrong
   password` in the log.
2. **⋯** on each kind of row (**On this device**, **On this device · Synced**, **In your cloud**) — also by
   right-click and from the keyboard. **Expect:** the menu belongs to that row: **Remove from this device…**
   only on rows on this device, **Get on this device…** only on **In your cloud** rows, **Delete from
   cloud…** on rows with a cloud copy.
3. **Refresh from cloud** fails readably with a broken credentials file (test 6.2's path), and the log has
   `WARN  [PICKER] …`.

## 12. Delete from cloud and clean up

1. Picker → each test profile → **⋯** → **Delete from cloud…** → confirm. Quit the other copy first: a copy
   that still has the profile open uploads it again on its next sync (known open issue).
   **Expect:** `Removed "<name>" from the cloud (N records).` **Bucket:** `<folder>/` is empty.
   **Log:** `INFO  [SYNC] deleted cloud profile <id> (N records)` per profile.
2. Run the secrets check above on both log files.
3. Quit both copies and remove their data:
   `~/Library/{Application Support,Caches,WebKit,Logs}/com.submarine.e2e.{a,b}`.

## 13. Acceptance on real machines

Release builds on each real machine (e.g. a Mac and a Linux box), pointed at the real folder. Restore each
profile on the machine that doesn't have it, edit on one and sync the other, then retire any older sync
script — keep its last copy in the bucket as a backup.

## Not reproducible on demand

- **"Replace the cloud copy with this device"** (Profile panel) appears only when items keep failing to come
  down after a sync — records this device can't decrypt. Covered by unit tests.
- **Throttling** (`503 SlowDown`) retries: only when the provider throttles; watch for it in the live tests.

## Known open issues to re-check

- Restore's wrong-password message shows the error code; unlocking a local copy says "Wrong password."
- The picker's "Couldn't load your cloud profiles: …" banner (test 11.3) shows the error code
  (`CREDENTIALS_FILE: can't read …`) and stays on screen after the settings are fixed, until another message
  replaces it. The dead-proxy error (test 10.1) is the HTTP client's raw text ("error sending request: client
  error (Connect): tunnel error: … Connection refused"), not a sentence.
- Every restore adds an `escrow/…` object; none are removed until the profile is deleted. Open question:
  after a password change, does the old password still restore the profile from the bucket?
- Profile panel wording with S3: "This profile lives only on your devices", "N items" includes deletion
  markers, edits show "unattributed" without a cloud account.
- **Delete from cloud** is undone by any device that still has the profile open: its next sync uploads it
  again. Needs a design decision.
- A profile that came to a device by **Get on this device** from a bucket has no sync-server link (`cloud_server`
  row) at all, and no row counts as "linked to Submarine Cloud". Move such a device to a self-hosted sync server
  and the profile refuses to sync there ("This profile syncs with …, not with …") instead of linking on its
  first sync, as a profile created on the device would. Found 2026-10-13 in a code review; not reproduced.
- The any-letter-case name rule (test 8) is enforced against cloud names by the picker, which first drops
  cloud profiles with no live items. So an empty cloud profile `E2e` doesn't stop a new local `e2e`; only the
  on-disk check runs then. Found 2026-10-13 in a code review.
- Upstream's archive tests (`archive::tests::sh_runs::…`) fail on macOS with "Abort trap: 6" from the
  system `tar`/`unzip`, on upstream's own code too.
