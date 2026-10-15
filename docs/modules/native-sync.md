# Native SyncServer Client Contract

## Scope And Status

This module owns PhotoViewer's C1 client for the independent Rust
`Luyao-1024/syncserver` service. Verified 2026-10-14 against server main
`2d9f9a7`: M3 token/snapshot/delta/lease coordination and M4 resource publication
are available, with M5 CI/deployment tooling. Server development is no longer
blocking C1.

**C1.1 is implemented**: typed protocol DTOs, endpoint normalization, the
`/readyz` + `/v1/capabilities` probe, capability/scope/origin validation, a
pinned capability scope on the connection row, explicit zone mapping on the job,
provider-kind dispatch in storage, and the native receive/dirty/publication
schema (schema 9). The WebDAV engine that previously shared this module has been
removed outright.

C1.2 adds the **durable receive path**: snapshot bootstrap with per-type
markers, an ordered replayable delta inbox, a validated typed remote graph, and
promotion that materializes resource obligations. `applied` deliberately stays
behind `target` while any of them is outstanding.

C1.3 adds the **uplink coordinator**: real local mutation hooks writing dirty
truth in the same transaction as the user's edit, per-category versions and edit
events, the exclusive upload lease with a conservative monotonic deadline, and
record mutations carrying idempotency keys whose receipts outlive the request.

Not implemented yet: **resource** synchronization — no asset upload, no
download, no publication, and no settings surface. Metadata-only record
mutations work; a create or a content change needs an asset pair and is
reported as outstanding rather than approximated. Native jobs exist as
configuration but are not yet scheduled onto a timer. The server's pre-release compatibility
waiver applies to that service, not the PhotoViewer user database.

| Read | Owns |
|---|---|
| [Protocol mapping](native-sync-protocol.md) | actual endpoints, DTO/capability names, scope/auth, limits, events, resources and errors |
| [Storage/publication](native-sync-storage.md) | actor/migration, existing-row dirty/version facts, receive progress, callbacks and full-media admission |
| [Implementation/acceptance](native-sync-plan.md) | module ownership, C1.1–C1.6 patch order, scope decisions and NC01–NC20 evidence |

Canonical server paths are `docs/design/v0.1-server.md`,
`docs/contracts/{upload-coordination,snapshots,photo-records,assets,records}.md`
and `docs/design/next-development-tasks.md` in its repository. When historical
M2 text differs, use the pinned implementation and current capability bits.
These are CloudKit-style subsets plus native extensions, not Apple login/private
SDK compatibility. The edit-first configuration contract will be restated
against the C1.5 native settings surface; the durable-edit engine itself is
described in [the module guide](sync.md).

C1 starts with explicit Native Sync jobs; Native Backup/history browsing is a
separate future contract.
Native metadata mapping initially covers favorite, relative path and capture date;
app-only album covers/order/editor history have no implied protocol. Supported
image/video originals use one stable original+thumbnail combination and existing
local extraction/loading, within actual server limits. No provider is silently
changed and no existing local library is automatically enrolled.

## Cycle And Exclusive Uplink

```text
Finish ordinary downstream reception and conflict coordination
  → Acquire one namespace upload lease once
      → acquisition failure: skip this cycle's entire uplink, retain dirty
      → acquired: start independent heartbeat and check current progress
          → behind: keep lease, repeat downstream and coordination, recheck
          → current + locally ready: freeze the DB-derived task view
              → delete → modify (metadata / content) → create
              → valid success callbacks settle their corresponding DB fields
              → release lease
```

A namespace has only one effective uplink device. Multiple devices may receive
metadata and download resources concurrently without that lease. The lease
covers all server mutations, including original/thumbnail upload sessions and
final record binding; one server SQLite writer is not itself a device lock.

The server's default TTL is 90 seconds, suggested heartbeat interval 30 seconds;
use actual grant/renewal values.
Heartbeat runs independently from file I/O, hashing, decoding and ordinary
request queues. The client uses a monotonic conservative deadline based on a
successful renewal request and known TTL, not its wall clock or receipt time of
an arbitrarily late renewal response. Renewal errors do not extend authority.

Acquisition failure, including busy or an unconfirmed network result, skips the
uplink once; do not spin/retry acquisition in the same cycle or mark the whole
library synced. Preserve the completed downstream result and all pending dirty
obligations. A later scheduled/manual cycle starts normally.

## Latest Token Gate

The server validates the local applied zone tokens against current cloud heads
while the lease is held: correct namespace, zone, kind, database generation,
complete pagination and committed watermark. Do not compare opaque strings or
substitute database discovery tokens, snapshot tokens or query markers.

`ingestToken` means pages and pending work have been durably received.
`appliedToken` means a complete fixed-watermark projection, configured download
scope, paired publication and conflicts have been coordinated. Both are fields
in the existing synchronization database, not a separate dirty journal.

Only the applied token may authorize uplink. The client independently checks
its inbox/projection/download/conflict state before declaring localReady; the
server cannot attest that local disk publication completed. Unrelated-zone
writes or a different encoding with no relevant changes must not cause endless
empty downstream cycles. Respect selected upload/download scope throughout.

When behind, retain and renew the lease during repeated downstream, including
required resources. A resource failure ends preparation without starting
uplink. After the gate passes, own uploads change cloud heads without invalidating
the original cycle gate. Do not optimistically advance receive tokens from an
upload response; the next downstream receives those changes reliably.

## Dirty Is An Existing Database Record Field

C1 adds/uses dirty flags on the existing `sync_entries` records, with local
change versions, event times, remote identity/baseline and operation correlation.
Do not create a dirty file, a standalone dirty table, or a second persistent
dirty queue. The in-memory task view derives from current database fields.
Existing task/operation recovery and paired publication records have different
responsibilities and are not another dirty source of truth.

| Dirty flags | Uplink |
|---|---|
| CLEAN | No task |
| NEW | Create; may retain subsequent metadata/content changes |
| METADATA | Update changed record fields, reuse existing resources |
| CONTENT | Upload changed resources, prepare thumbnail from the same original version, atomically update the record pair/generation |
| METADATA + CONTENT | Combine in one photo update |
| DELETE | Delete the record pair, retain DB identity/version/deletion time until confirmation |

Real local changes set the field and advance its local version. Enqueueing,
lease acquisition, request transmission and finishing one resource do not clear
it. Preserve explicit cloud deletion intent: local-only removal, scope exit and
unreachable drives keep [existing suppression safety](sync.md), not an automatic
remote delete. A newly created item that has never been submitted can cancel its
local-only obligation; if submission is uncertain, query the known operation
before assuming cloud absence.

## Callbacks And Loss Of Lease

Success callbacks correlate job/config/cycle/operationId and the captured local
change version. In one DbActor transaction, record confirmed upload facts and
clear only the corresponding completed dirty obligation. A local edit during
upload remains dirty for the next cycle; local CRUD and edit-first UI remain
available. A confirmed create registers remote identity even when subsequent
edits remain, converting their obligation to update/delete rather than repeating
create.

HTTP 200 alone is not success: validate atomic results or each non-atomic item.
AlreadyCurrent is allowed only after a verified matching target state and local
version, through the same successful confirmation path; do not use token equality
or an empty remote write just to clear dirty. Ordinary downstream LocalReady
updates download baselines, not proof of uplink success.

Lease expiration/explicit loss makes the cycle irreversibly Aborted. Cancel the
current body and attached preparation, stop dispatch, and fail every unfinished
uplink item with UPLOAD_LEASE_LOST. Do not acquire again inside this aborted cycle.
Previously confirmed successful items remain successful; late HTTP 200 does not
clear any field for this cycle. Independently running downstream is unaffected.

A request may have committed before loss while its response was lost. Keep dirty
and report failure now; a new cycle reads server operation/session status and
reconciles that fact before a valid confirmation. Never promise that aborting a
client reverted committed server changes.

## Conflict And Display Contract

Use the existing baseline to distinguish remote change from merely finding a
local dirty row. If both sides changed the same content/field, prefer the newer
real edit event, separately mapping `_syncserver.metadataEvent`, contentEvent and
deletionEvent (editedAtMs/source/eventId), with a stable tie rule. Unknown legacy
events and server commit-time fallback must not fabricate a newer user edit;
retain explicit conflicts/copies when comparison is ambiguous. File mtime/photo
capture date cannot describe all three. Merge
independent metadata/content changes and preserve losing content in a conflict
copy. Different photo IDs at the same path are a path conflict.

Downstream conflict resolution checks the current local version again before
publication so an edit made while downloading is not overwritten. Internal
sync writes must not set fresh user dirty through scanner/watcher events.

Download both original and thumbnail, verify the matching generation/digests,
prepare existing cache keys/sizes, and publish before gallery admission. Each
photo becomes visible separately once complete. Existing photos keep the last
complete version during failure. Grid/viewer continue their local URI/thumbnail/
preview/original pipelines; no remote placeholders or browse-triggered original
fetch. The planned publication ledger/read leases/old snapshot are distinct
from the server's exclusive upload lease and from dirty flags.

## Implementation Order And Verification

C1 can start now. Implement [C1.1–C1.6](native-sync-plan.md#stages-and-exit-gates):
connection/typed transport → durable receive → local dirty/lease/coordinator →
complete resource transfer/publication → real UI/recovery → integration delivery.
The detailed [wire contract](native-sync-protocol.md) and
[storage transactions](native-sync-storage.md) are prerequisites for code.

The file-operation provider trait is gone. The typed Native gateway in
`sync/native/` is the only transport boundary; do not fake a zone feed through
file operations, replace server leases with a local mutex, or use token string
equality for currency.
New original/thumbnail, scanner/cache and GTK acceptance must use real actor/
network/file/input paths. Server synthetic receivers do not establish PhotoViewer
acceptance. Each milestone distinguishes transport/metadata support from full
photo availability. Follow the exact dev push gate and report cloud CI for the
pushed SHA separately from local and production results.
