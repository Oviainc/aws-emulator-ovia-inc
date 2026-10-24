Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Harvest jobs never export anything to the S3 destination and always end FAILED instead of SUCCEEDED

_Filed: 2026-10-24T07:15:59Z_
_Operation: CreateHarvestJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `CreateHarvestJob` submits a job that exports a window of an origin endpoint's packaged content to the caller's S3 bucket. The job is created `IN_PROGRESS`, and once the clock has made the transition due it reaches the modelled terminal status `SUCCEEDED` for a well-formed submission (valid window inside the endpoint's startover window, a writable bucket, an assumable role). As part of that, the top-level manifest named by `S3Destination$ManifestKey` ("The key in the specified S3 bucket where the harvested top-level manifest will be placed") appears as an object in `S3Destination$BucketName`, so a client can follow a successful harvest with a GetObject on the manifest key and find it.

Actual: the emulator hard-codes the terminal status to `FAILED` for EVERY job, and writes nothing anywhere. `src/state.rs` declares `pub const HARVEST_OUTCOME: &str = HARVEST_FAILED;` and `CreateHarvestJob` stores `next_status_due: Some((now + HARVEST_SETTLE_AFTER, crate::state::HARVEST_OUTCOME))`, so 500 ms after any submission every job settles to `FAILED` whatever the request was. No handler in the crate touches the S3 store (the crate reaches cross-service state only for IAM, via `crate::state::iam_state`, to check the role's trust policy), so `ManifestKey` is copied verbatim into the record and no object is ever produced in the destination bucket.

Consequence: the only observable outcome of the emulator's harvest workflow is failure. A client that polls `DescribeHarvestJob`/`ListHarvestJobs` until the job leaves `IN_PROGRESS` always lands on `FAILED`, so a "harvest succeeded" branch can never be exercised, and a workflow that harvests live content to S3 and then reads the manifest back out of the bucket cannot be emulated at all. The crate documents the choice (state.rs ~1136-1146: "this emulator serves no MediaPackage data plane ... so no harvest runs and `SUCCEEDED` would claim content that is not there"), which is an honest explanation of the cause but still a divergence from the service: the status reported is not computed from the submission, and nothing is exported.

## Evidence

Spec grounding: `smithy_query(query="operation",operation="CreateHarvestJob")` / `shape=com.amazonaws.mediapackage#Status` — the modelled `Status` enum is `IN_PROGRESS | SUCCEEDED | FAILED`, and `S3Destination` is documented as "Configuration parameters for where in an S3 bucket to place the harvested content" with required `BucketName` / `ManifestKey` / `RoleArn`; `HarvestJob$Status` is "The current status of the HarvestJob. Consider setting up a CloudWatch Event to listen for HarvestJobs as they succeed or fail." — a service whose jobs can only fail makes the success half of that contract unreachable.

Handler/state: `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_harvest_job.rs` lines ~306-323 set `status: HARVEST_IN_PROGRESS` and `next_status_due: Some((now + HARVEST_SETTLE_AFTER, HARVEST_OUTCOME))`; `src/state.rs` line ~1146 `pub const HARVEST_OUTCOME: &str = HARVEST_FAILED;` and `HarvestJobRecord::settle` (~1194) applies that one constant. `grep` for an S3 store in `src/` finds nothing — only the modelled `S3Destination` members.

In-repo corroboration that this is a recognised divergence class: the open sibling issue `20261024T051159Z-mediapackagev2-createharvestjob-event-propagation.md` ("Harvest jobs never export anything to the S3 destination and always end FAILED instead of COMPLETED") files exactly this gap against MediaPackage v2; the v1 crate has the same cause and no issue of its own.

Test coverage: `tests/all/harvest_jobs.rs` pins the current behaviour rather than AWS's — it polls for `state::HARVEST_OUTCOME` (lines ~649-653) and filters on it, so the FAILED-only outcome is asserted by the emulator's own fixtures. No test asserts a SUCCEEDED job, and no test looks for the harvested manifest object in the destination bucket.

## Steps to Reproduce

1. Boot the emulator and point an SDK/boto3 client at it (region us-west-2).
2. Create the S3 destination bucket in the emulator's S3 service: s3.create_bucket(Bucket="harvest-out"), and an IAM role whose trust policy admits mediapackage.amazonaws.com.
3. mp.create_channel(Id="c1"); mp.create_origin_endpoint(Id="oe1", ChannelId="c1", HlsPackage={}, StartoverWindowSeconds=86400).
4. mp.create_harvest_job(Id="hj1", OriginEndpointId="oe1", StartTime=<past>, EndTime=<past+1h>, S3Destination={"BucketName": "harvest-out", "ManifestKey": "out/index.m3u8", "RoleArn": <the role ARN>}) -> 200, Status IN_PROGRESS.
5. Wait ~1s and call mp.describe_harvest_job(Id="hj1") -> Status is FAILED; AWS reports SUCCEEDED for a submission it accepted against a writable destination.
6. s3.get_object(Bucket="harvest-out", Key="out/index.m3u8") -> NoSuchKey; after a successful AWS harvest the top-level manifest exists at that key.
