# Sync Module

## Scope

Sync covers provider-neutral, local-first backup and bidirectional file synchronization
between local media directories and a remote WebDAV collection: the provider
contract, the pure planner, persistent state, local publication, recovery, and
the Settings UI that drives it.

The local gallery stays the source of truth for viewing. A remote file enters
the library only after it has been fully downloaded and published locally, so
remote-only items exist solely in the sync manifest until then.

Application metadata — favorites, album covers, ordering, edit history — is
**not** synchronized. Do not copy the SQLite/WAL files to achieve it; a future
metadata sync needs a versioned sidecar protocol.

## Planned Native SyncServer Integration

The native Rust service has a separate [client contract](native-sync.md): normal
downstream first, acquire one upload lease, held-lock applied-token gate and
catch-up, delete→modify→create, database dirty fields and valid success callbacks.
Acquisition failure skips uplink once; lease loss aborts every unfinished uplink
item while concurrent downstream remains available. These rules are planned C1
capabilities and do not describe the existing WebDAV runtime or override its
upload scope/local-deletion suppression.

## Key Files

| File | Role |
|---|---|
| `src/core/sync/mod.rs` | Module surface and re-exports |
| `src/core/sync/provider.rs` | `SyncProvider` trait, capabilities, error taxonomy |
| `src/core/sync/webdav.rs` | WebDAV adapter: HTTP, DAV XML, auth, capability probing |
| `src/core/sync/model.rs` | Connections, jobs, entry identity, baselines, conflicts, persisted failure reasons |
| `src/core/sync/planner.rs` | Pure three-way decision layer (no I/O, no clock) |
| `src/core/sync/local.rs` | Local snapshot, change detection, safe publication |
| `src/core/sync/store.rs` | Persistent state and the `DbActor` command boundary |
| `src/core/sync/service.rs` | Job/connection service, scheduling, live progress |
| `src/core/sync/service/reconcile.rs` | Per-entry observation/planning coordinator, deletion suppression verification, content verification, unchanged reconciliation, upload and download execution |
| `src/core/sync/remote_path.rs` | Shared segment predicates and remote-key construction/root-relative mapping |
| `src/platform/credentials.rs` | Secret Service-backed credential storage |
| `src/ui/window/settings.rs` | Connections, jobs, upload scope, conflict UI |
| `src/core/sync/service/backup.rs` | Immutable remote version objects and path/version records, protected before replacement |
| `src/ui/window/settings/sync_strategy.rs` | Native Backup / Sync strategy buttons for new and saved tasks |
| `src/ui/window/settings/sync_upload.rs` | Select All, atomic upload selection, checked-first list sorting |
| `src/ui/cloud_badge.rs` | Per-media cloud badge resources, and the rendered-ink contract for them |
| `src/core/schema.sql` | `sync_connections`, `sync_jobs`, `sync_job_upload_albums`, `sync_entries`, `sync_tasks`, `sync_conflicts` |

Per-entry reconciliation is implemented in `service/reconcile.rs`. The
coordinator plans and persists the observation, resolves suppression/content
verification, then dispatches the resulting action. Transfer helpers keep the
existing staging, task journal, baseline, progress, backup and rollback order;
a verified suppressed download is handed to the download executor for reuse.

`remote_path.rs` shares dot-segment and empty-segment predicates without
unifying caller policies: the store trims root/album edge slashes and rejects
empty interior segments; relative upload paths reject empty/dot segments;
WebDAV keys reject leading slashes, dot segments and NUL while allowing empty
keys/segments. Root-relative mapping retains its slash-boundary prefix check.
Normalization, error types and error text stay at their existing boundaries.

`walk_remote_collections` in `service.rs` is the single breadth-first traversal
of a remote tree, shared by the engine's discovery and the settings album
picker. It lists up to `REMOTE_LISTING_CONCURRENCY` collections at a time,
hands each collection's children to its caller as they arrive, and lets each
caller keep its own object/collection budget. Because the listings overlap, the
order collections complete in is not the order they were queued; callers must
not depend on it.

Concurrency pays off only when round-trip latency is what costs. Measured
against a real endpoint whose library is one flat collection of 3,107 files,
the walk took 184,557 ms at concurrency 8 against 184,693 ms serial — no
change. That server spends its time inside a *single* `PROPFIND`, computing
content hashes for every file before it can answer, so there is nothing left to
overlap. The bound is worth keeping for trees with many modest collections, and
it must not be read as a universal speedup.

Test bodies live in child modules: `model/tests.rs`, `local/tests.rs`, `planner/tests.rs`,
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
| Enumerate / stat | `PROPFIND Depth: 1`, per-directory, a bounded number of directories at a time | Pause the job; an unreadable directory is not an empty one |
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

## Backup And Sync Strategies

Every task has **Backup** (`direction=upload_only`) or **Sync**
(`direction=bidirectional`) buttons inside Settings → WebDAV. The compact,
equal-width segments read **备份 / 同步** in Chinese. Accent text
and a short underline identify the selected mode in both light/dark themes and
Liquid Glass/plain materials; the inactive underline retains its layout space. Native
grouped toggles expose the selected state to accessibility and keyboard focus.
New tasks default to Backup; existing jobs retain their saved direction and upload scope. Merely
opening Settings changes nothing. An accepted effective switch uses the durable
edit-first transition, coalesces rapid switches, completes uncertain old work in
its original context, and automatically runs the latest strategy when enabled.
Choosing Backup supersedes old bidirectional conflict choices; both file versions
are protected before any conditional replacement. A no-op switch does not restart work; modes remain editable while transferring
or globally disabled. Disabled edits persist without starting network work.

**Backup** uploads only explicitly selected local albums (or legacy `All`
scope). It never publishes remote-only or remotely edited files to the local
library. Local deletion leaves the cloud copy and archived versions intact.
Before conditionally updating a current cloud file, preserve both the incoming
local version and any existing cloud version under
`<remote-root>/.photoviewer-backup/objects/<blake3>.<extension>`. Immutable JSON
records in `records/` associate each relative path/content version with its
object. These archives are never enumerated as gallery media, even after a
switch to Sync; no automatic history cleanup is performed. Existing archive
objects and records are verified on reuse, and differing contents block backup
instead of being overwritten. A first switch/migration to Backup also archives
unchanged files with existing ordinary-sync baselines. Interrupted upload
recovery protects archive versions before retrying and never recreates a missing
local file in Backup. History currently provides per-file versions, not a
whole-library point-in-time snapshot or a Settings restore browser.

**Sync** retains the existing explicitly selected upload scope and downloads
remote media across the mapped root. Unselected local albums remain remote-authoritative when cloud content exists, subject to the deletion rule below and the explicit global Off reset.
Both-side content modifications in selected albums remain conflicts, with
version-checked resolution. Neither strategy automatically deletes remote files
or local files when the opposite side disappears.

A missing local file with a proven common baseline creates a durable
`sync_entries.suppressed_fingerprint/suppressed_size` record, independent of
`media_items`. It suppresses the **deleted content version**, not the path. An
unchanged remote version stays absent locally; a changed ETag triggers a full
content check and the same bytes remain suppressed. Strong, unchanged ETags and
matching sizes can reuse a previously verified fingerprint; weak/missing ETags
cannot. Actual changed content publishes through the normal recoverable
DownloadNew path; only successful publication/baseline commit clears suppression.
Deleting that downloaded version suppresses it in turn. Explicit local
recreation resumes normal planning. First-time remote files without a baseline
are ordinary additions. An incomplete remote listing or unavailable/incomplete
local scan never creates deletion records. Switching strategies preserves them;
removing a task removes its relationship and history from SQLite, so recreating
a task cannot infer deletions from its former client state. Remote records remain.

## Global Off Clears History, Keeps Configuration

Turning WebDAV off is an explicit history reset. Keep `sync_connections`,
credential references/passwords, surviving `sync_jobs`, local/remote roots,
strategy, SHA-256 ETag contract, upload scope and selected albums. Preserve the latest accepted static
fields even if an older transfer still uses their former configuration. Clear
all `sync_entries` (including deletion suppression and baselines), `sync_tasks`,
`sync_conflicts`, `sync_job_changes`, run timestamps, errors, transient pause and
queued execution/progress state. A confirmed pending task deletion remains a
deletion; pending conflict actions are discarded rather than executed.

`SyncStore` persists a UUID reset intent in the existing `settings` table before
cleanup and advances generations. New run/configuration admission uses a
per-database barrier; cleanup waits for active file operations and configuration
operations to retire. Operations already issued may finish publishing, but their
late records are removed by the reset transaction. Old configuration installers
are rejected atomically while the reset intent exists; scheduler errors use a
generation-checked write that cannot recreate completion timestamps after reset.

Cleanup removes the owned `sync-staging` tree, including unreferenced files,
and reserved `.photoviewer-sync-*` publication/rollback temporaries below saved
local roots. It uses the existing local-replica namespace rule, does not follow
symlink targets, and refuses to sweep a staging tree containing configured source
media. This explicit opt-out authorizes discarding pending recovery artifacts;
it never deletes ordinary local media, favorites, trash contents, thumbnails,
remote files or `.photoviewer-backup` history. No network recovery is initiated
to complete a reset. The already issued operation can settle under its original
context before its artifacts/history are cleared.

A cleanup failure preserves intent/history and reports its error in Settings;
it is not displayed as completed cleanup. Startup resumes an interrupted reset
before starting network work, and starts cleanup for an off preference if the
process exited between persisting the switch and recording the intent. A rapid
Off → On keeps the newer opt-in but waits for cleanup; only the latest saved
configurations can restart automatically. A newer Off replaces the reset token,
so an older completion cannot clear that newer intent. Pending local-change
notifications are cancelled while off/resetting. Ordinary future polling remains
idle while disabled. Reset isolation follows the database, including disabled
accounts and orphan staging files when no task exists.

Once cleanup succeeds, reopening Sync starts a new association using retained
task configuration. Formerly suppressed cloud copies may download again because
there is no longer a deletion record; Backup remains upload-only. History is
preserved across index rebuilds and ordinary strategy edits, but not across an
explicit global Off reset.

## SHA-256 Content ETags

User-facing server setup and verification instructions live in
[`README.md`](../../README.md#webdav-同步与服务器配置). Configure rclone with
`--etag-hash sha256` before enabling the task's “Server uses SHA-256 content
ETags” option. New task forms default to enabled; schema v8 migrates existing
jobs to `etag_is_sha256=false` without inferring a contract from endpoint names,
server software, or ETag shape.

The per-task flag explicitly configures `WebDavProvider::with_sha256_etags`.
Generic providers continue to expose opaque revisions. In SHA-256 mode, file
listings/stat must carry a quoted strong 64-digit hexadecimal ETag; missing,
weak or malformed values are configuration errors with the rclone option in
the diagnostic. Directories need no hash.

`VerifyContent` first rejects unequal lengths, then compares a trusted remote
SHA-256 to a stable local read. That read calculates SHA-256 and BLAKE3 together,
checks size/mtime before and after, and verifies the observed BLAKE3 fingerprint
still matches the scan. Matching digests commit the existing BLAKE3 baseline
without GET or staging, including files beyond the download verification cap.
Different digests preserve the existing selected-upload conflict and
remote-authoritative replacement rules. No hash means the existing full-download
verification path; size/time never substitutes for content proof. The saved
strong ETag then supports the existing no-change cache.

Accepted hash-mode edits use `JobEdit::EtagIsSha256`, durable desired state,
quiescence, generation guards and automatic retriggering. No-op edits do not
restart work. Global Off retains the latest accepted hash contract together
with task mappings, including pending edits, while clearing history. Existing
archive/recovery checks and remote-only downloads can still require bytes.
Network download savings do not eliminate local or server hashing I/O.

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
| Local deleted, remote unchanged | Retain remote; persist suppression of that content version |
| Local deleted, remote content changed | Download the new content; metadata/ETag changes alone do not restore |
| Remote deleted, local remains | Retain local; no automatic deletion |
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

Local observation is incremental: every run walks the tree but only
re-hashes a file whose byte length or nanosecond mtime differs from the
values stored with its last fingerprint. When both match, the stored
fingerprint is reused, so a no-change run transfers nothing and hashes
nothing. That is the trust boundary: a rewrite that preserves the exact
size and restores the exact mtime is not observed until one of them moves.
WebDAV itself offers no cheaper discovery — the server advertises `dav: 1, 2`
with no RFC 6578 sync-collection REPORT — so the remote side remains one
recursive listing, which is cheap next to hashing the library.

Writes are conditional in the same spirit: `reconcile_one` only upserts an
observation (or advances a baseline) when the computed values differ from
the stored row. A steady-state run therefore leaves `sync_entries` — and
with it the per-file `SyncStateDirty` stream — untouched, which is what
keeps a 100k-item library from turning every round into 100k UPDATEs and
100k UI events. `tests/a_no_change_run_does_not_rewrite_entries` pins the
generation-frozen behaviour.

WebDAV listings carry no content hash, so proving two same-named objects are
identical costs a full remote download. That proof is bounded twice, and both
bounds exist to prevent non-termination rather than slowness:

- Different lengths already prove the contents differ. The client records
  `InitialContentMismatch` (or resolves to `DownloadReplace` when the remote
  is authoritative) and fetches nothing; the cheap answer must not be paid
  for with the expensive one.
- Past `MAX_VERIFY_DOWNLOAD_BYTES` (256 MiB) the download cannot finish
  inside the provider's 15-minute request timeout, and every run refetches
  from zero — a livelock on one path that no amount of waiting resolves.
  The client records an `InsufficientEvidence` conflict instead of
  transferring, so the gap surfaces in Settings with a retry.

Upload recovery shares the cap. A `prepared` upload whose remote object is
past it cannot be proven landed by `stat` alone, so the task is recorded
`blocked` with a task-level English reason naming the
`upload-recovery verification limit`; recovery must not invent a conflict for
it and must not re-fetch the object every run.

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
event can be lost. Cloud configuration follows the edit-first contract below:
viewing or drafting never pauses a job; an accepted effective change requests
internal quiescence and automatic reconciliation with the latest configuration.

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

## Edit-First Configuration Contract

**Binding requirement.** The canonical
interaction and state-transition specification is
[`sync-configuration-ux.md`](sync-configuration-ux.md). Running sync must not be
an editing prerequisite: accept validated effective edits first, then internally
stop obsolete work, apply the latest desired configuration safely, and
retrigger affected surviving jobs automatically when sync is enabled. Opening,
expanding, browsing, drafts, cancellation, invalid input and no-op saves must
leave the existing run untouched. Global opt-out, confirmed deletion and
safety/validation failures have the explicit exceptions in that specification.

The implementation stores desired changes in `sync_job_changes` and serializes
acceptance, recovery, application, and restart through the configuration worker.
Strategy edits share that path with upload selection, root changes, task removal,
and conflict choices. Pending direction changes are merged with newer independent
fields rather than restoring an older scope/root. Read-only expansion and browsing
do not enqueue edits. See the real-transfer acceptance journeys in
`tests/ux_sync_album_selection.rs` and `tests/ux_sync_strategies.rs`.

## Run Lifecycle And Startup Reconciliation

`Running` is **derived**, not stored: `overview()` reports it for any active job
whose `last_started_at` is ahead of `last_completed_at`. That derivation is only
sound if every run that started also ends by recording its end, so the lifecycle
has two halves.

**Ending a run.** A run that has ended must record that fact unconditionally.
`mark_job_started` already persisted, and only `finish_run` can close it, so
skipping the completion is unrecoverable: nothing else writes
`last_completed_at`. Every outcome therefore closes the run, including the cases
that used to fall through — a configuration edit pending at the end of the run,
or sync being switched off while it was running. The *error*, by contrast, is
attributable to one specific configuration and stays guarded: it is written only
while `config_generation` still matches and no edit is queued to replace it.
Guarding the timestamp the same way was the bug: an edit applied mid-run bumps
the generation, which turned the completion into a silent zero-row `UPDATE`, and
the compact status reported "syncing" on every later launch with no failure
recorded and therefore no retry offered.

**Reconciling at launch.** A run cannot outlive the process that started it. So
at startup, before any run starts, `SyncService::reconcile_interrupted_runs`
closes every job whose `last_started_at` is ahead of its completion: those
describe runs *this* process never began. Each is closed with the stable
`SyncFailureReason::InterruptedRun` tag (`sync.interrupted_run`) rather than as
a success, because it did not finish, and the failure state carries the retry.
The reconciliation is idempotent and leaves settled jobs untouched, so it is
safe on every launch.

Known failure reasons are persisted independently of the current locale.
Settings maps the tag to `setting.sync.error.interrupted_run` when rendering task
status, upload-selection status and sync error dialogs, including periodic
refreshes. The parser also recognizes the exact Chinese sentence persisted by
older releases, so old failures display in the current language without a
schema migration or rewriting settled runs. Unknown provider/system errors keep
their diagnostic text. A known reason wrapped by `AppError::Backend` follows the
same translation path; arbitrary backend diagnostics retain their original text.

Without this, a single killed process was enough to pin the overview to
"syncing" permanently — and, because the Photos overview reveals itself on the
`not-running → running` edge, that stale state then surfaced as an overview that
popped itself open on every launch. Fixing the lifecycle, not the reveal, is
what makes that symptom go away.

## Scheduling And Triggers

While the application window is alive, remote changes are polled every 30
seconds. Filesystem and user media events debounce for 2 seconds and schedule
one catch-up if work is already active; synchronization-generated events do not
loop back into scheduling. Periodic polls skip active/reconfiguring jobs instead
of creating a permanent backlog. Global disabling suppresses all automatic
network work. Photos pull, startup, accepted effective edits and enabling sync
remain triggers. This is automatic eventual convergence, not instantaneous
server push, and no daemon runs after the application exits.
A trigger arriving while its job is already running is intercepted and
remembered: a job never runs concurrently, and after the active run settles
exactly one supplementary run fires, so repeated pulls during a long run
coalesce into a single catch-up. Creating a validated saved job while sync is
enabled must schedule that job automatically; an unchanged/invalid draft must
not schedule work. Accepted task creation uses the same coalescing configuration worker.

Discovering a remote media album never changes the persisted local upload
selection or configuration generation. Cloud-backed albums, nested albums, and
files directly under the configured root remain download-only unless their
local physical album is explicitly selected for upload. Selection matches the
immediate relative parent: selecting `camera` does not select `camera/nested`.
Unchecking a previously synced album remains effective on subsequent runs even
when that album still exists in the cloud.

Older versions automatically added cloud-backed albums to the same upload
selection table used for manual choices. Those rows have no provenance and
cannot safely be distinguished from explicit user selections. Existing
selections are retained rather than silently clearing deliberate choices; users
must uncheck any previously auto-selected albums they want to make download-only.
Legacy `All` jobs retain their documented compatibility behavior until their
upload scope is explicitly changed to selected albums.

WebDAV has a persisted global opt-in switch, **off by default**. When off,
home-pull sync and conflict resolution are disabled and the new-connection form
is unavailable. Each job has a separate collapsed album checklist. Checking a
physical folder album enables uploads for files directly in that album, while
every remote album stays in download scope. Unchecked albums use an explicit
remote-authoritative download policy: remote additions and updates publish
locally, local-only content is neither hashed nor uploaded, and remote absence
never deletes local content.

The upload checklist starts with a **Select All** switch. It is on only when
all currently listed local albums are selected; a partial selection leaves it
off and shows the selected/total count. Turning it on selects the current list,
not every future album (`UploadScope::All`); turning it off clears upload
permissions while retaining cloud downloads. Bulk changes use one atomic
`SetUploadAlbums` write and one configuration-generation increment. Individual
album rows can also be pressed to toggle their checkbox. The master switch and
checkboxes must remain editable while syncing or internally applying changes;
opening the checklist must not pause the task. Only an accepted changed scope
may request internal quiescence and automatic restart. An empty list disables
Select All; a running job does not.

Checked albums sort before unchecked albums, then by case-insensitive display
name and relative path for a stable tie-break. Sorting updates after each
successful selection change and when Settings is reopened, using the GTK
ListBox sort function without reparenting focused controls. A failed save
reports the error and may restore the last accepted selection, master state and
ordering only if no newer accepted edit exists; an older completion must never
roll back newer input. Applying changes does not freeze the checklist.

Creating a job requires non-overlapping local and remote roots, and roots
overlapping another job on the same connection are rejected. Saved tasks expose
a cloud-folder browser populated by recursively listing WebDAV collections.
Browsing alone must not pause the task. After a changed root is validated and
confirmed, quiesce old operations before safely retiring that job's old entries,
conflicts, operation records and unreferenced staging artifacts, so observations
from the previous collection cannot be reused. Automatically run the latest
mapping when sync is enabled; never leave the task awaiting a manual Photos
pull. Changing roots never deletes cloud content, and evidence needed for an
uncertain in-flight result must survive until reconciliation proves its outcome. Changing a remote root warns that cloud files may
replace same-name local files in albums not selected for upload.

Removing a job cascades its entries, conflicts, unfinished operations, and
staging artifacts without removing local or remote media. A connection and its
keyring credential are removed only when no other job references it; a
credential-cleanup error is reported after the relationship is gone.

Recommended starting budgets: at most 4 transfers globally, 2 per connection,
1 per object; remote polling at 30s, honoring `Retry-After` and
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
completed upload by downloading and hashing the current remote object — within
the verification cap — and a completed download from the published local
fingerprint plus the remote version, before committing.

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
upload, recovery replaces the remote object only when the local album is still
selected for upload, its downloaded bytes are a strict prefix of the preserved
upload snapshot, and the server supplies a strong ETag; the replacement is
conditional and downloaded again for hash verification. If the local file is
itself a strict prefix of the snapshot, recovery restores it even when the
truncation point differs from the current remote prefix. An unchecked album may
verify and commit a write that already completed, or restore the preserved local
copy, but must not issue another upload. Incomplete remote writes remain blocked
until the album is explicitly selected again. Unresolved upload paths are
excluded from normal download planning, including when the album was
subsequently unchecked, and the snapshot is retained until both copies are
proven complete. Recovery of a `prepared` upload stats the remote object
first; when that object is past the verification cap, whether the
interrupted upload landed is recorded as `blocked` with the reason on the
task instead of streaming the oversized object to prove it. Blocked tasks are
excluded from recovery sweeps, so the path settles through normal planning —
typically the capped verification conflict — rather than refetching forever.

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

Deletion propagation remains **off** in both strategies. A local absence with a
baseline suppresses that cloud content version without deleting it. Sync can
restore a changed remote content version; Backup never automatically restores
remote content. A remote absence never deletes a local file. Absence without a
baseline is a normal first association. Ordinary DAV layout has no cross-device shared deletion log, so a local
deletion record can stop *this* client from restoring a file, but cannot stop a
fresh or reset client from re-uploading a copy. Cross-device global deletion
convergence would need a shared remote manifest, device identity, deletion
confirmation, and a garbage-collection protocol — do not promise it.

## Cloud Badges And Progress

Live progress sessions belong to their database. Closing or resetting one
library cannot erase another library's preparing/active progress, and unrelated
activation slots cannot prevent the owning library from closing its session.
After a reset, a newer opt-in marks a catch-up on a retiring worker rather than
being skipped until the next periodic poll. Cleanup stays pending through that
handoff, so observers cannot mistake the gap before scheduling for an idle task.


`SyncStore::media_cloud_states()` checks a bounded set of image/video IDs
against enabled jobs and their selected physical albums. It returns no state
while the global opt-out is set, no state outside those albums, `cloud-off` when
a local item lacks a current completed baseline, and `cloud` when
`state='synced'`, the entry's local fingerprint and size match the baseline, and
the indexed file size and nanosecond mtime match the observation. The media
scanner intentionally leaves `blake3_hash` empty, so badge reads must not rely on
that column or hash files on the GTK path.

The global opt-out gate is load-bearing, not a shortcut. Global Off keeps saved
configuration, so `sync_connections.enabled` stays `1` and its baselines survive
until the asynchronous reset transaction finishes. Filtering on the connection
alone would keep drawing a cloud for a feature the user just turned off, and a
reset that is still draining jobs — or one that failed and kept its history —
would leave the icon up indefinitely. Badges therefore read the same
authoritative `webdav_sync_enabled` preference as `SyncStore::overview()` rather
than waiting for the database to catch up.

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
disabled. This projection is polled continuously from `set_db_pool`, not only
while the disclosure is open: the panel reveals itself on the not-running →
running edge, so a run started by application launch or by an accepted
configuration change becomes visible without a pull, and a poll that stopped at
the folded panel would make exactly that case silent. The completed label counts
distinct image and video paths with a proven common baseline across enabled
jobs, and separately counts distinct unresolved image conflict paths. A conflict
does not erase an earlier proven baseline. Blocked upload recovery keeps the
overview in failed status even when the last run finished, so a protected
unresolved upload is never labeled complete.

The Photos overview displays "正在同步，已同步 X/Y" (or "正在上传，已同步 X/Y")
using persistent, library-wide counts in both phases. X counts distinct local
paths with a proven common baseline, including previous successful runs. Y is
the union of those historical paths and `sync_inventory`, the complete known
upload/download scope across enabled connections. Both count each local path
once; changed files, repeat verification, and pending conflicts cannot add a
second denominator slot for a historical path. Completed labels retain X/Y and
optionally append the unresolved image-conflict count.

While no path is known yet the overview shows "同步准备中" instead of a
fraction. Y cannot exist before discovery has proven scope, and a large library
spends minutes listing — measured against a real endpoint, a single collection
holding 3,107 files took 162 seconds on its own. Rendering `0/0` for that long
is indistinguishable from a stalled run; naming the phase is the honest
alternative. The fraction returns as soon as any scope is known, so this only
covers the pre-discovery window and never hides a partial result.

Discovery grows the inventory while it runs. Each batch of remote paths the
walk has proven is appended in a generation-guarded transaction, so the
overview's denominator becomes visible during the enumeration instead of
staying empty until the whole tree has been listed. The append never removes a
row, so a discovery that fails part-way can only leave a superset of the last
complete scope, never a smaller one; the next complete discovery converges on
the exact scope through the authoritative write below. Remote files are only
published while the job can download — in `UploadOnly` they are not part of the
scope at all.

After complete remote discovery and a successful local scan, the service saves
the job's entire inventory in one generation-guarded database transaction,
before the first transfer. That write is authoritative: it reconciles away
anything the running walk appended, and it is the only place that decides
membership for locally proven paths. Remote files count in download-capable
jobs; local files count only in upload-capable jobs and the user's selected
upload scope. Thus unselected local-only albums and remote-only files in
UploadOnly mode are excluded. Protected unfinished uploads remain in scope. An
unchanged snapshot skips rewriting inventory rows. Failed/incomplete discovery
never removes from the previous inventory, and interrupted transfers leave the
full denominator in place for restart. New configuration generations invalidate
obsolete snapshots without removing history; the next complete scan refreshes
the scope. Known totals may grow when new files or jobs are discovered,
including sequential jobs in one trigger. Before first discovery, only
previously known paths can be counted. Schema v7 seeds legacy inventory from
proven baselines and unresolved sync entries. Media-index rebuilds preserve it;
job removal cascades, and an explicit global opt-out clears it with the other
sync history.

`SyncService` still tracks live transfer progress in an in-memory, process-wide
session that is never persisted. The home pull and each application launch open
the session (`trigger_saved_jobs_once`). Each job pre-plans every path with the
same pure planner used by `reconcile_one` and adds planned download/upload
totals. Those per-run counters remain for diagnostics; the overview uses only
the live phase to distinguish upload activity. While a file streams, byte
counters advance through the `TransferProgress` sink attached to the
`WebDavProvider`; bytes surface only in STORAGE-target logs, never in the
overview label. Sessions are bounded to a trigger: crash-recovery transfers
before planning are not counted, totals accumulate across sequential jobs, and
a duplicate trigger never opens or closes the active session.

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
- **WebDAV has no change feed**, so every cycle costs the server `O(all
  bytes)`: answering a `PROPFIND` with content-hash ETags requires recomputing
  every member's hash, and there is nowhere to persist the result. Measured on a
  1.7 GiB host with a 17 GB flat collection, one listing took 156 s and a
  second identical listing took 160 s — page cache cannot hold the working set,
  so nothing is reused. This is paid every cycle, at every trigger, and scales
  linearly: 100 GB of library is roughly 11 minutes per cycle. It is the reason
  a large library outgrows WebDAV regardless of client-side tuning.
- With SHA-256 content ETags the client can prove a cloud copy identical from
  metadata alone, which replaces a full download per path with a local read.
  That switch is a per-job contract, not a capability probe: it is off for jobs
  that predate the column, and on by default only for newly created ones.
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
| C14 | Automatic cleanup uses persistent ownership/recovery evidence; explicit global Off discards owned sync temporaries after quiescence, preserving ordinary media and remote history |
| C15 | Local deletion suppresses its cloud content version across restarts/index rebuilds; explicit global Off clears that history; otherwise only changed content or local recreation resumes Sync; Backup never downloads automatically |
| C16 | Logs correlate discovery, execution, and recovery by `JobId` / `EntryId` / `OperationId`, recording error classification and retry reasons |
| C17 | Running/quiescing jobs remain editable; viewing, drafting, cancellation, invalid input and no-ops have no pause/restart side effects |
| C18 | An accepted effective edit drives durable desired state, safe quiescence and automatic latest-configuration reconciliation; never require manual pause/resume or a Photos pull, and never restart opted-out/deleted jobs |
| C19 | Serialize/coalesce edits per job; obsolete runs and callbacks cannot overwrite newer desired state, claim new old-generation work, or mark the latest generation complete |
| C20 | A remote walk lists a bounded number of collections concurrently (`REMOTE_LISTING_CONCURRENCY`); every collection is visited exactly once, the first listing error aborts the walk, and the existing object/collection budgets still apply |

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
| Local deletes a synced file, either strategy | Remote stays; unchanged content does not auto-restore; changed content returns only in Sync | C15 |
| Same path and mtime, changed content | Thumbnails and viewer show the new content; the cache never lags permanently | C06, C13 |
| Discover a remote album, including nested/root albums | Download cloud media without changing local upload selection or configuration generation | C06 |
| Uncheck an album that exists in the cloud | Continue downloads, never auto-reselect it or upload local-only files/edits | C06 |
| Uncheck an album with an interrupted upload | Verify completed writes without re-uploading; retain incomplete-write artifacts until explicitly selected again | C04, C08, C14 |
| Open Settings, expand upload scope, browse a cloud folder, cancel or submit a no-op | Existing run continues; no configuration-generation bump or restart | C17 |
| Change upload scope while a real file is transferring | Edit accepted without a prior pause; safely settle old work and automatically run the latest scope | C04, C06, C18 |
| Make rapid edits while an earlier edit is applying | Keep controls editable; coalesce latest desired state; stale callbacks never restore older choices | C06, C19 |
| Apply a changed root or enable/save a task | Validate/accept first; automatically run the latest surviving eligible configuration without a Photos pull | C04, C18 |
| Disable global sync during reconfiguration | Keep latest accepted task configuration, quiesce, clear history and temporaries; a newer opt-in waits for reset and a newer opt-out wins | C18, C19 |
| Confirm task deletion during reconfiguration | Keep deletion authoritative; no pending trigger resurrects a deleted job | C18, C19 |
| Change filter, clear cache, or rebuild the library | Old tasks invalidated or paused; baselines and unfinished artifacts retained | C06, C08, C14 |
| Unicode paths, encoding, symlinks, name collisions | Valid names sync exactly; out-of-root and collisions block execution | C09, C10 |
| Real DAV and Flatpak | Network, certificates, credentials, conditional write, and recovery all work | C01, C11 |

Mock DAV and the fake provider cover repeatable fault injection; they do not
substitute for real-service acceptance. Verify at least one ordinary DAV
server, Nextcloud, and the user's actual NAS or drive, including Flatpak
network, certificate, and credential access. Passing the mock suite is not
evidence of service compatibility.
