Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the CloudFront emulator

_Filed: 2026-10-21T23:28:05Z_
_Operation: service-com-amazonaws-cloudfront-cloudfront2020-05-31 | Class: missing_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 167 operations in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding no state. CloudFront therefore has no working emulator at all.

Implement a FIRST SLICE of 6 operations that makes the distribution lifecycle plus cache invalidation work end-to-end over the `restXml` protocol (SDK id `CloudFront`, signing name `cloudfront`, all routes under `/2020-05-31/...`). Operations outside this slice stay 501 and are filed separately.

### 1. CreateDistribution — `POST /2020-05-31/distribution -> 201`
- Input: `DistributionConfig` (@httpPayload, required). Required members inside it: `CallerReference`, `Origins`, `DefaultCacheBehavior`, `Comment`, `Enabled`.
- Must allocate a new distribution id (CloudFront style, e.g. `E` + 13 uppercase alphanumerics), persist the full `DistributionConfig` verbatim, and synthesize `ARN` = `arn:aws:cloudfront::<account>:distribution/<Id>`, `DomainName` = `<lowercase-random>.cloudfront.net`, `Status` = `Deployed`, `LastModifiedTime` = now, `InProgressInvalidationBatches` = 0.
- Output: `Distribution` (@httpPayload) containing `Id`, `ARN`, `Status`, `LastModifiedTime`, `InProgressInvalidationBatches`, `DomainName`, `DistributionConfig`; plus `Location` header (`/2020-05-31/distribution/<Id>`) and an `ETag` header holding a per-distribution version token that changes on every mutation.
- `CallerReference` already seen for a stored distribution must fail `DistributionAlreadyExists (409)`.

### 2. GetDistribution — `GET /2020-05-31/distribution/{Id} -> 200`
- Input: `Id` (@httpLabel, required). Output: `Distribution` (@httpPayload) + `ETag` header.
- Must return the stored distribution, byte-for-byte the config that was created/updated (not an echo of the current request), with the SAME `ETag` the last mutation returned. Unknown id -> `NoSuchDistribution (404)`.

### 3. ListDistributions — `GET /2020-05-31/distribution -> 200`
- Input: `Marker`, `MaxItems` (@httpQuery). Output: `DistributionList` (@httpPayload) with required `Marker`, `MaxItems`, `IsTruncated`, `Quantity` and `Items` (list of `DistributionSummary`).
- Must enumerate persisted distributions (not an empty canned list), honor `MaxItems` as a page size, set `IsTruncated`/`NextMarker` when more remain, and resume from `Marker`. `Quantity` must equal `Items.len()` for the page.

### 4. UpdateDistribution — `PUT /2020-05-31/distribution/{Id}/config -> 200`
- Input: `Id` (@httpLabel), `DistributionConfig` (@httpPayload, required), `IfMatch` (`If-Match` header).
- Must replace the stored config, bump `LastModifiedTime` and the ETag, and return the updated `Distribution` + new `ETag`. A later GetDistribution must observe the change (e.g. `Enabled: false`).
- `IfMatch` not matching the current ETag -> `PreconditionFailed (412)`; missing/garbage If-Match -> `InvalidIfMatchVersion (400)`; unknown id -> `NoSuchDistribution (404)`.

### 5. CreateInvalidation — `POST /2020-05-31/distribution/{DistributionId}/invalidation -> 201`
- Input: `DistributionId` (@httpLabel), `InvalidationBatch` (@httpPayload, required: `Paths`, `CallerReference`).
- Must store the invalidation under its distribution with a generated `Id`, `Status` (`InProgress` then observably `Completed`), `CreateTime` = now, and the submitted `InvalidationBatch` preserved. Output: `Invalidation` (@httpPayload) + `Location` header. Unknown distribution -> `NoSuchDistribution (404)`.
- `GetInvalidation` (`GET /2020-05-31/distribution/{DistributionId}/invalidation/{Id}`) should return the same stored record if included; otherwise the invalidation must still be retrievable via `ListInvalidations` in a later slice.

### 6. DeleteDistribution — `DELETE /2020-05-31/distribution/{Id} -> 204`
- Input: `Id` (@httpLabel), `IfMatch` (`If-Match` header). No output members; 204 on success.
- Must enforce real CloudFront semantics: deleting a distribution whose stored config has `Enabled: true` -> `DistributionNotDisabled (409)`; ETag mismatch -> `PreconditionFailed (412)`; unknown id -> `NoSuchDistribution (404)`. After a successful delete the distribution must be gone: GetDistribution -> `NoSuchDistribution (404)` and it must not appear in ListDistributions.

### NEVER STUBS
Every one of these six must compute its response from persisted state. Hardcoded `Distribution` bodies, echoing the request payload back without storing it, or generating an id without recording it are failures — a create followed by a get/list must return exactly what was created.

## State Model
In `src/state.rs` (`ComAmazonawsCloudfrontCloudfront20200531State`, reached via `crate::state::service_state(&*self.engine)`):

- `distributions: BTreeMap<String /*Id*/, StoredDistribution>` — insertion-ordered-by-id map, the authoritative store.
- `StoredDistribution { id: String, arn: String, domain_name: String, status: String /*"Deployed"*/, last_modified_time: DateTime, in_progress_invalidation_batches: i32, etag: String, config: DistributionConfig /*the modeled shape, stored whole*/, tags: Vec<(String,String)>, invalidations: BTreeMap<String /*InvalidationId*/, StoredInvalidation> }`
- `StoredInvalidation { id: String, status: String /*"InProgress"|"Completed"*/, create_time: DateTime, batch: InvalidationBatch }`
- `caller_references: HashMap<String /*CallerReference*/, String /*distribution Id*/>` — for `DistributionAlreadyExists` detection.
- Monotonic counters/seeds for id and ETag generation so each mutation yields a fresh ETag (e.g. `next_etag_seq: u64`).

ETags live on `StoredDistribution.etag` and are the single source of truth for the `If-Match` checks in UpdateDistribution and DeleteDistribution.

## Evidence

`services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json`) documents: protocol `restXml`, 167 operations; CreateDistribution `POST /2020-05-31/distribution -> 201` with `DistributionConfig` @httpPayload and outputs `Distribution`/`Location`/`ETag` plus `DistributionAlreadyExists (409)`; GetDistribution `GET /2020-05-31/distribution/{Id} -> 200` with `NoSuchDistribution (404)`; ListDistributions `GET /2020-05-31/distribution -> 200`, @paginated(inputToken=Marker, outputToken=DistributionList.NextMarker, items=DistributionList.Items); UpdateDistribution `PUT /2020-05-31/distribution/{Id}/config -> 200` with `IfMatch` header, `PreconditionFailed (412)`, `InvalidIfMatchVersion (400)`; CreateInvalidation `POST /2020-05-31/distribution/{DistributionId}/invalidation -> 201` with `InvalidationBatch` @httpPayload and `NoSuchDistribution (404)`; DeleteDistribution `DELETE /2020-05-31/distribution/{Id} -> 204` with `DistributionNotDisabled (409)`, `PreconditionFailed (412)`, `NoSuchDistribution (404)`. smithy_query on shapes `Distribution`, `DistributionConfig`, `Invalidation`, `DistributionList` gives the required members cited above (Distribution.Id/ARN/Status/LastModifiedTime/InProgressInvalidationBatches/DomainName/DistributionConfig; DistributionConfig.CallerReference/Origins/DefaultCacheBehavior/Comment/Enabled; Invalidation.Id/Status/CreateTime/InvalidationBatch; DistributionList.Marker/MaxItems/IsTruncated/Quantity/Items). A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` across `src/ops/*.rs` matches every operation file, including `create_invalidation.rs`, confirming zero implemented handlers.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI CloudFront client at it.
2. `CreateDistribution` with a DistributionConfig (CallerReference="ref-1", one S3/custom origin, a DefaultCacheBehavior, Comment="demo", Enabled=true) — expect HTTP 201 with a generated Id, ARN, DomainName, Location header and ETag (today: 501 NotImplemented).
3. `GetDistribution` with that Id — expect 200 and the exact config submitted, same ETag.
4. `ListDistributions` — expect the new distribution in DistributionList.Items with Quantity=1, IsTruncated=false; with MaxItems=1 and two distributions created, expect IsTruncated=true + NextMarker, and the second page resumes via Marker.
5. `CreateInvalidation` on that distribution with Paths=["/*"] and CallerReference="inv-1" — expect 201, an Invalidation with Id/Status/CreateTime and the submitted batch echoed from storage; `GetInvalidation` returns the same record.
6. `DeleteDistribution` while Enabled=true — expect 409 DistributionNotDisabled.
7. `UpdateDistribution` with the same config but Enabled=false and If-Match set to the current ETag — expect 200, a new ETag; repeat with a stale ETag and expect 412 PreconditionFailed.
8. `GetDistribution` — expect the stored config to show Enabled=false (proving the update persisted).
9. `DeleteDistribution` with the current ETag — expect 204; then `GetDistribution` expects 404 NoSuchDistribution and `ListDistributions` no longer lists it.
