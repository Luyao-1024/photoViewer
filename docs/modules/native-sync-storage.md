# Native SyncServer Storage And Publication Design

Status: C1.1 delivered, 2026-10-14. Schema 9 is the native-only shape: the
WebDAV engine and its columns were removed outright, and a schema-8 library is
upgraded by dropping the sync tables and recreating them empty — media, albums,
favorites, edits and settings are untouched, WebDAV sync history is not carried
forward. There is no in-place column migration and no compatibility waiver for
the old sync rows. Follow [the main contract](native-sync.md),
[wire mapping](native-sync-protocol.md) and [storage rules](storage.md).

## Ownership And Migration

All writes go through the existing DbActor. Native protocol/network code owns
no SQL, GTK widgets or direct media-row admission. Extend SyncStore/DbCommand
with typed native operations; background scans, editor/favorite changes, trash,
recovery and callbacks use the same transaction owner.

Schema 9 exists to preserve the *library*, not the WebDAV state: media IDs,
favorites, album order, covers and edits survive the upgrade; sync connections,
jobs, entries, tasks and conflicts are discarded because they describe a
transport this build no longer speaks. `provider_kind` is native-only (a CHECK,
not a default). A native connection is created explicitly by a completed probe;
nothing enrolls local media automatically.

## Planned Fields On Existing Rows

| Owner | Added native data / invariant |
|---|---|
| sync_connections | provider_kind=native_syncserver (CHECK-enforced), credential_ref for Bearer, pinned capability document; no plaintext key |
| sync_jobs | explicit NOT NULL zone/owner and scope metadata; config_generation still gates desired state; no path-shaped remote root exists |
| sync_entries | native_dirty bitmask and native_local_version; independent metadata/content/deletion versions and edit events |
| sync_entries | stable photo/master IDs, both confirmed record revisions, content generation and typed field baselines |
| sync_entries | native original/thumbnail IDs, SHA-256/size baselines, acknowledged category versions and desired metadata snapshot |
| sync_tasks | issued native operation evidence: captured job/config/cycle/entry/category versions, namespace generation, exact logical body, resource-session correlation |
| sync_conflicts | category, both event stamps/revisions and losing-copy ownership; no second dirty truth |

Store category stamps in nullable validated JSON columns (editedAtMs/source/eventId),
with names such as native_metadata_event/native_content_event/native_deletion_event.
Wire `_syncserver` is assembled from them; absent old timestamps remain absent.
Use separate nullable native_* columns rather than treating existing BLAKE3
fingerprints or weak WebDAV revisions as SHA-256/recordChangeTag. Current
entry_generation advances on sync observation as well as user changes, so it
cannot replace the independent native_local_version/category counters.

Keep sync_entries.id as local identity through media-index rebuilds; nullable
media_id is only a presentation association. Native photo/master IDs are stable
UUIDs, not media IDs or relative paths. Add a scoped uniqueness constraint for
native photo identity. Update paths in place; do not recreate the entry to rename
it. For a valid path swap, change affected row paths through unoccupied internal
placeholder values inside one DbActor transaction, then final paths; never emit
those temporary paths, write them to disk, or commit a placeholder. A conflict
with an unrelated occupant remains a conflict.

## Column Shapes And Index Keys

The first native migration uses these types/defaults; additional physical
columns may split JSON only while preserving the same transaction contract.

| Planned columns | Shape / meaning |
|---|---|
| native_dirty | INTEGER NOT NULL DEFAULT 0, CHECK 0–15; bit semantics below |
| native_local_version | INTEGER NOT NULL DEFAULT 0, monotonic user-change counter |
| native_metadata_version / native_content_version / native_deletion_version | INTEGER NOT NULL DEFAULT 0; independently captured/acknowledged |
| native_acked_metadata_version / native_acked_content_version / native_acked_deletion_version | INTEGER NOT NULL DEFAULT 0; never above their corresponding local version |
| native_metadata_event / native_content_event / native_deletion_event | nullable validated JSON event stamps; unknown is NULL |
| native_photo_id / native_master_id | nullable UUID TEXT, assigned together; provisional NEW IDs are distinct from confirmed cloud existence |
| native_asset_revision / native_master_revision / native_confirmed_generation | nullable opaque TEXT; acknowledged pair/scope facts |
| native_content_generation | nullable UUID TEXT for the confirmed combination |
| native_desired_metadata / native_baseline_metadata | nullable validated typed-field JSON; desired user values and exact confirmed baseline have different roles |
| native_resource_baseline | nullable validated JSON for both roles' IDs/SHA-256/sizes; never store a lease token or expiring URL as identity |
| sync_tasks.native_capture / sync_conflicts.native_detail | nullable validated JSON for issued-operation captures or conflict candidates; preserve legacy columns/rows |

Allocated NEW record IDs do not prove a create happened. Once submitted, retain
those IDs and the stable operation intent until outcome reconciliation. Before
any submission a verified existing cloud pair may replace provisional IDs via
explicit adoption; after a confirmed deletion recreation always gets fresh IDs.

Progress has PRIMARY KEY(job_id, zone_name, owner); namespace, generation and
scope digest (including local placement/download scope) are explicit fields.
Inbox has an ordered receive ordinal and UNIQUE(job_id, generation, zone,
record_name, revision) for replay, never sorts by opaque token/revision strings.
Remote records use PRIMARY KEY(job_id, generation, zone, record_name); resource
work uses (job_id, generation, asset_id, role) and ownership-qualified staging.
Publication IDs and issued operation IDs are unique. New rows have job foreign
keys and explicit cleanup; preserve pending operation evidence while an entry
is deleted/unlinked. A task may reference a photo pair; child result correlation
is persisted rather than inferred from recordName ordering.

Index active native entries by (job_id, native_dirty, id) for derivation and
partial-unique (job_id, native_photo_id) when non-NULL. Index inbox by scope/ordinal,
resource work by job/state, and publication intents by job/state. All actor
commands check provider kind, scope/config and captured versions; SQL tracing
and EXPLAIN tests cover actual statements. File/network waits never occur in
these transactions, and no native triggers overwrite WebDAV fields.

## Dirty Truth And Mutation Transactions

| Bit | Meaning | Derived uplink phase |
|---|---|---|
| 0 / CLEAN | no unconfirmed local obligation | none |
| 1 / NEW | no confirmed remote photo identity | create, optionally with later META/CONTENT bits |
| 2 / METADATA | supported user metadata/path changed | modify; no asset upload |
| 4 / CONTENT | original bytes/resource combination changed | modify; prepare changed resource pair |
| 8 / DELETE | explicit cloud deletion intent | delete; retain entry/IDs/revisions/event |

Classification priority is DELETE, then NEW, then META/CONTENT, although global
execution order is delete→modify→create. META+CONTENT is one photo update. A
new item that was never submitted may cancel locally; uncertain creation first
requires operation reconciliation, not an assumption that it never existed.

One user mutation transaction updates the affected media/desired fields, dirty
bits, native_local_version and affected category versions/stamps. Favorite/path/
capture-date changes are metadata; editor/EXIF byte changes are content; entering
an explicit remote-delete workflow supplies deletionEvent before losing the
media association. Row disappearance, external drive loss, local-only trash,
leaving scope and unchecking an album never manufacture remote DELETE.
Preserve propagate_deletes=false by default; C1.5 must connect an explicitly
accepted native cloud-deletion policy/action to this intent. When authorized,
use the real user trash/delete path and retain its event time. A pending/failed
local filesystem mutation cannot dispatch a cloud delete: preserve existing
mark-before-GIO and rollback/recovery rules, then make the native obligation
eligible only after the accepted local operation is settled. Undo before any
submission may cancel the local delete obligation; uncertain submission must
be reconciled before deciding whether restoration needs fresh remote IDs.

Enrolling an eligible local item supplies NEW without inventing an edit time.
Bootstrap/import/cache updates do not count as user edits. Existing same-path
cloud content is not blindly recreated: verified matching pair identity may be
adopted through AlreadyCurrent confirmation; different IDs/content/path ownership
are explicit conflicts. Preserve metadata that has no trustworthy comparison
event instead of assigning migration time and declaring it newer.

The uplink queue is an in-memory view of these rows. Native code must not persist
one sync_tasks row for every future dirty item or replay legacy pending actions
as dirty truth. Existing sync_tasks is reused only for issued-operation recovery:
write the intent immediately before an uncertain network mutation, including
its exact captured input/version and stable operationId. Recover that intent
before deriving a new operation for the same photo. Upload snapshots and their
ownership records provide file recovery, not another dirty queue.

## Success Callback Transaction

Validate authenticated response/scope, current job/config/cycle, operationId,
confirmed atomic pair or non-atomic item, and captured versions. In one actor
transaction register confirmed remote identity/revisions/resource facts and
advance only the corresponding acknowledgement baselines. Clear a dirty bit
only if its category version still equals the captured version; a different
category/new edit remains dirty. An aborted cycle admits no late callback.

A confirmed create registers its remote IDs even when subsequent user edits
remain. Remove only the confirmed NEW obligation; residual work becomes update
or delete, not another create. A confirmed delete cannot revive newer local
content: later recreation uses fresh retired-safe remote IDs. Responses lost
before confirmation retain dirty and an issued-operation recovery fact. A new
cycle can confirm known committed facts only after checking current identity,
local versions and generation; it must not reuse a superseded logical request.

AlreadyCurrent uses this same typed callback after verified matching cloud/local
state. A download baseline, lease grant, token response, successful one-role
upload or HTTP 200 with item errors is not such confirmation. Conflict-selected
remote state may supersede a captured local obligation only through a validated
AlreadyCurrent/conflict completion; it cannot unconditionally zero dirty.

## Receive Tables And Atomic Progress

New tables belong to the same application DB, with job/namespace/zone/generation
scope and bounded indexes. They are not local-change truth:

| Planned table | Stored fact |
|---|---|
| sync_native_progress | snapshot + per-type bootstrap markers, separate database discovery/zone ingest/zone applied/target tokens, round state |
| sync_native_inbox | ordered version events and pending projection work, deduplicated by scoped record revision |
| sync_native_records | final-H remote projection keyed by recordName/type, not visible media_items |
| sync_native_resources | role-specific expected IDs/digests/sizes, owned staging progress and verified cache locations |
| sync_publications | owned pair/file/cache publication intent, captured local versions, old complete snapshot, recovery state |

Per-job/zone progress keeps configured download scope distinct from discovery.
One native job per zone avoids competing applied positions for the same local
projection. A scope/namespace change invalidates old claims and re-evaluates
unresolved in-scope work; old applied progress does not prove newly included
resources are complete. Full-zone metadata retention remains independent from
selected local upload albums. Do not erase unresolved work merely to make the
new scope appear complete.

Each page transaction validates current scope, saves inbox/projection work and
ingestToken together. A bootstrap persists one snapshotToken and independent
markers for every supported type; no deletions are inferred while incomplete.
On a final fixed-H page, replay ordered inbox into a provisional projection,
validate the complete pair graph, promote the remote projection and materialize
resource/conflict obligations in one transaction. Do not expose half an atomic
pair from an earlier page. Unknown types are retained and not silently counted
as complete supported media.

appliedToken reaches targetToken only when the final projection exists and all
actual configured-scope publication/conflict obligations are settled. Empty
inbox alone is insufficient. Download failure retains ingest and work; restart
resumes from those durable facts. Token expiry rebuilds metadata while retaining
local dirty, valid confirmed facts, verified assets and unknown operations for
reconciliation; it never performs mass deletion from a partial new listing.

## Stable Content And Incremental Hashing

Reuse current local hashing/snapshot machinery, with explicit native SHA-256
for each original/thumbnail and BLAKE3 for current local/media baselines. Compute
needed digests during one bounded streaming pass over a stable owned snapshot;
regenerate the upload thumbnail from that exact original. New user content
invalidates only that entry's cached snapshot/hash. mtime+length alone cannot
prove unchanged content, and server commit time is not a user edit time.

An app-offline/watcher-loss interval requires a bounded reconciliation audit of
the eligible local scope before declaring local change tracking reliable. It
may read local files; ordinary native rounds must not repeatedly hash the whole
library. The server never needs to re-hash existing assets for client polling.
Known matching verified asset IDs can be reused across generation/photo bindings.
Metadata-only dirtiness does not prepare or upload an original/thumbnail.

## Pair Publication And Existing Loader Reuse

```text
Remote metadata → durable resource work → download/verify both roles
  → build local original metadata + existing thumbnail cache variants
  → durable publication intent and old-version read protection
  → publish files/cache → recheck local versions → media admission transaction
  → complete in-scope obligations → appliedToken
```

Use current private sync-staging and safe job-root-relative mapping. Do not scan
in-progress originals into media_items. The admission check must exist in every
scanner/watcher/direct upsert path, including restart before normal scans.
Recheck local/content/config versions immediately before publication; a user
edit during download cannot be overwritten. Native-owned events update verified
baselines/admission, not user dirty flags. Genuine concurrent events still do.

Filesystem and SQLite are not one transaction: persist intent before destructive
placement, fsync files/directories, preserve the last complete original/cache
snapshot and reader leases, then commit final media identity/metadata/cache facts.
Failure/restart at any boundary either finishes the proven pair or restores the
last complete version. Never advertise LocalReady solely because one rename
or one resource finished. Suppressed local removals keep unchanged remote bytes
absent; metadata-only remote changes do not lift content-version suppression.

Use existing source path + final original mtime cache identity (`thumb-v4`),
ThumbnailSize Small/Medium/Large (256/512/1024) and current memory/in-flight keys.
Import/downscale the validated server thumbnail through the existing thumbnail
module; do not name cache files by remote URL or asset UUID alone. Missing native
cache-import APIs are added inside that module, with orientation/color/aspect
handling validated against the original and no repeated original decode merely
to create another cache size. Do not fake thumbnail_generated_at before cache
files are complete. Grid/viewer keep the existing local URI/thumbnail/preview/
original loading pipeline; no remote placeholders, thumbnail-only admission or
click-triggered network fetch. Each complete media item can appear independently
while other items are still downloading.

## Completion, Overview And Startup

Reuse the complete eligible sync_inventory for the denominator and retain the
historical completion fraction. Count one media item after its required complete
pair/confirmation, not two resources or a received metadata row. A job is complete
only with current applied/scope readiness, no eligible unconfirmed dirty, no
unresolved issued operation and current configuration; an empty runtime queue
or ineligible dirty entries alone cannot determine that state.

Startup order is migration → load owned publication/admission evidence → settle
or shield incomplete files and keep last complete versions → allow normal
scanner/watcher admission → reconcile native receive/issued operations and run
new cycles. Ordinary independent local media remains available; do not block
the entire library on a failed remote transfer or let startup scans import its
uncommitted files before recovery has established their ownership.

## Reset And Lifetime

Configuration browsing/drafts/no-ops leave active work untouched. An accepted
effective change revokes obsolete dispatch/callback claims, safely settles
issued facts, releases the relevant lease and automatically schedules the latest
surviving job, following the existing edit-first worker. Independent jobs keep
their state; a shared local lease broker is only admission optimization.

Global Off keeps configured jobs/credentials/roots/selected scope while discarding
owned runtime/history state under the existing explicit reset contract. It never
deletes ordinary media or remote history. Exit/restart, ordinary failure or scope
uncheck is not Global Off and preserves dirty/unknown-operation evidence. Failed
or deleted native jobs do not restart from old triggers. The entire native
migration/recovery path must remain safe with existing WebDAV jobs in the same DB.
