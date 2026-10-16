# Native SyncServer Storage And Publication Design

当前未正式发布，数据库版本始终固定为 **1**。`schema.sql` 是当前结构的唯一来源；结构变化时重新创建开发/测试数据库，不维护开发版本间的升级兼容。此文只定义 PhotoViewer 本地数据和发布方案；服务端规范统一引用[服务端契约索引](native-sync-protocol.md)。

## Ownership And Initialization

All writes go through the existing DbActor. Native protocol/network code owns
no SQL, GTK widgets or direct media-row admission. Extend SyncStore/DbCommand
with typed native operations; background scans, editor/favorite changes, trash,
recovery and callbacks use the same transaction owner.

Initialization executes `schema.sql` and writes `PRAGMA user_version=1` in one
transaction. Previous development version numbers are not an admission gate;
initialization does not migrate columns or discard existing data. Provider kind
is native-only, and a connection is created by an explicit completed probe.

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

The native schema uses these types/defaults; additional physical
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
adopted. A different record ID/path owner is not eligible for same-record timestamp
resolution; the receive path preserves differing local bytes under a free numeric
suffix. Preserve
metadata that has no trustworthy comparison event instead of assigning migration
time and declaring it newer.

### Identity adoption

`native_photo_id IS NULL` is not a disagreement. It means this entry has never
been published — or a publication whose confirmation was lost, which leaves the
entry looking identical. Both are settled by comparing the bytes, and the
comparison is the whole proof: the service keys `relativePath` uniquely per
zone, so a same-path cloud photo *is* this photo, the identity match is a claim,
and only a digest match settles it. `runtime::adoption` is the only place that
decides it, and it fires only on `size` + SHA-256 equality between the local
original and the cloud `original` descriptor.

Adoption is **entirely local** and sends nothing. The service has no adopt or
import operation, and does not need one: nothing here changes what the cloud
holds, it only stops this client from proposing to recreate what is already
there. It therefore bypasses the "save a recovery copy first" guard that
`PhotoAction::UseCloud` enforces — that guard exists to prevent losing local
bytes, and here the digest has proven the local bytes *are* the cloud bytes.

Three states are not adoption, and none of them is a content conflict:

* **An issued or publishing operation owns the entry.** Binding an identity
  underneath it would turn its recovery into a create the service can only
  answer with 409. Nothing is written and no conflict is raised; the recovery
  path settles it.
* **The entry has no live local file.** There is nothing to compare, and a
  conflict row would block the very download that would give the photo a local
  representation. The resource and admission paths own that outcome.
* **The photo is hidden, or a deletion is pending.** A hidden cloud photo has
  no local representation to bind to, and adoption would silently cancel an
  explicit delete intent.

Digests are measured in `SyncService::measure_local_matches`, before the promotion
transaction opens — a transaction that hashed a library would hold the single
write lock for the length of a disk sweep. `native_local_digest` caches the
result so an unchanged file is read once rather than once per cycle, and
`mark()` clears it whenever a local edit is recorded. Every measured or cached
result, including its persisted cache, also carries `sync_entries.id` and the
captured `native_local_version`.
The promotion transaction verifies both before consuming or caching the digest.
If an edit or entry replacement happened after measurement, adoption defers;
the new edit's dirty bits and acknowledged versions remain untouched. Cache
entries whose captured identity/version differs are discarded; successful file
publication also clears the old local digest.

### Local SHA-256 boundaries and name collisions

Ordinary local indexing, browsing and editing do not compute or persist SHA-256.
There is no SHA-256 field on `media_items`. Upload preparation hashes a stable
snapshot of a selected new/modified local original, and builds the thumbnail
from that same snapshot.

Receive comparison is enabled only for configured cloud synchronization. It
indexes existing local files at incoming paths even outside the upload scope,
without enrolling other local files or hashing them. Matching record IDs use
the timestamp rule directly. For a different/unbound identity at the same
relative album and filename, compare size first; only equal-size files are
hashed. Equal size and SHA-256 merge into the incoming cloud identity when the
previous identity is no longer live. Distinct live record IDs keep their separate
placements. Different bytes (or size) preserve the local file as `stem_1.ext`,
`stem_2.ext`, and so on, using the first name unused by disk, local DB rows and
this cloud inventory. The cloud photo retains its original filename. This does
not grant upload permission to an unselected album.

`service/receive/collisions.rs` runs file/metadata work off the runtime/UI thread.
Before moving a collision it journals the captured entry/version and both paths
in `sync_publications`. A no-overwrite hard link plus unlink preserves bytes;
the actor then commits media/sync paths without inventing a user edit timestamp.
A failed commit or an interrupted move restores the old name through ordinary
publication recovery. Successful preservation clears only the obsolete path
collision and leaves the renamed local version's upload eligibility intact.

A local merge settles only that photo's local placement. When asset IDs are
shared with another pending photo, that photo still needs staging bytes before
publication; library-backed verification alone cannot satisfy it.

### What adoption settles about resources

Adoption also has to settle the photo's `sync_native_resources` rows, and this
is the part that is easy to get wrong. `state='verified'` means two different
things depending on how the bytes got here, and the rest of the system reads it
as one:

* **Downloaded** — the bytes were fetched and checked into staging, so
  `staging_path` names a real file and `verified_cache` names the local copy.
* **Adopted** — the library file *is* those bytes. Nothing was staged and
  nothing needs to be.

So a settled-by-adoption row points at the library file through
`verified_cache`, and the `ON CONFLICT` refresh that upgrades an existing row's
state is **guarded on the entry owing nothing upward** — no dirty bits, no
pending publication. That guard is the whole safety property. A dirty entry's
bytes are about to be uploaded, which is exactly what staging is for; settling
its resources would hand the publish stage a verified row and no file to read,
and it would fail on the missing `staging_path`. A clean entry has no
publication, so it can never reach that code.

The reason this matters beyond tidiness: an outstanding resource obligation is
what holds `applied` behind `target`. Leave them open and the job reports
itself as synchronizing forever, with a spinner and a full counter — which looks
like a hang and is not one.

### Reading `native_dirty`

`native_dirty` is a **bitmask** — `NEW=1, METADATA=2, CONTENT=4, DELETE=8` —
not an enumeration. The categories are tracked independently, each with its own
version counter and its own acknowledgement, because they upload differently
and are cleared by different evidence. Two bits set at once is the normal
state, not an anomaly: it is exactly what an edit made while an operation was
in flight produces, and it is how the residual obligation survives the callback
that could not prove it was accepted.

**Never compare it for equality against a combined value.** `dirty == 2` and
`dirty == 4` read as "only metadata" and "only content" and are wrong for every
combination; ask per category with `NativeDirty::contains` instead. This is
what `Confirm` already does, and what promotion must do too — the two read the
same column and must read it the same way.

A new entry absorbs metadata, and the reason is on the wire: a create
unconditionally carries metadata (`create ||` in the body builder), so a
metadata bit set alongside `NEW` changes nothing about what is sent. It is still
recorded, because a *later* metadata edit — one made after the create's
snapshot was taken — is what the version comparison detects, and the mismatch
is what forces the follow-up update. Absorbing the bit would lose it.

After adoption, `native_dirty` keeps its `METADATA` bit only when the user had
actually edited metadata locally. A set bit means the local value stands and
the next uplink publishes it; a clear bit means there is nothing local to lose,
the cloud is authoritative, and the value is written to the library row. The
same bit decides both, so the two cannot disagree.

The uplink queue is an in-memory view of these rows. Native code must not persist
one sync_tasks row for every future dirty item or replay legacy pending actions
as dirty truth. Existing sync_tasks is reused only for issued-operation recovery:
write the intent immediately before an uncertain network mutation, including
its exact captured input/version and stable operationId. Recover that intent
before deriving a new operation for the same photo. Upload snapshots and their
ownership records provide file recovery, not another dirty queue.

## Same-record Concurrent Edits

Timestamp resolution requires the existing entry's `native_photo_id` to equal
the incoming photo record ID. A shared path alone never enables this rule.
Missing identity still goes through the separate size/SHA-256 adoption proof.

Metadata and content are decided independently against their confirmed
baselines. A changed remote category and a pending local category compare real
`editedAtMs` edit events; file mtime, capture date and upload/receive time are
not substitutes. Reuse `native::records::latest_event`: compare edit time first,
then `(source, eventId)` for deterministic ties. A known edit event beats a
missing one. If both events are missing, preserve an explicit pending local
edit rather than inventing a new timestamp or stopping synchronization.

A newer local category remains dirty and uploads with the latest observed
server revisions. Its losing remote content does not create download or
publication obligations, and is never falsely labelled locally verified.
A newer remote category clears only that local category and acknowledges its
current captured version. Remote content uses the existing pair publication,
with any independently newer local metadata preserved in the capture. A later
local edit invalidates that publication through its version/dirty guard. A
remote path change can publish after the winning local content has uploaded;
library-backed resources without staging bytes are restaged before placement.
Ordinary same-record edits do not create manual-resolution conflicts.

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

Startup order is schema initialization → load owned publication/admission evidence → settle
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
or deleted native jobs do not restart from old triggers. Recovery preserves the current native jobs and their issued-operation evidence.


## C1.5 客户端决策与快照恢复

首次清单先通过 DbActor 持久保存服务端返回的实际 snapshotToken 和对应 zone syncToken；所有记录类型复用该快照。页标记只推进自己的类型，不能把 zone token 放到 snapshot_token。墓碑即使 recordType 为 null 仍进入持久 inbox。只有完整固定水位图才可判定既有远端身份已删除。

native_retired 保存在既有 sync_entries。干净的云端删除保留本地文件并使身份退役；有竞争本地修改则建立冲突。明确重新上传才清退役标识，并分配新 ID。相册范围变化、离线根和本地删除均不产生云端删除事件。

用户选择版本时核对本地版本、冲突快照及未确认操作。UseCloud 先保留本地原图，再创建原有双资源发布意图；KeepLocal 先下载并保留竞争云端资源，已校验但未作为图库版本发布的资源以 retained 区分于 verified。该状态只结算明确用户选择覆盖的接收义务，不能用于普通媒体入库。确认的原子拒绝只结束已发出操作证据，不清 dirty；网络超时和未知回执继续保留。

新范围的本地索引通过已有 scanner 的 mtime/size 短路和 DbActor 批次完成；未变化的本地原图不参与每 15 秒检查的正文哈希。每个资源后检查可以入库的照片，后台发布任务完成前不释放运行屏障。下载期间接受配置编辑可以停止旧网络传输；文件发布仍等待完成或回滚。


## 账户身份与重新认证

用户信息是服务器 pinned scope 的一部分，在既有 JSON 中保存可选 authentication 字段，数据库版本仍固定为 1。重登通过 actor 命令核对旧连接整体捕获、账户 ID、namespace、generation 和 endpoint 后更新认证信息，不清 dirty 或接收位置。旧无用户身份仅可升级至同 namespace/generation 的显式数据接管者。相同 endpoint/zone 的唯一映射检查增加既有 native_tenant，其他用户可用不同根建立任务，物理路径重叠仍拒绝。细节见[账户接入方案](native-sync-accounts.md)。
