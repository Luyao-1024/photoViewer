# Native SyncServer Storage And Publication Design

当前未正式发布，数据库版本始终固定为 **1**。`schema.sql` 是当前结构的唯一来源；结构变化时检测列布局，自动恢复最多只重建一个不兼容的表，不维护开发版本间的升级兼容。此文只定义 PhotoViewer 本地数据和发布方案；服务端规范统一引用[服务端契约索引](native-sync-protocol.md)。

## Ownership And Initialization

Cloud-photo and sync state uses ordinary SQLite columns and normalized relations. No sync table persists JSON documents or opaque serialized BLOBs. JSON is used at the HTTP and in-memory DTO boundary. The [relational storage guide](native-sync-relational.md) owns the field/index rules and the 100-record, one-second batch target.

Development `user_version` remains 1. Before generic schema-drift recovery, a single transaction converts recognized legacy document columns, preserves row IDs, account selection, dirty/category versions, receive tokens, resource state, issued UUIDs and publication ownership, then removes the old columns. Issued request bodies and preparation must reconstruct exactly; unsupported legacy state aborts and rolls back the conversion. Conversion is not a reason to clear history or upload photos again. `PRAGMA foreign_key_check` validates the converted graph before commit.

| Owner | Stored state |
|---|---|
| `sync_accounts` / `sync_connections` | stable account ID; endpoint, namespace, generation, authenticated user, capabilities and limits in scalar columns; credential reference only |
| `sync_connection_zones` | accepted zones, ordered by position |
| `settings.native_sync_account` | scalar stable account ID; removing one connection does not change the selected account |
| `sync_entries` | photo/master IDs and revisions, dirty and local/acknowledged versions, retired/readiness flags, edit time/source/event ID columns and versioned SHA-256/size columns |
| `sync_records` | owned pending/baseline/conflict/publication/request record headers; indexed job/record/photo/master identity plus scalar relative path and generation |
| `sync_record_fields` | typed wire values: string/integer/real/boolean, reference identity, or asset ID/checksum/size/MIME/dimensions; no encoded value object |
| `sync_tasks` / `sync_task_resources` | issued-operation context, captured versions, body/preparation presence, ordered request records, owned snapshots, upload sessions and typed resource descriptions |
| `sync_native_progress` / `sync_bootstrap_types` | distinct snapshot/ingest/target/applied tokens; per-type status, continuation marker and count |
| `sync_conflicts` | reason, recovery path and an owned structured competing photo |
| `sync_publications` | captured configuration/local versions, structured target photo, file targets/new/backup paths and captured filesystem stamps |
| `sync_job_changes` / `sync_job_change_albums` | accepted edit revision, desired direction, deletion and explicit album draft |

`sync_entries.id` is the stable local identity. `media_id` is a nullable presentation association. Pending record rows represent only incomplete work and are deleted after application; baseline records replace the former baseline document on the same entry. They do not introduce a second full remote inventory. Historical issued/publication evidence has its own owner and survives ordinary media association changes.

## Fields And Indexes

Native dirty is exclusive: 0 clean, 1 new, 2 modified, 4 permanently deleted. Metadata/content/deletion retain independent local and acknowledged counters. Each event has nullable `edited_ms`, `source`, `event_id` columns; absence never becomes a fabricated time. Digest columns capture entry ID and local version as well as SHA-256 and size; a new edit invalidates the capture.

Use the partial unique `(job_id,native_photo_id)` index and `(job_id,native_master_id)` index for confirmed identity. Pending records have `(job_id,owner_kind,record_name)` and photo/master indexes. Read the pending revision first, then the confirmed scalar revision. Album/path joins use `(job_id,relative_path)` and `media_items(path)`. Per-page comparisons select only incoming paths, in bounded parameter chunks, rather than loading the whole task.

Resource work remains keyed by `(job,generation,asset,role)`. Publication lookups use `(job,photo,state)`; account connections use a stable account FK. Query-plan tests verify actual identity index use. File/network/hash work remains outside actor transactions. Wire omission versus explicit null and request record order are preserved for idempotent replay.

## Dirty Truth And Mutation Transactions

| State | Meaning | Uplink phase |
|---|---|---|
| 0 / CLEAN | no outstanding local change | none |
| 1 / NEW | not yet created remotely | create |
| 2 / MODIFIED | metadata and/or content changed | modify |
| 4 / DELETE | physical file permanently removed | delete |

States are exclusive. Metadata/content/deletion retain independent local and
acknowledged version counters and edit events. Version differences decide which
parts an update sends; new edits during a create become state 2 after the create
is confirmed. A pending deletion stays 4 until deletion is confirmed, regardless
of later remote metadata/content events. The sync row and cloud identities remain
in SQLite after its local gallery row is removed. Only successful synchronization,
an explicit account switch, or global Off may discard the deletion obligation.

One user mutation transaction updates the affected media/desired fields, dirty
state, native_local_version and affected category versions/stamps. Favorite/path/
capture-date changes are metadata; editor/EXIF byte changes are content; a verified permanent physical deletion supplies deletionEvent before losing the
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
  representation. Unless an issued/publishing operation or explicit deletion
  still owns the entry, promotion binds the remote identity and creates both
  resource work and a publication intent in the same transaction. Stale NEW or
  CONTENT bits cannot preserve a missing original. After the complete pair is
  verified, ordinary publication admits the photo without a new upload obligation.
* **The photo is hidden, or a deletion is pending.** A hidden cloud photo has
  no local representation to bind to, and adoption would silently cancel an
  explicit delete intent.

Merging identical bytes settles content identity, independently of metadata. A
pending local metadata edit survives as METADATA (`dirty=2`) and remains in the
uplink view against the adopted record ID. Otherwise, cloud favorite/capture date
is applied to the media ID found by path, even if the entry's old `media_id` is
NULL; that write and identity/baseline confirmation share one transaction. Cloud
metadata application is not a local edit and creates no upload obligation. A
subsequent local metadata edit follows the ordinary dirty/version/event path.

Digests are measured in `SyncService::measure_local_matches`, before the promotion
transaction opens — a transaction that hashed a library would hold the single
write lock for the length of a disk sweep. The typed entry digest/version columns cache the
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

`native_dirty` is one operation state: 0=clean, 1=new, 2=modified, 4=permanently
deleted. Metadata and content both use 2; their independent version and
acknowledgement counters select the actual payload and preserve concurrent edits.
A metadata-only edit consumes no asset allowance. Content replacement still sends
a complete original/thumbnail pair.

Moving to or restoring from trash is metadata, never dirty=4. A successful physical
permanent deletion records state 4 with its event/identity before removing the
local gallery row. The sync row survives process restart, failures and retries;
acknowledged cloud deletion removes it. A newer remote edit cannot clear it or
re-download the removed local file. Explicit account switching and global Off are
the other allowed cleanup boundaries. A never-published entry with no issued
operation may be confirmed already absent at a valid current-watermark upload gate.

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

## Per-Cycle Upload Failures

`sync_entries.native_upload_error` stores the individual photo's latest upload
failure. An accepted `Begin` clears the current job's errors in the same actor
transaction that admits its new cycle. A rejected start leaves errors intact.
`UploadFailed` validates the current epoch and entry/job identity before writing;
a stale callback cannot label a newer cycle as failed. The marker survives cycle
completion and restart until that job's next accepted cycle.

Receipt lookup/confirmation, pending-operation retries and new preparation or
submission all isolate photo-specific failures. Marked entries are excluded from
later dispatch within the same cycle while retaining dirty/category versions and
issued evidence. A definitive server rejection retires only the rejected issued
operation; its entry still waits until the next cycle before creating a fresh
request. Unknown outcomes keep the original operation identity. Lease loss,
superseded configuration, scope/database-generation mismatch and unresolved
issued operations from an older configuration still prevent unsafe new writes.
Other jobs' diagnostics and successful entries are unaffected. Batch allowances
still count attempted uploads; receipt-only checks do not consume them.

An original whose current size exceeds the pinned server asset limit is a
waiting condition, not an upload failure. It is rejected before an attachment
slot is consumed and before a source snapshot is made. Later cycles re-check
its metadata cheaply; a content edit or a changed server capability can make it
eligible again.

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

## Per-photo Receive And Atomic Progress

The persistent owners are the existing media/sync entries, job receive cursors,
role-specific resource work and owned file-publication intents. No table mirrors
the entire cloud inventory. A received Asset admits its user metadata immediately
as cloud=2; incomplete Master/Asset fields are temporarily attached to that entry.
At the end of a fixed-height delta round, only touched pairs are validated and
applied, then those temporary fields are cleared. Bootstrap pages can apply their
complete photos without waiting for the rest of the library. Missing entries never
infer deletion; explicit server tombstones are the sole receive deletion evidence.

Each ingest position follows the durable fields it covers. A bootstrap retains its
shared snapshot token and independent continuation markers. `applied` advances to
`target` after bootstrap and actual pending pair/resource/conflict/publication work
are settled. A failure-free upload already commits its canonical identity and
resource baseline, so it can adopt its head without copying records to another
inventory. Token expiry resets receive positions, preserving dirty=4, versions,
confirmed identity and unknown issued operations for reconciliation.

## Stable Content And Incremental Hashing

Reuse current local hashing/snapshot machinery, with explicit native SHA-256
for each original/thumbnail and BLAKE3 for current local/media baselines. Compute
needed original digests during one bounded streaming pass over a stable owned
snapshot. Read the upload thumbnail from the existing local cache, preferring
Large, then Medium, then Small, using the usual source path/mtime cache identity.
Do not require a source-content digest binding or regenerate a thumbnail for
uplink: temporary original/cache version differences are accepted. Missing local
thumbnails defer the entry without clearing dirty, recording an upload failure,
issuing an operation, or consuming an attachment slot. Local thumbnail workers
own generation; later automatic cycles retry. Reuse fitting JPEG/PNG bytes
unchanged; conversion/size reduction decodes only cached thumbnail pixels.
New user content invalidates only that entry's cached snapshot/hash.
mtime+length alone cannot prove unchanged original content, and server commit
time is not a user edit time.

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
in-progress original files as complete local originals. The readiness check must
exist in scanner/watcher/direct upsert paths; received metadata itself is already
admitted as cloud=2 before those files are available.
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
files are complete. Grid/viewer use the existing local URI/cache pipeline. Metadata-only rows stay
hidden until their thumbnail is ready. A cloud-only row then shows that cached
preview, and its original becomes available after publication changes location
2→3. No click-triggered network fetch or repeated whole-library comparison is
needed. Each photo can appear while other originals continue downloading.

## Completion, Overview And Startup

Derive the denominator from current-account sync entries, photo location/readiness,
explicit upload scope and pending deletions; retain the
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

首次清单先通过 DbActor 持久保存服务端返回的实际 snapshotToken 和对应 zone syncToken；所有记录类型复用该快照。页标记只推进自己的类型，不能把 zone token 放到 snapshot_token。墓碑即使 recordType 为 null 仍作为该照片的临时字段持久化；只有明确墓碑才处理远端删除，不以清单缺项作判断。

native_retired 保存在既有 sync_entries。干净的云端删除保留本地文件并使身份退役；有竞争本地修改则建立冲突。明确重新上传才清退役标识，并分配新 ID。相册范围变化、离线根和移入回收站不产生云端删除事件；确认本地文件彻底删除才记录 dirty=4。

用户选择版本时核对本地版本、冲突快照及未确认操作。UseCloud 先保留本地原图，再创建原有双资源发布意图；KeepLocal 先下载并保留竞争云端资源，已校验但未作为图库版本发布的资源以 retained 区分于 verified。该状态只结算明确用户选择覆盖的接收义务，不能用于普通媒体入库。确认的原子拒绝只结束已发出操作证据，不清 dirty；网络超时和未知回执继续保留。

新范围的本地索引通过已有 scanner 的 mtime/size 短路和 DbActor 批次完成；未变化的本地原图不参与回退调度的正文哈希。每个资源后检查可以入库的照片，后台发布任务完成前不释放运行屏障。下载期间接受配置编辑可以停止旧网络传输；文件发布仍等待完成或回滚。成功批次后仍有义务时，配置 worker 立即续跑下一批；只有无进展时才等待回退调度。


## 账户身份与重新认证

用户信息、认证模式、服务器能力和限制保存在连接普通列中；`sync_accounts` 提供稳定账号 ID，数据库版本仍固定为 1。重登通过 actor 命令核对旧连接整体捕获、账户 ID、namespace、generation 和 endpoint 后更新认证信息，不清 dirty 或接收位置。旧无用户身份仅可升级至同 namespace/generation 的显式数据接管者。相同 endpoint/zone 的唯一映射检查增加既有 native_tenant，其他账号保留独立任务且可复用本地根；同账号内部仍拒绝重叠。settings.native_sync_account 保存稳定账号 ID，sync_connections.enabled 只启用这个账号，切换通过 actor 事务使旧 epoch 失效，并等待旧运行退出后才允许新运行。细节见[账户接入方案](native-sync-accounts.md)。

A received rename can wait for a newer local content upload. Once that upload is
confirmed, its canonical baseline still names the accepted destination. The next
cycle resumes placement for that specific row even when no later delta arrives;
this uses local deferred work and does not enumerate or compare a cloud inventory.
Shared verified assets without staging must be fetched again if another photo
still needs publication; adopting one local file cannot settle another's placement.
