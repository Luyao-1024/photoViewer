# Sync Module

## Scope

This module owns PhotoViewer's synchronization with an independent Rust
`Luyao-1024/syncserver` service. The previous WebDAV file-sync engine was
removed outright: no provider, planner, reconciler or migration for it remains,
and a schema-8 library is upgraded by dropping the WebDAV sync tables and
recreating them empty (media, albums, favorites and edits are untouched).

C1.1 through C1.4 are implemented. A native connection can be created and probed,
its accepted capability scope is pinned onto the connection row, a job can map a
local root to a configured zone, and a receive cycle can bootstrap that zone and
keep an ordered, replayable delta of everything the cloud says — durably,
resumable mid-page, and validated as a complete photo graph before it is
projected.

The settings surface is still absent (C1.5). Everything else is in place: the
connection and probe, the durable receive path, the dirty-truth/lease
coordinator, and complete media transfer — a local photo is snapshotted, digested,
paired with a thumbnail from the same copy, uploaded, downloaded, verified and
published. A photo becomes visible only when its whole pair is usable, and
`applied` cannot advance past work that is still owed. The WebDAV-era settings group, strategy controls, conflict
buttons and cloud-folder browser are gone; the native configuration UI arrives
in C1.5.

| Read | Owns |
|---|---|
| [Client contract](native-sync.md) | cycle/lease/dirty/callback/conflict contract for the full C1 client |
| [Wire mapping](native-sync-protocol.md) | endpoints, DTO/capability names, scope/auth, limits, errors |
| [Storage/publication](native-sync-storage.md) | actor/migration, dirty/version facts, receive progress, publication |
| [Implementation/acceptance](native-sync-plan.md) | C1.1–C1.6 patch order and NC01–NC20 evidence |

## Key Files

| File | Owns |
|---|---|
| `src/core/sync/native/protocol.rs` | typed DTOs, endpoint normalization, capability validation, `NativeClient` HTTP (readyz/capabilities/probe), CloudKit base building, error envelope parsing |
| `src/core/sync/native/lease.rs` | the exclusive upload lease: acquire/check/phase/renew/release and the conservative monotonic deadline |
| `src/core/sync/native/modify.rs` | record mutations, idempotency keys, and issued-operation receipts |
| `src/core/sync/native/provider.rs` | `NativeProvider`: a connection + pinned scope + zone |
| `src/core/sync/native/receive.rs` | receive-side wire types: snapshot/query/zone-delta DTOs, the record envelope, and the shared authenticated JSON POST |
| `src/core/sync/native/records.rs` | the typed remote graph: pair validation, path safety, resource expectations |
| `src/core/sync/store/receive.rs` | durable receive state: bootstrap markers, the ordered inbox, promotion, and the `applied` gate |
| `src/core/sync/store/dirty.rs` | dirty truth: category bits, versions, edit events, the derived uplink view, and confirmation |
| `src/core/sync/service/coordinator.rs` | one cycle: receive, acquire, gate, phases, dispatch, callbacks, release |
| `src/core/sync/content.rs` | stable snapshots, dual-encoded digests, bounded thumbnail generation |
| `src/core/sync/native/assets.rs` | asset reservation, byte upload, session status, describe and download |
| `src/core/sync/store/publication.rs` | resource obligations, publication intents, and the admission gate |
| `src/core/sync/service/transfer.rs` | the download path: fetch, verify, stage, publish, admit |
| `src/core/sync/device.rs` | this installation's stable device identity |
| `src/core/sync/service/receive.rs` | one receive cycle: bootstrap when needed, then a fixed-height delta round |
| `src/core/sync/model.rs` | `ProviderKind`, `SyncDirection`, `UploadScope` |
| `src/core/sync/store.rs` | `SyncStore`: jobs/connections/inventory/overview, native job creation + zone/pin validation, run lifecycle |
| `src/core/sync/store/configuration.rs` | durable accepted-edit queue (`sync_job_changes`) |
| `src/core/sync/store/reset.rs` | global-off history reset |
| `src/core/sync/service.rs` | `SyncService`: `probe_native_connection`, `create_native_job`, interrupted-run reconciliation |
| `src/core/sync/service/configuration.rs` | edit-coalescing worker: applies accepted edits, owns quiescence |
| `src/core/sync/service/reset.rs` | reset worker and configuration-transition barrier |
| `src/core/sync/error.rs` | shared `SyncError`/`SyncResult` |
| `src/core/sync/remote_path.rs` | job-relative path rules and the owned `.photoviewer-sync-*` temp namespace |
| `src/platform/credentials.rs` | Bearer API key in the platform secret store; SQLite keeps only the reference |
| `src/core/schema.sql` | sync tables, native columns, receive tables |

## Connection And Probe

`NativeEndpoint::parse` accepts only an origin root: no userinfo, query,
fragment or path prefix, and cleartext HTTP only for a **literal** loopback
address (`localhost` is a DNS name and is refused). `NativeClient::probe` runs
`GET /readyz`, then authenticated `GET /v1/capabilities`, then
`validate_capabilities`, which requires: the service identifies itself as
`syncserver`, `protocolVersion` 1, all twelve required capability bits, the
`SHA256` checksum algorithm, a well-formed `databaseGeneration`, and an
announced `publicBaseURL` that shares the configured origin. A textual `stage`
never enables anything.

The accepted document is pinned onto the connection (`native_pinned_scope`);
later cycles compare against it instead of re-deriving trust. An invalid draft
writes nothing: no connection row, no credential, no generation bump.

A native job names its zone explicitly (`sync_jobs.native_zone_name`,
NOT NULL); `remote_root` no longer exists. Two active jobs may not map the same
endpoint+zone into different local roots, and overlapping local roots are
refused — the UNIQUE constraints on `sync_jobs` back the application-level
checks that name the offending task.

## Receive Position And The Applied Gate

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

A dirty bit is a field on the existing record, never a queue: 1=NEW, 2=METADATA,
4=CONTENT, 8=DELETE. Each of the three categories carries its own version, its
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
declared digest — and generates the thumbnail from that copy, so the pair
describes one version. The digest is stated twice in two encodings (hex for
reservation and photo field, Base64 for a descriptor); `ContentDigest` is the
join and never a re-hash.

Downloads verify what arrived against what the projection promised, before
anything is written. Publication is ordered: staged → published (file in the job
root) → verified (cache artifact exists) → complete (both roles verified) →
admitted (media row written, exactly once). A crash between any two steps leaves
the previous complete version readable and the work still owed; a photo that
left the projection cannot be resurrected from a stale intent.

## Badges And Overview

Promotion also rejects an incomplete graph: an asset without its master, a master
no asset references, mismatched content generations, a cross-zone reference, a
duplicate or unsafe relative path, or a thumbnail digest that is not SHA-256 hex
are all refusals, never partial accepts. Unknown record types are retained in
the projection and never counted as complete supported media.

`sync_entries.native_dirty` (bitmask 0–15: 1=NEW, 2=METADATA, 4=CONTENT,
8=DELETE) is the only dirty truth; there is no second queue. Cloud badges read
`native_confirmed_generation IS NOT NULL AND native_dirty = 0` — a confirmed
complete pair with nothing outstanding. The overview counts a media item after
its required complete pair is confirmed, using `sync_inventory` as the eligible
denominator. Nothing writes these yet beyond defaults; the mutation hooks land
in C1.3.

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
`docs/testing.md`). Schema shape and the schema-8 → 9 upgrade are covered in
`tests/db_migrations.rs`.
