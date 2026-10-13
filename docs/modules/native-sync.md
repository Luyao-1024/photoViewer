# Native SyncServer Client Contract

## Scope And Status

This module owns the planned PhotoViewer client for the independent Rust
`Luyao-1024/syncserver` service. It is a maintained integration contract, not an
implementation report. No native Provider, upload lease, token gate, or database
dirty field has been added to PhotoViewer in this change. The server currently
implements M1/M2 metadata foundations; M3/M4 and this client's C1 remain pending.

The canonical server documents are `docs/contracts/upload-coordination.md`,
`docs/contracts/snapshots.md`, `docs/contracts/photo-records.md`,
`docs/contracts/assets.md`, and `docs/design/next-development-tasks.md` in that
repository. Its CloudKit-style endpoints are a documented subset plus native
extensions, not Apple authentication. Existing [WebDAV behavior](sync.md) and
[edit-first configuration](sync-configuration-ux.md) remain the current runtime.

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

The server's planned default TTL is 90 seconds, heartbeat interval 30 seconds.
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
edit event, with distinct metadataModifiedAt/contentModifiedAt/deletedAt and a
stable tie rule. File mtime/photo capture date cannot describe all three. Merge
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

1. Wait for the server's M3 token/real delta and lease/gate/phase capabilities,
   then its M4 resource publication. Do not emulate missing capabilities with a
   local mutex or opaque-token string equality.
2. Add a native Provider and transactional inbox/token fields behind SyncStore,
   preserving the core/GTK separation, selected scope and configuration epochs.
3. Migrate existing sync_entries with dirty/version/event/identity fields;
   update real local mutation paths, suppression and recovery without changing
   WebDAV's current semantics to claim native support.
4. Add the once-per-cycle lease coordinator/heartbeat, held-lock catch-up,
   delete/modify/create task derivation and version-checked callbacks.
5. Add original/thumbnail publication, cache import and scanner/watcher admission
   plus local read leases. Existing media viewing continues to use complete files.
6. Test two devices, skipped acquisition, several TTLs of transfer, loss at each
   commit window, late callbacks, newer local edits, partial batch success and
   internal-event suppression against real DB/network/file paths. Drive GTK
   through real [testing input helpers](../testing.md), not terminal widget state.

Implement only after entering C1; synthetic server client tests are not this
client's acceptance evidence. At the remote-push gate run the exact commands
from `.github/workflows/ci.yml` and report local versus exact-SHA cloud results.
