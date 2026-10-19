# Sync Module

## Scope

This module owns PhotoViewer's synchronization with an independent Rust
`Luyao-1024/syncserver` service. The previous WebDAV file-sync engine was
removed outright: no provider, planner, reconciler or migration for it remains,
and no development-schema migration ladder remains. Column drift is detected
at startup; [single-table recovery](storage.md#database) recreates at most one
incompatible table and clears sync history with row deletions so synchronization
starts from a fresh snapshot. Compatible configuration and library data survive;
multiple incompatible tables are reported without automatic deletion.

C1.1–C1.5 supported paths have focused runtime coverage; exhaustive recovery and the remote-release gate remain separate. See [implementation notes](native-sync-integration.md). A native connection can be created and probed,
its accepted capability scope is pinned onto the connection row, a job can map a
local root to a configured zone, and a receive cycle can bootstrap that zone and
keep an ordered, replayable delta of everything the cloud says — durably,
resumable mid-page, and validated as a complete photo graph before it is
projected.

The cloud-sync Settings category directly contains the global switch, overview,
empty-state guidance and the current account’s folder cards. Each task has a status header with a visible
pause/resume button; the header itself has no activation action. A separate
"Task details" expander reveals identity and album scope. Run control never
changes expansion, and viewing details never changes job state or configuration.
The account surface owns switch-account and authentication actions; folder overflow menus contain removal only. Only one account is enabled at a time, and a switch drains the old account before admitting the new one. Saved account histories remain separate; see [account switching](native-sync-accounts.md). Polling
updates existing task widgets so progress changes preserve expansion and focus.

The native Settings surface supports connection probes, explicit album scope,
manual/automatic synchronization and pause/resume. Photo browsing and management
use the existing library, albums and viewer; Settings has no separate cloud-photo
management surface. Versioned photo-decision APIs remain available internally. The
implementation boundaries are the connection and probe, the durable receive path, the dirty-truth/lease
coordinator, and complete media transfer — a local photo is snapshotted, digested,
paired with an existing local thumbnail, uploaded, downloaded, verified and
published. A cloud photo becomes visible when its thumbnail is ready; the original can arrive later, and
`applied` cannot advance past work that is still owed. Native user choices are described in the [usage guide](native-sync-usage.md).

Native uplink reads the existing local thumbnail cache (Large, Medium, then
Small) using its normal source path/mtime identity. It does not decode originals,
extract video frames, request thumbnail generation, or require source-digest
provenance for the cache. Missing thumbnails leave the photo dirty and deferred,
without an upload error or consuming an attachment slot. A later automatic cycle
retries after local thumbnail generation. Fitting JPEG/PNG caches are uploaded
byte-for-byte; conversion or size reduction reads only the cached thumbnail.
Metadata-only changes and deletions do not require a thumbnail.

Protocol definitions remain exclusively in the sibling server repository.

| Read | Owns |
|---|---|
| [Client integration](native-sync.md) | cycle orchestration, local dirty/callback data and conflict integration |
| [Wire mapping](native-sync-protocol.md) | endpoints, DTO/capability names, scope/auth, limits, errors |
| [Storage/publication](native-sync-storage.md) | actor/migration, dirty/version facts, receive progress, publication |
| [Implementation/acceptance](native-sync-plan.md) | C1.1–C1.6 patch order and NC01–NC20 evidence |

## Relational persistence and batch timing

Cloud-sync tables contain scalar columns and typed relations, including pending records, confirmed baselines, account capabilities, drafts and recovery evidence. The one-time legacy conversion preserves unfinished obligations and exact issued requests; [storage rules and indexes](native-sync-relational.md) describe the boundary. A 100-record batch targets one second from HTTP request through local application and progress checkpoint. Page logs expose HTTP, persistence, application and checkpoint time separately. Shared-asset reconciliation locates baseline owners through the indexed asset field before looking for pending publications; a growing fresh-download queue does not repeatedly scan unrelated entries and publications.

## Key Files

| File | Owns |
|---|---|
| `src/core/sync/store/records.rs` | typed record headers, field values, baselines and versioned digest/event storage |
| `src/core/sync/store/state.rs` | scalar capabilities/accounts, bootstrap markers, configuration drafts and legacy conversion |
| `src/core/sync/store/issued.rs` | lossless issued request/preparation reconstruction |
| `src/core/sync/store/recovery.rs` | structured conflict and file-publication recovery state |
| `src/core/sync/native/protocol.rs` | typed DTOs, endpoint normalization, capability validation, `NativeClient` HTTP (readyz/capabilities/probe), CloudKit base building, error envelope parsing |
| `src/core/sync/native/lease.rs` | the exclusive upload lease: acquire/check/phase/renew/release and the conservative monotonic deadline |
| `src/core/sync/native/modify.rs` | record mutations, idempotency keys, and issued-operation receipts |
| `src/core/sync/native/provider.rs` | `NativeProvider`: a connection + pinned scope + zone |
| `src/core/sync/native/receive.rs` | receive-side wire types: snapshot/query/zone-delta DTOs, the record envelope, and the shared authenticated JSON POST |
| `src/core/sync/native/records.rs` | the typed remote graph: pair validation, path safety, resource expectations |
| `src/core/sync/store/receive.rs` | durable receive state: bootstrap markers, temporary pair fragments, metadata admission, and the `applied` gate |
| `src/core/sync/store/dirty.rs` | dirty truth: exclusive states, category versions, edit events, the derived uplink view, and confirmation |
| `src/core/sync/service/coordinator.rs` | one cycle: receive, acquire, gate, phases, dispatch, callbacks, release |
| `src/core/sync/content.rs` | stable snapshots, dual-encoded digests, bounded thumbnail generation |
| `src/core/sync/native/assets.rs` | asset reservation, byte upload, session status, describe and download |
| `src/core/sync/store/publication.rs` | resource obligations, publication intents, and the admission gate |
| `src/core/sync/service/transfer.rs` | the download path: fetch, verify, stage, publish, admit |
| `src/core/sync/device.rs` | this installation's stable device identity |
| `src/core/sync/service/receive.rs` | one receive cycle: bootstrap when needed, then a fixed-height delta round |
| `src/core/sync/model.rs` | `ProviderKind`, `SyncDirection`, `UploadScope` |
| `src/core/sync/store.rs` | `SyncStore`: jobs/connections/media-derived overview, native job creation + zone/pin validation, run lifecycle |
| `src/core/sync/store/configuration.rs` | durable accepted-edit queue (`sync_job_changes`) |
| `src/core/sync/store/reset.rs` | global-off history reset |
| `src/core/sync/service.rs` | `SyncService`: `probe_native_connection`, `create_native_job`, interrupted-run reconciliation |
| `src/core/sync/service/configuration.rs` | edit-coalescing worker: applies accepted edits, owns quiescence |
| `src/core/sync/service/reset.rs` | reset worker and configuration-transition barrier |
| `src/core/sync/error.rs` | shared `SyncError`/`SyncResult` |
| `src/core/sync/remote_path.rs` | job-relative path rules and the owned `.photoviewer-sync-*` temp namespace |
| `src/platform/credentials.rs` | Bearer API key in the platform secret store; SQLite keeps only the reference |
| `src/core/schema.sql` | sync tables, native columns, receive tables |

## Sync Activity Logs

Cloud synchronization is a normal `INFO` activity under target `sync`; no debug
switch is needed at the default log level. Use `RUST_LOG=sync=info` to isolate it.
Session files and stderr use the existing [diagnostics pipeline](diagnostics.md).

Each admitted `run_cycle` logs a batch start and one terminal result with the
same `job_id` and `cycle_id` (also the stored native cycle ID). The terminal
summary records sent/confirmed mutations, failed items, remaining obligations,
work deferred by the 5-photo asset / 20-photo metadata limits, lease skip/release
state and elapsed milliseconds. `complete` means no reported obligations remain;
`pending` means more work remains, `partial` means items failed, and `deferred`
means uplink was skipped. A hard failure, including failure to persist the run
result, logs a `WARN` terminal event instead of a success summary. Individual
upload failures also log `WARN` with the entry and cycle IDs.

Every full-snapshot page and delta page logs its received/saved record counts,
page number, zone, continuation presence and elapsed time **after persistence**.
These events report durable metadata receipt, not completed photo publication.
A thumbnail that first makes a photo displayable emits `MediaUpserted`; original
publication or thumbnail updates to an already visible photo emit a targeted
`MediaUpdated` with availability fields. Photos and album grids apply these
through the [incremental browsing path](browsing.md), without a whole-layout
replacement for each completed resource.

A nonempty resource batch logs its requested and already-staged counts before
work starts. Resuming checks staged files against the persisted size/SHA-256
before credentials or HTTP; valid bytes are reused. Each resource reconciles
only publications referencing that asset, with a final whole-job reconciliation
at the end of the batch. Already displayed thumbnails therefore do not cause
repeated scans of every publication before fresh downloads can proceed. The
terminal download summary reports reused resources separately from new fetches.

Resource download logs appear after verification/staging; successfully admitted
photo pairs report publication batch counts. Nonempty download batches report
requested/fetched/admitted/failed counts and elapsed time. No byte-progress loop,
heartbeat or UI poll emits `INFO` activity. Credentials, lease proofs, opaque
snapshot/delta tokens, continuation markers and raw record bodies are not log
fields.

## Connection And Probe

PhotoViewer applies a root-origin connection policy and rejects insecure non-loopback
addresses and credential-bearing URLs. `NativeClient::probe` discovers readiness and
capabilities and checks the features the client actually needs. The endpoint shapes,
field meanings, protocol versions and checksums come exclusively from the
[server contracts](native-sync-protocol.md); they are not specified again here.
A textual stage never enables a client feature.

The accepted document is pinned onto the connection (`native_pinned_scope`);
later cycles compare against it instead of re-deriving trust. An invalid draft
writes nothing: no connection row, no credential, no generation bump.

A native job names its zone explicitly (`sync_jobs.native_zone_name`,
NOT NULL); `remote_root` no longer exists. Two active jobs may not map the same
endpoint+account+zone into different local roots, and overlapping local roots within that account are
refused — the UNIQUE constraints on `sync_jobs` back the application-level
checks that name the offending task.

## Receive Position And The Applied Gate

Remote metadata is admitted directly to `media_items`: storage_location 1=local,
2=cloud without a downloaded original, 3=both. `thumbnail_ready` means locally
generated for local originals and downloaded for cloud-only rows. Metadata is not
kept in a second full inventory; only incomplete pair fields, per-photo download
work and file-publication captures survive between pages. A cloud thumbnail can
be displayed before its original arrives. Original publication then changes 2→3.


Four token positions, never interchanged: `snapshot_token` pins a full read at
one height; `ingest_token` is where the delta stream resumes; `target_token` is
the height of the newest complete projection; `applied_token` is the height
this client has actually finished catching up to.

The snapshot's zone token means "nothing after H", so the bootstrap pages *are*
the inventory; a delta round resumed from it reports only later changes. While
`moreComing` is true the returned token pins the round's height, so a
concurrent write cannot leak into the pages that follow it — only the final
page's token is re-based and may start the next round.

`applied` advances only when the projection at `target` owes nothing. Promotion
materializes one pending resource per photo role, so in the current stage
`applied` correctly stays behind `target` for any non-empty zone. A changed
scope, namespace or database generation discards every previous claim: the old
applied position says nothing about work that was never in scope.

## Dirty Truth And Local Change

Trash/restore is metadata, carried as the optional `isTrashed` field through the
server's generic field support. Permanent physical removal leaves the existing
sync entry, cloud IDs, revisions and deletion event durably at dirty=4. Ordinary
scans, restarts, retries and later remote edits never discard this record. A
successful guarded cloud deletion removes it; an explicit account switch or
global Off may clear it. Failed physical deletion creates no deletion intent.
A never-published item that already has a sync entry is settled as already
absent only after the current-watermark upload gate succeeds. Deleting a media
index row that has never had a sync entry does not enroll it for deletion.

Global Off clears history and durably disables local-change tracking, independently
of the selected account. Neither cleanup nor later edits while Off may recreate
entries. Tracking resumes only after an explicit opt-in and the reset barrier has
finished. Files removed while Off may still have stale media index rows at the
next startup; pruning those rows must not create deletion obligations. A fresh
receive can therefore download the cloud photos again.


Dirty is a state on the existing record, never a queue: 0=CLEAN, 1=NEW,
2=MODIFIED (metadata or content), 4=DELETE after physical permanent removal. Each change category carries its own version, its
own acknowledged counter and its own edit event, and the uplink view is derived
from those fields at dispatch time.

A user's change and the obligation it creates are written in **one**
transaction — the favorite toggle is the current example — so a crash can never
leave a changed photo the cloud never learns about, or an obligation for a
change that never happened. A photo inside a job root that has no entry row yet
is enrolled as NEW rather than ignored; a photo outside every root, and a
trashed photo, gain nothing. Row disappearance is never a delete intent.

Confirmation is version-matched: a callback carrying the version that was sent
clears that category, and a callback carrying an older one registers the cloud
identity it proved but leaves the newer obligation standing.

## Complete Media

A local upload snapshots the original into owned staging first — a user editing
the file mid-upload would otherwise produce bytes that no longer match the
declared digest — and reuses the existing source-path/mtime thumbnail cache; it does not decode
originals to produce upload thumbnails. The digest is stated twice in two encodings (hex for
reservation and photo field, Base64 for a descriptor); `ContentDigest` is the
join and never a re-hash.

Downloads verify what arrived against what the projection promised, before
anything is written. Publication is ordered: staged → published (file in the job
root) → verified (cache artifact exists) → complete (both roles verified) →
original ready (existing metadata row changes cloud=2 to both=3). A crash between any two steps leaves
the previous complete version readable and the work still owed; a photo that
left the projection cannot be resurrected from a stale intent.

## Badges And Overview

Received photo pairs validate references, generations, safe paths and asset
descriptors only for affected photos. Incomplete cross-page pairs remain on their
own entries; unrelated confirmed photos are not replayed or compared.

`sync_entries.native_dirty` (0=CLEAN, 1=NEW, 2=MODIFIED, 4=DELETE) is the only dirty
truth. Cloud badges use the media's declared location plus its current-account
cloud identity. Overview counts fully confirmed, clean, both=3 photos with ready
thumbnails and no pending publication. Its denominator derives from current-account
entries, photo fields, explicit upload scope and persistent pending deletions.
There is no separate sync_inventory or per-run counter that resets earlier success.

## Credentials

One Bearer API key per connection, stored under the keyring service
`io.github.luyao_1024.photoviewer.native_syncserver`. The database holds only
`credential_ref`. Deleting a task deletes its credential; a failed creation
leaves no stranded secret.

## Testing

Unit tests live beside the code (`native/protocol/tests.rs`,
`native/receive/tests.rs`, `native/records/tests.rs`, `store/receive/tests.rs`,
`store/tests.rs`, `service/tests.rs`); the real-server acceptance suites are
`tests/native_sync_probe.rs` and `tests/native_sync_receive.rs` (see
`docs/testing.md`). Current schema shape is covered in `tests/db_migrations.rs`; single-table
recovery, preservation, rollback and fresh sync bootstrap are covered in
`tests/db_schema_recovery.rs`. The focused asset cases
`uplink_batches_limit_assets_to_5_and_metadata_to_20_and_resume`,
`two_clients_create_receive_edit_and_replace_a_complete_photo` and
`upload_failures::pending_asset_failure_does_not_block_other_photos_and_retries_next_cycle`
capture `sync=info` events from real SyncServer runs and check batch correlation,
counts/status, receipt/download/publication coverage and failure visibility.
