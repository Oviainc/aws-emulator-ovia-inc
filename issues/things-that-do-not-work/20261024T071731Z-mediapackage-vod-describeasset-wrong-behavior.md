Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Egress endpoint Status is a 500 ms wall-clock timer, so ingest never reports PROCESSING and never FAILS for a source the service cannot read

_Filed: 2026-10-24T07:17:31Z_
_Operation: DescribeAsset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `EgressEndpoint$Status` reports the real processing state of one asset packaged by one packaging configuration — the model documents the value set as "QUEUED, PROCESSING, PLAYABLE, or FAILED". MediaPackage VOD ingest reads the object named by the asset's `SourceArn` out of S3 assuming the role named by `SourceRoleArn`; when that object does not exist (or the role cannot read it) the packaging job cannot succeed and the asset's endpoints end in a FAILED state — they never become PLAYABLE.

Actual: the status is computed purely from a wall clock. `crate::state::egress_status` returns `QUEUED` while `now - max(asset.created_instant, cfg.created_instant) < INGEST_PLAYABLE_AFTER` (500 ms, `state.rs`: `pub const INGEST_PLAYABLE_AFTER: Duration = Duration::from_millis(500)`) and `PLAYABLE` afterwards. Nothing about the asset's source takes part: `CreateAsset` only checks that `SourceArn`/`SourceRoleArn` are syntactically ARNs of S3/IAM (`crate::state::check_arn_members`) and never looks the object up in the S3 emulator, so an asset whose `SourceArn` names a bucket/key that does not exist — or whose `SourceRoleArn` names a role that does not exist — is reported `PLAYABLE` by `DescribeAsset`/`CreateAsset` half a second later. Two of the four modeled statuses (`PROCESSING`, `FAILED`) can never be produced by this emulator at all.

Consequence: a client that polls `DescribeAsset` waiting for the terminal state of a mis-configured ingest (wrong bucket, missing object, un-assumable role) waits for a FAILED that never arrives and instead gets PLAYABLE, i.e. the emulator reports a successful ingest for content it could not have packaged. Error handling keyed on the FAILED status never fires.

This is the FAILURE path of the ingest state machine, not the egress data plane: the already-open issue `20261022T132356Z-mediapackage-vod-egress-manifest-url-serves-nothing-createpackaginggroup` covers the advertised manifest URL answering 404 for an endpoint that reports playable. Fixing this one means deriving the status from whether the source is actually readable (and letting it settle on FAILED when it is not).

## Evidence

Spec: the model documents `EgressEndpoint$Status` as "The current processing status of the asset used for the packaging configuration. The status can be either QUEUED, PROCESSING, PLAYABLE, or FAILED." (quoted verbatim in `src/state.rs` beside `EGRESS_STATUS_QUEUED`), and `CreateAssetRequest$SourceArn` as "ARN of the source object in S3" with `SourceRoleArn` "The IAM role ARN used to access the source S3 bucket" — i.e. ingest reads that object as that role.

Handler/state: `src/state.rs` declares only two status constants (`EGRESS_STATUS_QUEUED`, `EGRESS_STATUS_PLAYABLE`) and `fn egress_status(asset, cfg, now)` chooses between them on `INGEST_PLAYABLE_AFTER` (500 ms) alone; `Self::egress_endpoints` maps every configuration of the asset's group through it. `src/ops/describe_asset.rs` (line ~176) and `src/ops/create_asset.rs` (line ~265) both project `state.egress_endpoints(stored, Instant::now())`. `src/ops/create_asset.rs` validates `SourceArn`/`SourceRoleArn` only through `crate::state::check_arn_members(.., S3_OBJECT_ARN / IAM_ROLE_ARN)`, which parses the ARN form and never resolves the object or the role.

Tests: `tests/all/packaging_lifecycle.rs::an_egress_endpoint_reports_queued_until_the_packaging_job_is_due` pins exactly the timer behavior (QUEUED then PLAYABLE); no test asserts a FAILED or PROCESSING status, and no test creates an asset whose source object is absent from the S3 emulator. So the failure path is unasserted, not covered.

## Steps to Reproduce

1. Boot the emulator and call CreatePackagingGroup (id `g1`), then CreatePackagingConfiguration (id `c1`, `packagingGroupId: g1`, an `hlsPackage` block).
2. Call CreateAsset with `id: a1`, `packagingGroupId: g1`, `sourceArn: arn:aws:s3:::no-such-bucket/no-such-key.smil` and a `sourceRoleArn` naming an IAM role that was never created (e.g. `arn:aws:iam::000000000000:role/does-not-exist`). Observe HTTP 200 and the asset stored.
3. Wait >500 ms and call DescribeAsset on `a1`: observe `egressEndpoints[0].status == "PLAYABLE"`, although neither the source object nor the role exists, so no packaging job could have run.
4. Poll DescribeAsset for a terminal failure: no response ever carries `FAILED` (nor the intermediate `PROCESSING`) — `src/state.rs` can only emit `QUEUED` or `PLAYABLE`.
