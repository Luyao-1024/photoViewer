# Native SyncServer Protocol Mapping

Status: frozen C1 client design, 2026-10-14; not implemented in PhotoViewer.
Verified server baseline: `Luyao-1024/syncserver` main commit `2d9f9a7`,
protocolVersion=1, stage=M4. The authority is that repository's
`docs/design/v0.1-server.md` and `docs/contracts/{snapshots,records,photo-records,assets,upload-coordination}.md`.
Read alongside [the client contract](native-sync.md), [storage](native-sync-storage.md)
and [implementation plan](native-sync-plan.md).

## Connection And Discovery

`native_syncserver` is the only provider kind (CHECK-enforced in schema 9; the
WebDAV engine was removed outright). Native authentication is one Bearer API key
stored by the [credential boundary](sync.md#credentials); username/password,
Apple ID, Cookie and the server's signing key are not client credentials.

Normalize an absolute root endpoint, reject userinfo/query/fragment/path prefixes,
and require HTTPS except for literal loopback IPs. C1 initially requires the
announced publicBaseURL to share the configured origin; alternate asset origins
need a separate future contract. Do not rewrite signed URL paths/parameters,
follow cross-origin credential redirects or send Bearer/lease headers to an
unrelated host. Local integration fixtures configure the actual listener port
as publicBaseURL rather than bypassing this check.

Probe `GET /readyz`, then authenticated `GET /v1/capabilities`. Pin namespace
`tenant/container/environment/database` and databaseGeneration. Use supported
protocolVersion, capability bits and numeric limits; a textual stage or version
number alone does not enable features. Required bits are zoneListing, recordCrud,
snapshots, incrementalChanges, uploadLeases, fencing, tokenCurrentGate,
uploadPhases, operationStatusRead, assetUpload, assetDownload and photoPublication.
queryBatch/changeAliases are optional optimizations. uploadResume/sharing/
appleAuthentication remain false. Missing requirements produce a localized
unsupported-provider error before an effective configuration is accepted.

CloudKit base is `/database/1/{container}/production/private`; container comes
from the pinned capability scope. One native job initially maps one local root
to one configured zone, owner `_defaultOwner`. Native remote_root is not a DAV
collection path: persist the zone explicitly. Reject two active native jobs
that map the same endpoint/namespace/zone into different local roots, and reject
ambiguous overlapping root ownership. No online zone creation/deletion is assumed.

## Endpoint Table

All control/metadata calls use Bearer and application/json. Responses may contain
errors inside an HTTP 200 batch; parse those before considering tokens or results.

| Operation | Request | Response / client rule |
|---|---|---|
| zones/list | POST CloudKit base + `/zones/list`, `{}` | `zones[].zoneID`; only configured zones |
| snapshot | POST `/v1/snapshots`, zoneID | snapshotToken + zone syncToken at one H |
| query | POST base + `/records/query` | records, syncToken, optional continuationMarker |
| lookup | POST base + `/records/lookup`, records | current or snapshot-bound historical items; per-item UNKNOWN_ITEM |
| database delta | POST base + `/changes/database` | zones, syncToken, moreComing; discovery only |
| zone delta | POST base + `/changes/zone`, zones array | per-zone records/tombstones/token/moreComing or error |
| acquire | POST `/v1/upload-leases` | leaseId/leaseToken/fencingToken/expiresAtMs/ttlSeconds/heartbeatSeconds/phase/zoneIDs |
| check | POST `/v1/upload-leases/{leaseId}/check` | isCurrent, mayUpload, cloud zoneTokens, phase |
| phase | POST `/v1/upload-leases/{leaseId}/phase` | phase; only delete→modify→create |
| renew/release | POST corresponding lease path, `{}` | ttlSeconds/expiresAtMs or released; renewal keeps lease identity |
| modify | POST base + `/records/modify` | atomic record results or ordered non-atomic item results |
| operation status | GET `/v1/operations/{operationId}` | Committed/Pending, result, items; no upload lease |
| reserve upload | POST base + `/assets/upload` | tokens[].url + session extension |
| upload bytes | POST signed `/v1/uploads/{sessionId}/content` | raw application/octet-stream, `singleFile` descriptor |
| session status/renew | GET `/v1/uploads/{sessionId}`; POST `/renew`, `{}` | durable state/asset/error; renew only Reserved with current lease |
| asset description | GET `/v1/assets/{assetId}?role=original\|thumbnail` | fresh downloadURL/receipt, stable identity |
| asset data | GET/HEAD `/v1/assets/{assetId}/content?role=...` | signed capability or same-origin Bearer, ETag/Range/conditions |

Use the canonical changes/zone path initially. records/changes and zones/changes
are aliases of the same zones-array shape; query batch is `{"batch":[query requests]}`.
No Hyperion/private count index or nonempty filterBy/sortBy is supported. Natural
query order is recordName ascending. Full photo replication initially omits
desiredKeys; a later projection must retain all graph/resource/event fields.
desiredRecordTypes is only a hint and never a reliable change-stream filter.

Limits: query/delta pages 1–200, zone arrays 1–20, upload reservations ≤200,
modify/lookup ≤400, record fields ≤1 MiB, wire result ≤8 MiB. Query desiredKeys
has at most 256 names and at most 8 KiB encoded JSON. The server may shorten a
page by bytes. Use markers/moreComing, never response length, to detect completion.
Apply the actual configured JSON/resource limits and use bounded homogeneous
batches; these are maxima, not a requirement to fill every batch.

## Shared Bootstrap And Reliable Delta

```json
{"zoneID":{"zoneName":"PrimarySync"}}
```

Persist the returned snapshotToken and an incomplete bootstrap marker. Query
CPLMaster and CPLAsset using that same token, with an independent marker per type:

```json
{"zoneID":{"zoneName":"PrimarySync"},"snapshotToken":"example-snapshot-token","query":{"recordType":"CPLMaster"},"resultsLimit":200}
```

A single type/page does not establish a complete inventory. Validate the final
one-to-one graph at shared H, then receive changes from its zone syncToken.
Snapshot/marker/database tokens must never substitute for an applied zone token.

```json
{"zones":[{"zoneID":{"zoneName":"PrimarySync"},"syncToken":"example-zone-token","resultsLimit":200}]}
```

Each fixed-H round can split an atomic photo pair. Save ordered immutable events,
inbox and ingestToken in one DbActor transaction. Project/validate only after
moreComing=false; create resource/conflict obligations in the final transaction.
Promote appliedToken only after all in-scope obligations settle. Persist unknown
record types in the remote projection and explicitly reject unsupported graphs;
never infer deletions from an incomplete or filtered listing. Database discovery
can repeat a zone across pages and has a separate progress token.

## Lease Wire And Conservative Client Deadline

```json
{"deviceId":"example-device","zoneIDs":[{"zoneName":"PrimarySync"}]}
```

Keep fencingToken as an opaque decimal string. Carry exactly one of each
`X-Sync-Lease-ID`, `X-Sync-Lease-Token`, `X-Sync-Lease-Fence` on modify,
reservation, upload body, session renew and lease control calls. The path leaseId
must match. Neither API key, deviceId nor a local mutex replaces this proof.
Lease/default timing is 90/30 seconds, but use the actual grant/renewal parameters.
A successful check moves the lease from checking into delete.

```json
{"zoneTokens":[{"zoneID":{"zoneName":"PrimarySync"},"syncToken":"example-applied-token"}],"localReady":true}
```

A localReady=true declaration is valid only after the [storage barriers](native-sync-storage.md).
A false/behind result keeps the lease and heartbeat alive while catch-up repeats.
Do not persist the response's cloud-latest token as applied merely because it
was returned. The server determines semantic currency from zone changes; own
uploads do not revoke a passed gate.

Use request-start monotonic time + returned TTL − a bounded safety margin as a
conservative deadline, never server wall-clock expiresAtMs. An unusually late
response cannot extend an already aborted cycle. Renew independently of data
transfers; transient renewal failure may retry within the previously confirmed
deadline, but grants no extension. Explicit loss/expired deadline aborts once,
cancels current/future uplink, invalidates callbacks and never reacquires in
that cycle. Settle issued operations before phase advancement, even when a phase
is empty. Release remains best-effort with the exact old proof.

## Record And Event Mapping

| Record / wire field | Client mapping |
|---|---|
| CPLAsset.recordName | stable photo identity; not a path or media_items integer ID |
| CPLAsset.masterRef | immutable partner name, REFERENCE/action=NONE, same zone |
| both contentGeneration | UUID for the original/thumbnail combination, independent of revisions |
| CPLAsset.relativePath | validated job-root-relative local placement; reject traversal, symlinks and ownership conflicts |
| CPLAsset.isFavorite | INT64 0/1 ↔ media_items.is_favorite; mark real user changes METADATA |
| CPLAsset.assetDate | TIMESTAMP UTC milliseconds ↔ display taken_at seconds; retain exact wire value in the baseline |
| CPLAsset.isHidden | server-supported but no current local hidden model; preserve remotely, do not reset/import as trash |
| CPLMaster.photoId | exactly CPLAsset.recordName |
| CPLMaster.resOriginalRes / resJPEGThumbRes | ASSETID descriptions for original / thumbnail, JPEG or PNG thumbnail |
| CPLMaster.thumbnailSourceOriginalSHA256 | lowercase hex original digest, validates the declared relationship |
| recordChangeTag | separate opaque revision per Asset and Master; conditional update/delete |

Native C1 synchronizes these fields and complete media bytes. Album covers/order,
editor history and other app-only metadata have no frozen native mapping and
remain local. A remote isHidden=1 is not silently exposed as an ordinary new
gallery item: retain the remote projection and an explicit unsupported/conflict
obligation for that in-scope item, preserving an existing complete local version
until resolution. Do not claim hidden-state synchronization or turn it into trash.
Existing image/video formats can use the same original+thumbnail
pair when the current extractor/thumbnail pipeline and server size limits support
them; this does not promise full Apple CPL video/Live Photo semantics.

Independent edit events live outside `fields`, inside `_syncserver`:

```json
{"_syncserver":{"metadataEvent":{"editedAtMs":1791936000000,"source":"example-device","eventId":"example-meta-edit"},"contentEvent":{"editedAtMs":1791935999000,"source":"example-device","eventId":"example-content-edit"}}}
```

Map metadataEvent/contentEvent/deletionEvent to local category event fields.
Each stamp has editedAtMs/source/eventId, not top-level metadataModifiedAt or
file mtime. Omitted historical events stay unknown. Server fallback `source=server`
represents commit-time evidence; never fabricate user edit times from migration,
bootstrap, capture date or transfer time. Unknown/ambiguous same-field conflicts
retain the existing complete version/copy and an explicit conflict; supported
real events compare time, then a stable source/eventId tie. Independent fields
merge by baseline differences, not by replacing the entire fields dictionary.

## Resource Transfer And Unknown Outcomes

```json
{"zoneID":{"zoneName":"PrimarySync"},"tokens":[{"recordName":"example-master","recordType":"CPLMaster","fieldName":"resOriginalRes","_syncserver":{"role":"original","intent":"create","expectedSize":1024,"expectedSha256":"0000000000000000000000000000000000000000000000000000000000000000"}}]}
```

The hash above is a placeholder, never an upload success fixture. Generate
original and JPEG/PNG thumbnail from one stable content snapshot; thumbnail
long edge ≤2048 and ≤2 MiB. Original defaults to ≤512 MiB; actual limits apply.
Reserve uses intent=create/update for create/modify phases; phase names are
`create`/`modify`, while record operationType for modification is `update`.

Validate returned descriptor identity, size and canonical 44-character padded
Base64 SHA-256. Do not confuse it with lowercase hex expectedSha256 or BLAKE3.
Changing URLs/MIME validation attributes does not change the logical asset
identity. Obtain fresh receipt/URL by description when needed, preserving the
original logical operationId/body/revision. Do not add expiry/lease/fence to its
logical fingerprint. Preserve record baselines' MIME/dimensions at their version;
fresh descriptions must not rewrite historical user fields.

Server persistent states are Reserved, Receiving, PublishPrepared, Published,
Failed, Expired. Verified/ObjectPublished are pipeline boundaries, not separate
persistent states in this baseline. A session becoming Published alone never
clears photo dirty. Bind a new/content-changed photo in an atomic record pair;
metadata-only update patches Asset fields and reuses the resources. Pair deletion
is atomic too. Retired names require new IDs when restoring/recreating a photo.

Upload has no resume. An uncertain POST first queries session status; Published
recovers the descriptor, active state is polled with bounds, Reserved may renew
under its original live lease, Failed/Expired requires a new reservation/key.
Do not replay an old-fence reservation or receipt blindly after lease loss.
A record operation can be read without a lease; Committed may contain item errors,
Pending may contain valid committed children, and UNKNOWN_ITEM cannot invalidate
known partial facts. Only validated confirmed items enter success callbacks.

Download one role at a time through bounded queues. Range + strong If-Range can
resume a persisted owned staging file: validate 206/Content-Range/ETag/length,
restart on full 200, and refresh expired URLs via Bearer description. Final
SHA-256/size is mandatory; 304 is usable only with an already verified owned copy.
A thumbnail is never replaced with an original response. Heartbeat does not
extend body idle/total limits or create upload resumability.

## Error Decisions

| Code / condition | Client action |
|---|---|
| UPLOAD_LOCK_BUSY or any failed/unconfirmed acquire | skip uplink once, keep dirty and downstream results |
| check isCurrent/mayUpload=false | held-lock catch-up/local coordination; no writes |
| UPLOAD_LEASE_LOST or local deadline | irreversibly abort every unfinished uplink item |
| SYNC_NOT_CURRENT / UPLOAD_PHASE_MISMATCH | stop invalid dispatch, preserve dirty, reconcile in a new cycle |
| CONFLICT | use returned current record and fresh pair lookup, compare baselines/events |
| PHOTO_PAIR_REQUIRED / PATH_CONFLICT / RECORD_NAME_RETIRED | explicit graph/path/recreation handling; no overwrite or fake success |
| IDEMPOTENCY_CONFLICT | verify logical body; changed revision/body needs a new operationId |
| EXPIRED_SYNC_TOKEN | abort old scope/cycle, rebuild metadata at new generation, reuse verified matching assets |
| BAD_REQUEST / unsupported protocol/feature | deterministic validation error, no retry loop |
| 401/403 or expired asset capability | distinguish connection credentials from URL refresh; never log secrets |
| 408/413/416 / checksum or thumbnail failure | retain work, enforce limits or restart owned transfer as appropriate |
| 429/503, network failure, SQL INTERNAL_ERROR | bounded backoff for reads/data; acquire still has the once-per-cycle rule |

Never translate SQL faults to expired tokens, clear dirty on a token-only
response, or count a partially received/projected photo as synchronized.
