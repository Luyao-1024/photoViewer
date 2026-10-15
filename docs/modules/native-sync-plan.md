# Native SyncServer C1 Implementation And Acceptance Plan

Status: C1.1–C1.4 delivered 2026-10-15 against `Luyao-1024/syncserver@2d9f9a7`.
The WebDAV engine was removed outright rather than kept alongside; native is the
only provider. Follow [main contract](native-sync.md),
[wire mapping](native-sync-protocol.md), [storage/publication](native-sync-storage.md)
and [testing rules](../testing.md).

## Scope Decisions

C1 enables Native **Sync/Bidirectional** only. Native Backup is not exposed as
supported until its immutable-history/catalog and restore behavior has a
separate acceptance contract; server history retention alone is not a Backup
client. An unsupported strategy fails validation before stopping current work,
following edit-first UX.

Synchronize complete original/thumbnail pairs and the supported favorite/path/
capture-date fields. Preserve unknown server fields with patch updates. No
remote placeholders, thumbnail-only gallery admission, original lazy loading,
Apple login, private CPL indexes, upload resume, sharing or raw SQLite/WAL sync.
Limits and format errors are visible failures, never silently skipped successful
items. Do not promise metadata synchronization for album covers/order/edit history.

One native job maps one local root/zone; duplicate or ambiguous mappings are
rejected at configuration validation. Multiple zones/jobs can receive concurrently.
A local lease broker serializes/skips competing uplink admission per canonical
endpoint/namespace; it does not grant upload authority. Each job obtains its
own server lease once for its zone and settles/releases it before another native
uplink cycle. Never combine unrelated job configurations into one frozen task
batch or stop unrelated jobs merely because a different job was edited.

## Modules And Existing Boundaries

Paths below are planned new code; only existing paths are linked in module guides.
Keep one in-process service on the current Tokio runtime and the existing DbActor.

| Boundary | Planned responsibility | Reuse / guard |
|---|---|---|
| `sync/native/protocol.rs` | typed DTOs, opaque tokens/revisions, capability/limit/error validation, authenticated HTTP | transfer progress and bounded HTTP helpers; no SQL/GTK |
| `sync/native/coordinator.rs` | cycle state, local admission, server lease/deadline/heartbeat, catch-up, phase dispatch and callback epoch | current job scheduler/edit-first desired-state worker |
| `sync/native/receive.rs` | shared bootstrap, delta inbox, graph/conflict planning and applied readiness | pure planner concepts, actor transactions; no file publication mid-page |
| `sync/native/upload.rs` | DB-derived tasks, stable original/thumbnail snapshots, resource sessions, atomic record requests and unknown outcomes | local snapshot machinery and existing issued-operation evidence |
| `sync/native/publish.rs` | verified dual downloads, cache import, owned file intent/recovery and admission | local publication, scanner/watcher, ThumbnailLoader and existing media model |
| `sync/model.rs`, `sync/store.rs`, `db_actor.rs`, `schema.sql` | typed provider discriminator, schema 9, native dirty/version/baseline/progress commands | no second dirty queue, no WebDAV-era columns left behind |
| `platform/credentials.rs` | distinct Bearer credential reference and lifecycle | current Secret Service boundary; never persist plaintext key |
| Settings + event/UI adapters | create/probe native connection, configured zone, supported strategy/scope, localized errors and truthful status | accepted changes only; existing local grid/viewer and glass/i18n/accessibility rules |

The former file-only `SyncProvider` trait was removed with the WebDAV engine.
The native boundary is its own typed protocol (`sync/native/`): zone feeds,
record mutations, delta pages and upload leases are methods with their own
shapes, never squeezed through a file metaphor. `NativeProvider` deliberately
implements no file operations.

## Stages And Exit Gates

| Stage | Concrete patch scope | Required focused evidence | Exit condition |
|---|---|---|---|
| C1.1 Connection/transport | provider discriminator, native DTOs/probe/auth/limit handling, native job configuration, schema 9 (WebDAV tables dropped and recreated, library preserved) | real server health/capability/scope/auth/errors (tests/native_sync_probe.rs against the actual binary); invalid drafts have no side effects; schema-8 upgrade keeps media/favorites/albums and discards sync history | native connection can be configured/probed; no claim of media sync yet |
| C1.2 Durable receive | snapshot/type markers, ordered delta inbox/projection, typed remote graph, scope-aware obligations and progress | `tests/native_sync_receive.rs` against the real binary: concurrent writes at a fixed height, a write between pages, replay idempotence, an empty zone, token-substitution refusal; store suites cover split atomic pairs, mid-page restart, scope/generation invalidation and token-expiry reset | metadata is durable; ingest can advance; applied remains blocked by unresolved work |
| C1.3 Dirty/coordination/uplink | real local mutation hooks, category versions/events, server lease/heartbeat/gate/phases, issued-operation reconciliation and callbacks | `tests/native_sync_lease.rs`: two clients, one acquire, busy skip, a forged fence refused, a refused gate keeping its lease, a passed gate being single-use, held-lock catch-up, renewal after loss, a receipt readable after the lease ended, a replayed mutation returning one receipt; store suites cover a mid-flight edit racing its callback and a confirmed create that must not repeat | generic coordinator and metadata-only native mutation work; no fake asset publication |
| C1.4 Complete media | stable source/thumbnail upload, trusted receipts, atomic photo pair, dual download/publication/cache/admission | `tests/native_sync_assets.rs` against the real binary: a pair uploads and downloads back byte for byte, tampered bytes fail the session terminally, a swapped role/field and a wrong-phase reservation are refused; store/service suites cover half-pair non-admission, exactly-once admission, stale intents, vanished staging and replaced files | new/updated media appears only when its complete validated pair is usable locally |
| C1.5 Real UI/recovery | settings/scope/status/explicit native cloud-delete intent/local media events, read leases, startup ordering and complete protocol journeys | real input/middle painted frames; favorites/edit/trash/recreation; viewport navigation causes no image network; interrupted config/global Off | complete native user journey end to end |
| C1.6 Integration delivery | final mixed-provider tests, maintainer docs, clean dev commit and push | exact full push gate and exact-SHA CI; real two-device HTTP/DB/file journeys | client acceptance recorded; production deployment is separate evidence |

C1.3 validates metadata coordination with native records/fixtures. Resource-
requiring photo creation/content mutation stays unavailable until C1.4; stage
milestones never set unsupported client features true just because server bits
are true. Bootstrap/resource schema can be introduced incrementally with coherent
migration tests; no fixed version-number reservation conflicts with intervening
storage work.

## Mandatory Acceptance Matrix

| ID | Real behavior to prove |
|---|---|
| NC01 | schema-8 upgrade preserves existing media/favorites/albums while discarding the removed WebDAV sync state; a fresh schema-9 library is native-shaped end to end |
| NC02 | different photo types share S/H; independent markers and delta catch-up do not omit/mix versions |
| NC03 | inbox/work/ingest commit atomically; split-pair pages and a crash never produce visible half media |
| NC04 | applied requires full projection + current configured-scope resource/conflict readiness; no token-only success |
| NC05 | two devices compete: one holder; loser skips once, retains dirty, keeps successful downstream |
| NC06 | cloud changes between normal downstream and acquire force held-lock catch-up; other-zone head changes do not loop |
| NC07 | body transfer across multiple TTLs succeeds with independent heartbeat; URL expiry is not upload resumption |
| NC08 | lease loss/deadline aborts every unfinished item, including queued work; old/late responses cannot clear dirty |
| NC09 | metadata-only edit uploads fields only; content edit derives both roles from one stable original; combined dirtiness is one photo update |
| NC10 | delete→modify→create ordering, atomic pairs/path swaps, conditional conflicts and retired-safe recreation |
| NC11 | new local/category edits during upload/callback remain dirty; confirmed create registers identity once |
| NC12 | response loss/Pending/Committed children reconcile through read-only status, with no duplicate create or premature dirty clear |
| NC13 | baseline/event conflict resolution merges independent changes; ambiguous/losing content remains recoverable |
| NC14 | actual original/thumbnail checksum/size/generation, signed URL refresh and Range/If-Range handling are validated |
| NC15 | either download/cache/publication failure keeps old complete media and durable work; restart settles each owned window |
| NC16 | scanner/watcher/direct upsert and internal file events cannot bypass admission or manufacture dirty |
| NC17 | small/medium/large local cache identity/orientation/color work; real grid/viewer navigation stays on local files |
| NC18 | accepted effective job edits coalesce/retrigger latest; drafts/no-ops/invalid inputs do not pause/restart work |
| NC19 | selected scope, local-only deletion suppression, historical eligible counts and explicit global Off remain correct |
| NC20 | steady no-change rounds use metadata/known digests; no remote body hashing GET or whole-local-library hash loop |

Use temporary directories, synthetic media and real HTTP with the Rust server,
not terminal DB rows manufactured to make a test pass. A native protocol mock
can test transport errors, but cannot replace actual actor/transaction/file/
server journeys or prove the real client's acceptance from server receiver demos.

## Implementation And Push Discipline

During each implementation patch run only added/directly modified focused tests.
For UX install Sandbox before gtk::init, drive hit-test then press through
`tests/common/interaction.rs` + `tests/common/shell.rs`, and use a desktop-sized
1920×1080 display. Never set final selection/visibility/DB state to simulate input.
Read relevant browsing/viewer/storage/editor/albums-trash guides as those paths
are changed. New user-visible copy belongs in i18n; update the maintained widget
naming map only when actual controls change.

PhotoViewer development/push stays on dev; SyncServer stays on main. Before each
remote push run the exact four commands in `.github/workflows/ci.yml` on the final
tree: fmt, strict clippy with the existing two allowances, locked all-target
build, and `tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --locked --all`.
Every commit has an honest Tests section. Report local result, pushed SHA,
exact-SHA remote CI and actual deployment independently.
