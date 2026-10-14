# Sync Module

## Scope

This module owns PhotoViewer's synchronization with an independent Rust
`Luyao-1024/syncserver` service. The previous WebDAV file-sync engine was
removed outright: no provider, planner, reconciler or migration for it remains,
and a schema-8 library is upgraded by dropping the WebDAV sync tables and
recreating them empty (media, albums, favorites and edits are untouched).

C1.1 (current state) delivers the **connection and capability probe only**.
A native connection can be created and probed, its accepted capability scope is
pinned onto the connection row, and a job can map a local root to a configured
zone. Nothing synchronizes media yet: no receive cycle, no lease, no upload,
and no settings surface. The WebDAV-era settings group, strategy controls,
conflict buttons and cloud-folder browser are gone; the native configuration UI
arrives in C1.5.

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
| `src/core/sync/native/provider.rs` | `NativeProvider`: a connection + pinned scope + zone; C1.2 adds the typed zone/record/delta calls |
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

## Dirty Truth, Badges, Overview

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

Unit tests live beside the code (`native/protocol/tests.rs`, `store/tests.rs`,
`service/tests.rs`); the real-server acceptance suite is
`tests/native_sync_probe.rs` (see `docs/testing.md`). Schema shape and the
schema-8 → 9 upgrade are covered in `tests/db_migrations.rs`.
