# Sync Module

## Scope

Sync covers provider-neutral, local-first bidirectional file synchronization
between local media directories and a remote WebDAV collection: the provider
contract, the pure planner, persistent state, local publication, recovery, and
the Settings UI that drives it.

The local gallery stays the source of truth for viewing. A remote file enters
the library only after it has been fully downloaded and published locally, so
remote-only items exist solely in the sync manifest until then.

Application metadata — favorites, album covers, ordering, edit history — is
**not** synchronized. Do not copy the SQLite/WAL files to achieve it; a future
metadata sync needs a versioned sidecar protocol.

## Key Files

| File | Role |
|---|---|
| `src/core/sync/mod.rs` | Module surface and re-exports |
| `src/core/sync/provider.rs` | `SyncProvider` trait, capabilities, error taxonomy |
| `src/core/sync/webdav.rs` | WebDAV adapter: HTTP, DAV XML, auth, capability probing |
| `src/core/sync/model.rs` | Connections, jobs, entry identity, baselines, conflicts |
| `src/core/sync/planner.rs` | Pure three-way decision layer (no I/O, no clock) |
| `src/core/sync/local.rs` | Local snapshot, change detection, safe publication |
| `src/core/sync/store.rs` | Persistent state and the `DbActor` command boundary |
| `src/core/sync/service.rs` | Job/connection service, scheduling, live progress |
| `src/platform/credentials.rs` | Secret Service-backed credential storage |
| `src/ui/window/settings.rs` | Connections, jobs, upload scope, conflict UI |
| `src/ui/cloud_badge.rs` | Per-media cloud badge resources, and the rendered-ink contract for them |
| `src/core/schema.sql` | `sync_connections`, `sync_jobs`, `sync_job_upload_albums`, `sync_entries`, `sync_tasks`, `sync_conflicts` |

Test bodies live in child modules: `local/tests.rs`, `planner/tests.rs`,
`service/tests.rs`, `store/tests.rs`, `webdav/tests.rs`.

## Architecture

```text
UI → SyncService → Planner / Scheduler / Executor → SyncProvider
                    ↓                                    ↓
              SyncStore → DbActor (single writer)    WebDAV adapter
                    ↓
              staging + recovery log
```

The dependency direction is fixed. Protocol adapters never write the database
and never construct GTK objects. Every database commit goes through `DbActor`,
which never waits on the network, file locks, or UI callbacks. Network I/O,
hashing, large copies, and XML parsing run on worker threads — never the GTK
main thread, never inside a database transaction.

| Component | Owns | Must not |
|---|---|---|
| `SyncService` | Job config, control entry points, UI state projection | Send DAV requests or overwrite files |
| Planner | No-op / upload / download / conflict / pending from the baseline | Choose a winner by timestamp |
| `SyncProvider` | Enumeration, read/write, conditional operations, capability | Modify the media library |
| `LocalReplica` | Stable read, upload snapshot, download publication | Own GTK presentation or protocol auth |
| `SyncStore` | State queries, transactional commands, generation checks | Become a second database writer |
| Staging | Immutable upload artifacts, download artifacts, recovery log | Be swept by ordinary cache cleanup |

A connection describes an endpoint and credential reference. A job describes a
local/remote root mapping, direction, filter, and delete policy. One connection
can carry many jobs; overlapping roots are rejected so two jobs never write the
same directory.

## Provider Contract

`ObjectKey` is an adapter-managed locator, not an OS `PathBuf`. `Revision` is an
opaque version with a declared validation strength — a WebDAV ETag is not a
content hash and not a stable file ID.

| Capability | WebDAV direction | When missing |
|---|---|---|
| Enumerate / stat | `PROPFIND Depth: 1`, per-directory | Pause the job; an unreadable directory is not an empty one |
| Prepare parent | `MKCOL`, handling already-exists and permission errors | Fail the object; do not skip a required parent |
| Read | Streaming `GET`, conditional when a strong validator exists | Mark the guarantee weak; do not fake snapshot consistency |
| Create-only | Conditional `PUT`, `If-None-Match: *` | Refuse automatic writes |
| Conditional replace | Strong ETag + `If-Match` | Refuse automatic overwrite; tell the user the endpoint is not fully bidirectional-capable |
| Range read | `Range` + `If-Range`, verify `206` and `Content-Range` | Restart from zero; never splice two versions |
| Conditional move | `MOVE` semantics verified per service | Do not pass copy-then-delete off as an atomic rename |
| Incremental enumerate | Optional sync-token / `REPORT` | Full traversal; rebuild the cursor when it is invalidated |
| Recycle / version retention | Service-specific extension | No delete propagation until a recoverable strategy is defined |

`WebDavProvider` uses HTTPS with Basic auth, `MKCOL`, streaming GET/PUT,
exclusive `LOCK`/`UNLOCK`, `If-None-Match: *`, and strong-ETag `If-Match`. A
server that supports locking gets an atomic version check plus write under the
lock; a server returning 405/501 for `LOCK` falls back to HTTP conditional
requests. DAV hrefs are decoded only after origin and root validation,
successful properties are selected per `propstat`, directory XML responses are
capped at 32 MiB, redirects are disabled, and weak ETags never authorize an
automatic replacement.

Reliable conditional writing must be verified from observed behavior.
`OPTIONS` declarations are not evidence, and `DELETE`, `MOVE`, and `PUT`
capabilities are declared separately — supporting one does not imply the others.
Parse namespaces and every `207` sub-status, bound XML and response resource
use, validate href origin/root/encoding, and disable external entity
resolution. HTTPS verification is on by default; cross-origin redirects must
not carry credentials.

A server that ignores `If-Match`/`If-None-Match` is still safe when it honors
DAV Class 2 locking: the adapter takes an exclusive `LOCK`, checks the version
inside the lock, writes, then `UNLOCK`. Verified against an Aliyun-hosted
rclone WebDAV 1.60.1-DEV endpoint, which also returns `200` and `404` in the
same response across multiple `propstat` elements.

## Planning And Conflict Rules

`planner.rs` is the pure three-way decision layer: local observation, remote
observation, and the last proven common baseline. It never picks a winner by
timestamp, because device clocks and server clocks are not guaranteed to agree.

| Relative to the common baseline | Bidirectional default |
|---|---|
| Local-only change | Upload against the expected remote version |
| Remote-only change | Download, re-checking local state before publication |
| Both sides identical | Advance the baseline, transfer nothing |
| Both sides changed, content differs | Keep both, record a conflict |
| One side deleted, other unchanged | Only when the job explicitly enables delete propagation |
| One side deleted, other modified | Conflict; never auto-delete |
| First association, same name on both sides | Verify content or require a choice |

No stable remote object ID means an external rename can only be observed as an
addition plus a disappearance. Same content hash does not prove a rename — the
user may have copied the file. Only preserve a relationship on reliable
evidence, and never claim lossless external-rename detection.

Conflict records bind both specific versions. After the user picks local or
remote, those versions are re-checked: any change on either side invalidates
the choice and the conflict is shown again. Choosing a winner still does not
skip the version-conditional write. "Keep both" first creates persistent
artifacts for both contents, then saves each side under a deterministic
conflict-copy name containing the persistent `OperationId`, reusing the same
name on retry. Neither original path is replaced until both copies are
protected.

## Upload

1. Wait for local writes to settle, then create an immutable snapshot. The
   snapshot's content fingerprint must match the operation record; verify the
   source generation before sealing and redo if it changed. Hard links do not
   guarantee immutability against in-place modification and cannot serve as a
   general snapshot mechanism.
2. Persist the operation preconditions and the sealed artifact reference
   *before* any remote write.
3. Write create-only or conditionally.
4. Re-read and verify when the result is uncertain, then commit the proven
   outcome and the new baseline.

Editing inside the app produces a sync intent only after a successful save;
dragging the edit preview does not. External modifications arrive through the
watcher and are compensated by periodic and startup scans, because an in-memory
event can be lost. Deleting a task, browsing to a different cloud folder, or
changing the upload-album checklist all pause the job and wait for its current
file operation to finish.

If the app saves v3 while v2 is still uploading, v2 may complete, but local v3
must stay dirty with a follow-up task. A stale result must never mark a newer
edit as synchronized.

## Download

1. Record the remote version; confirm the local relative baseline is unchanged.
2. Download into a managed temporary location, verifying response, length,
   version, and content fingerprint. Incomplete files must never appear in the
   gallery.
3. Prepare the publication artifact on the target filesystem and persist the
   recovery log. A cross-filesystem move must copy and sync a target-side
   temporary file first — `rename` is not atomic across mounts, and the Flatpak
   data directory and home-library mount can reject a direct rename even when
   both paths appear under the same host filesystem.
4. Take the short-lived local-change coordination lock and re-confirm that the
   target does not exist (create) or still equals the planned local version
   (replace). A long network transfer must not hold this lock.
5. Publish without blind overwrite, keeping a recoverable previous version.
6. Extract metadata, commit the media change plus the content version and sync
   result, then publish domain events.
7. Release recovery artifacts only once the database result is durable and
   nothing references them.

The publication lock is shared with edit save, delete, and move. It cannot
constrain an external editor, so use a final re-check and previous-version
retention, and do not claim full filesystem transaction isolation. On an
ambiguous race, stop and reconcile rather than continuing to overwrite. If the
file was published but the database commit failed, keep the operation evidence
and reconcile — never blindly restore an old backup over a newer user edit.

Downloaded files are copied to a hidden sibling temporary file before atomic
publication in the local album. A watcher event caused by a download is
merged as a duplicate only when the file's current fingerprint matches the
committed sync result. "Ignore this path for N seconds" is not acceptable, and
an event's sync origin does not excuse a real local edit that happened in the
meantime.

## Scheduling And Triggers

There is no timer-based sync. A run is triggered by pulling down at the top of
Photos, or once per application launch in parallel with the startup scan.
A trigger arriving while its job is already running is intercepted and
remembered: a job never runs concurrently, and after the active run settles
exactly one supplementary run fires, so repeated pulls during a long run
coalesce into a single catch-up. Saved jobs are never started merely by being
created.

Before planning, the service adds every discovered remote media album to the
persisted local upload selection, including the root album for files directly
under the configured root. This makes cloud-backed albums bidirectional on both
sides; only local-only albums remain manually selectable. The updated
configuration generation is used for the same run, and an unchanged selection
is not rewritten.

WebDAV has a persisted global opt-in switch, **off by default**. When off,
home-pull sync and conflict resolution are disabled and the new-connection form
is unavailable. Each job has a separate collapsed album checklist. Checking a
physical folder album enables uploads for files directly in that album, while
every remote album stays in download scope. Unchecked albums use an explicit
remote-authoritative download policy: remote additions and updates publish
locally, local-only content is neither hashed nor uploaded, and remote absence
never deletes local content.

Creating a job requires non-overlapping local and remote roots, and roots
overlapping another job on the same connection are rejected. Saved tasks expose
a cloud-folder browser populated by recursively listing WebDAV collections.
Applying a new root clears that job's old entries, conflicts, operation records,
and staging artifacts so observations from the previous collection cannot be
reused; the job stays paused until the next Photos pull, and changing roots
never deletes cloud content. Changing a remote root warns that cloud files may
replace same-name local files in albums not selected for upload.

Removing a job cascades its entries, conflicts, unfinished operations, and
staging artifacts without removing local or remote media. A connection and its
keyring credential are removed only when no other job references it; a
credential-cleanup error is reported after the relationship is gone.

Recommended starting budgets: at most 4 transfers globally, 2 per connection,
1 per object; remote polling from 60s with jitter, honoring `Retry-After` and
exponential backoff. These are tunable initial values to validate against a real
service, not constants to hard-code into business semantics. Large files
stream; bandwidth and staging budgets are managed by job/run configuration.

Suggested task states: `queued → running → succeeded`, with side paths
`retry_wait`, `blocked_auth`, `conflict`, and `cancelled`. Pausing is a job
state — no new operations are claimed, the queue is retained. An in-flight
request can be cancelled, but an already-issued write may have succeeded, so
interruption enters `reconciling`. Cancelling is not undoing a completed file
change.

```text
[*] --> queued
queued --> running: scheduled, preconditions persisted
running --> succeeded: result verified and committed
running --> retry_wait: proven safe to retry
retry_wait --> queued: backoff elapsed
running --> blocked: auth / permission / space / missing capability
blocked --> queued: condition restored
running --> conflict: both sides changed
conflict --> queued: valid resolution plan
running --> reconciling: timeout / exit / uncertain result
reconciling --> succeeded: operation proven complete
reconciling --> queued: proven not executed, preconditions still valid
reconciling --> conflict: object changed another way
reconciling --> blocked: insufficient evidence
queued --> cancelled: cancelled before execution
```

Remote writes and SQLite commits cannot form one transaction. Persist the
operation ID, expected version, and artifact fingerprint before every remote
side effect. A timeout is not proof of failure — check the target before
retrying, and enter a pending state when the outcome cannot be determined. The
goal is recoverable at-least-once execution, not exactly-once.

App-initiated saves, renames, and moves commit their media change and sync
intent in the same `DbActor` transaction. A crash between filesystem
publication and the database commit is recovered from the staging log.
Re-check the baseline before applying a download so a transfer does not clobber
an edit made during it.

## Recovery

`sync_tasks` is the crash evidence log. Startup reconciliation proves a
completed upload by downloading and hashing the current remote object, and a
completed download from the published local fingerprint plus the remote
version, before committing.

| Crash scene | Handling |
|---|---|
| Intent only, no stable artifact | Re-observe and re-plan |
| Artifact sealed, no remote write yet | Verify the artifact and preconditions, then execute |
| Remote may have written; response or commit lost | Query the target and compare fingerprints; complete the commit if it matches, otherwise keep it pending |
| Incomplete download | Resume only when version and range verification hold, otherwise re-download |
| Published locally, not committed | Compare publication result, backup, and current file; complete the commit or keep the new edit. No blind rollback |
| Old task with a stale configuration generation | Stop the old plan, verify the side effects that already happened, then re-plan |

Task operation IDs include a random per-process session UUID and a sequence
number. Flatpak can reuse a PID across launches, so PID plus a reset sequence
is not unique across restarts. Existing task IDs and their staging artifacts
remain valid for recovery after an upgrade.

Blocked upload tasks stay eligible for reconciliation. For an interrupted new
upload, recovery replaces the remote object only when its downloaded bytes are
a strict prefix of the preserved upload snapshot *and* the server supplies a
strong ETag; the replacement is conditional and downloaded again for hash
verification. If the local file is itself a strict prefix of the snapshot,
recovery restores it even when the truncation point differs from the current
remote prefix. Unresolved upload paths are excluded from normal download
planning, including when the album was subsequently unchecked, and the
snapshot is retained until both copies are proven complete.

Startup order: migrate the database → load jobs and logs → recover unresolved
publications and writes → reconcile both sides → schedule new work. Local
browsing continues during sync recovery; a single file affected by an
unresolved publication is held back rather than blocking the whole gallery.

## Clear, Rebuild, And Detach

- **Clear thumbnail cache**: never touch sync tables, user originals, staging,
  or conflict artifacts.
- **Rebuild the media index**: stop claiming new operations, handle in-flight
  work, and preserve sync state. Null out old `MediaId` links, rescan,
  re-associate, then reconcile. Removing a media index row is not the same as
  deleting a file or detaching sync. Sync entries use `ON DELETE SET NULL` for
  the media link and are not cascade-deleted.
- **Exclude a synced directory or lose root reachability**: pause the job and
  prompt. Never interpret it as "everything was deleted".
- **Detach a job or remove a connection**: stop scheduling and verify in-flight
  side effects first; detaching breaks the relationship without deleting files
  on either side, and unreferenced artifacts are retained with a prompt.
- **Staging cleanup**: reclaim only artifacts with no task, conflict, or log
  reference. Crash-leftover unknown files need ownership verification — never
  recursively clean by directory age.

Deletion propagation is **off** in the first release. A one-sided absence with a
baseline becomes a displayed pending difference; it does not delete the other
side and does not auto-restore. Absence without a baseline is a normal first
sync. Ordinary DAV layout has no cross-device shared deletion log, so a local
deletion record can stop *this* client from restoring a file, but cannot stop a
fresh or reset client from re-uploading a copy. Cross-device global deletion
convergence would need a shared remote manifest, device identity, deletion
confirmation, and a garbage-collection protocol — do not promise it.

## Cloud Badges And Progress

`SyncStore::media_cloud_states()` checks a bounded set of image/video IDs
against enabled jobs and their selected physical albums. It returns no state
outside those albums, `cloud-off` when a local item lacks a current completed
baseline, and `cloud` when `state='synced'`, the entry's local fingerprint and
size match the baseline, and the indexed file size and nanosecond mtime match
the observation. The media scanner intentionally leaves `blake3_hash` empty, so
badge reads must not rely on that column or hash files on the GTK path.

`settings.json` stores `day_cloud_badges_visible` (default `true`); it gates
only Day-grid badges, independently of the WebDAV master switch and the viewer.
Sync writes that change badge state emit `DomainEvent::SyncStateDirty` through
the DB actor's normal event channel. The UI refresh hub then updates visible
Photos/album tiles and an open viewer, including during transfers and after
upload-album selection changes. There is no badge-specific sync timer.

### Badge artwork contract

The badge is one silhouette rendered four ways (`Synced`/`Off` x light/dark
surface), mapped to GResource paths by `src/ui/cloud_badge.rs` and drawn from
`data/icons/gnome-cloud{,-off}-symbolic.svg` by `data/icons/_generate.py`.
Regenerate with `cd data/icons && python3 _generate.py`; the script rasterises
via librsvg at 4x and downsamples, so the checked-in PNGs stay 72x72.

Three properties are load-bearing, because the badge shares a row with the
viewer's vector toolbar icons and used to read as a different set than them:

- **Hairline weight.** One stroke unit in a 16 unit box, so the stroke is 1 px
  at 16 px. The original GNOME path used `stroke-width 2`, which rendered at
  2.25 px next to a 1 px outline glyph. One unit is the floor here, not a shared
  target: the favorite heart beside it is a 1.4 unit wall, because a half-unit
  wall did not survive being scaled to 18 px and read as a pale smear (see
  [viewer.md](viewer.md)). The two are tuned for legibility at the sizes they
  are drawn at, not for equality, and each has its own test.
- **The silhouette fills its box.** The ink spans 13.4 x 11.0 of the 16 units.
  A cloud cannot be square, but when it also leaves the vertical margins empty
  it reads as a smaller icon than its neighbour.
- **The four variants share one ink box.** Switching sync state or light/dark
  must not move or resize the glyph, and all four PNGs carry 4x headroom so
  they stay crisp at 2x.

`src/ui/cloud_badge.rs` asserts all three from the rendered pixels, including
the hairline weight (ink coverage) and the variant parity, so a re-export that
regresses any of them fails the lib tests.

`SyncStore::overview()` is the provider-neutral read projection for compact UI
status. It derives disabled, not-configured, paused, running, failed, ready, or
completed from the global opt-in and enabled jobs' persisted lifecycle
timestamps and errors. The Photos overview hides the sync row while globally
disabled and polls this projection only while its disclosure is open. The
completed label counts distinct image and video paths with a proven common
baseline across enabled jobs, and separately counts distinct unresolved image
conflict paths. A conflict does not erase an earlier proven baseline. Blocked
upload recovery keeps the overview in failed status even when the last run
finished, so a protected unresolved upload is never labeled complete.

While a run is active, the overview label shows live transfer progress from an
in-memory, process-wide session in `SyncService` that is never persisted. The
home pull and each application launch open the session
(`trigger_saved_jobs_once`). Each job pre-plans every path with the same pure
planner used by `reconcile_one` and adds planned download/upload totals;
`reconcile_one` marks the active phase when a transfer starts and counts it
only after the transfer is committed, with a verification that resolves as a
conflict counting too and completed counters clamped to the planned totals.
`sync::live_progress()` returns the snapshot (`None` outside a run). While a
file streams, byte counters advance through the `TransferProgress` sink
attached to the `WebDavProvider`; those bytes surface only in the STORAGE-target
logs — one info line per completed transfer plus a per-job summary — never in
the overview label. The overview renders "正在同步，已同步 X/Y" while
downloading and "正在上传 X/Y 个图片/视频" while uploading, falling back to the
generic running label before totals are known. Sessions are bounded to a
trigger: crash-recovery transfers that run before planning are not counted, and
totals accumulate across the sequential jobs of one pull instead of resetting
per job. A duplicate trigger intercepted during an active run never opens or
closes the session — the closing side keeps it alive while any job is still
running.

## Credentials And Secrets

Passwords are stored through the platform keyring; SQLite holds only a
credential reference. Credentials are injected exclusively through the
credential store, and configuration, the database, and logs must never contain
plaintext passwords or authorization headers. Logs must not record passwords,
auth headers, or full URLs carrying sensitive query parameters.

Flatpak requires the manifest's `--share=network` permission and
`org.freedesktop.secrets` access. Host `cargo run` and the installed Flatpak can
therefore exercise different keyring and session-bus environments. Reinstall the
Flatpak after changing either permission, and test connection creation from
inside the sandbox before claiming service compatibility. Real-service
credentials and WebDAV URLs must never be committed.

## Current Limits

These are intentional, not oversights, and must not be presented in product
copy as supported:

- No delete propagation, and no remote trash. WebDAV `DELETE` is not a
  recoverable system trash.
- No private-CA UI, no range resume, no sync-token incremental enumeration.
- Remote enumeration and BLAKE3 hashing for checked/remote-present paths still
  run every cycle, so large selected albums need metadata/watcher-based
  dirty-item optimization.
- Single-direction UI modes and remote browsing are post-release.
- Verified real-service coverage: DAV locking, both-side additions and
  modifications, Unicode paths, and conflict resolution, against an Aliyun
  rclone WebDAV endpoint. That does not imply compatibility with every WebDAV
  implementation; Nextcloud interop is not yet exercised.

## Design Constraints

These are binding. Violating one is a bug even when the current tests pass.

| ID | Constraint |
|---|---|
| C01 | Every overwrite carries the expected target version; unreliable conditional capability must never silently degrade to a blind write |
| C02 | A baseline advances only after a transfer result or both-side content equality is proven; enumeration alone never advances it |
| C03 | `unknown` and `absent` stay distinct; an incomplete scan never produces a missing-item propagation |
| C04 | Persist intent and recovery evidence before any non-ignorable side effect; when a result is uncertain, verify first |
| C05 | Operations on one `EntryId` are serialized; cross-path operations acquire coordination locks in a fixed order; no two publishers for one target |
| C06 | Background results are bounded by configuration generation, entry generation, and object version; a stale result never clears a newer modification |
| C07 | GTK does no network or large-file I/O; database transactions wait on no network, hash, or external lock; progress notifications are bounded and mergeable |
| C08 | Staging is isolated from originals and the thumbnail cache; artifacts are never removed by a cache clear or index rebuild |
| C09 | Paths are validated per segment and confined to the root; the first release does not follow directory symlinks, preventing out-of-root access and cycles |
| C10 | Case, Unicode normalization, or illegal-name mapping conflicts are reported explicitly, never merged or overwritten |
| C11 | Credentials are injected only through the credential store; logs omit passwords, auth headers, and sensitive full URLs |
| C12 | Enumeration, queue, memory, and staging are budgeted; over budget pauses or applies backpressure and never marks a truncated directory complete |
| C13 | Local content hashes come from a stable snapshot; without a strong remote validator, the weak guarantee is stated and mtime/length alone never proves equality |
| C14 | All automatic cleanup is based on persistent references and recovery state; no simple TTL may delete the only copy of data |
| C15 | With deletion off, a one-sided absence that has a baseline is not auto-restored; first-time additions and deletion candidates use different rules |
| C16 | Logs correlate discovery, execution, and recovery by `JobId` / `EntryId` / `OperationId`, recording error classification and retry reasons |

## Acceptance Matrix

| Scenario | Expected result | Constraints |
|---|---|---|
| Each side adds a file | Bidirectionally consistent files; first creation never overwrites a same-named file | C01, C02 |
| Save v2 locally, save v3 while v2 uploads | v2 may complete; v3 stays pending and converges | C05, C06 |
| Remote updates, local unchanged | Full download then publish; albums, thumbnails, and viewer update | C02, C07 |
| Remote updates, local edited during download | Local edit preserved, producing a re-plan or conflict | C01, C05, C06 |
| Both sides edit while offline | Keep both, never overwrite by time; an expired conflict choice is rejected | C01, C06 |
| Upload succeeds, response lost | Verify the remote first; avoid a blind second overwrite | C04 |
| Crash after publication | Restart recovers media and sync result; a new edit is not clobbered by an old backup | C04, C06, C08 |
| `207` partial failure, directory over budget, network drop | State is unknown/incomplete; no mass delete or reverse re-add | C03, C12 |
| One side deletes a synced file, deletion off | Show a pending difference; the other side stays; no auto-resurrection | C15 |
| Same path and mtime, changed content | Thumbnails and viewer show the new content; the cache never lags permanently | C06, C13 |
| Change filter, clear cache, or rebuild the library | Old tasks invalidated or paused; baselines and unfinished artifacts retained | C06, C08, C14 |
| Unicode paths, encoding, symlinks, name collisions | Valid names sync exactly; out-of-root and collisions block execution | C09, C10 |
| Real DAV and Flatpak | Network, certificates, credentials, conditional write, and recovery all work | C01, C11 |

Mock DAV and the fake provider cover repeatable fault injection; they do not
substitute for real-service acceptance. Verify at least one ordinary DAV
server, Nextcloud, and the user's actual NAS or drive, including Flatpak
network, certificate, and credential access. Passing the mock suite is not
evidence of service compatibility.
