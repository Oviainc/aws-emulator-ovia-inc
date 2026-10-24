Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# CreateHarvestJob never validates the S3 harvest destination (bucket existence/writability, or a RoleArn that names no role in this account)

_Filed: 2026-10-24T01:07:18Z_
_Operation: CreateHarvestJob | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: MediaPackage pre-flights the harvest DESTINATION before it accepts a job. A `S3Destination` whose bucket the service cannot write to as the supplied `RoleArn` — including a bucket that does not exist in the account — is refused with the modelled `UnprocessableEntityException` (HTTP 422) and NO job is persisted (a following `DescribeHarvestJob`/`ListHarvestJobs` must not show it). A `RoleArn` that names no role at all in the caller's own account is likewise unusable, so a submission naming one is refused.

Actual: `src/ops/create_harvest_job.rs` validates only (a) the string lengths, (b) the window ordering, (c) the IAM role's TRUST POLICY — and only when the IAM store happens to hold a role with that exact ARN (`crate::state::harvest_role_refusal` returns `None` on its first `?` when `iam.roles` has no matching ARN). The destination BUCKET is never looked at: `s3_destination` is copied verbatim into `HarvestJobRecord` and the handler never reaches into the engine's S3 store. So `CreateHarvestJob` with `S3Destination { bucketName: "does-not-exist", manifestKey: "k", roleArn: "arn:aws:iam::<emulator-account>:role/never-created" }` answers HTTP 200 and persists an `IN_PROGRESS` job that is then reported by `DescribeHarvestJob` and `ListHarvestJobs`.

Consequence: a client whose harvest destination is misconfigured is told the submission was accepted, and the phantom job leaks into every later read — the same leak class the already-closed startover-precondition cause produced. The fix belongs in the same pre-flight the role-trust check already lives in: resolve the bucket against the shared S3 store (the handler already reaches the IAM store in the same engine, so the cross-store seam exists), and treat a role ARN whose account is this emulator's own account but which the IAM store holds no role for as unassumable (keep the deliberate exemption for ARNs belonging to another account).

## Evidence

Spec/recorded grounding: the Smithy model declares `UnprocessableEntityException` (httpError 422) on `CreateHarvestJob` and types `S3Destination` with required `BucketName`/`ManifestKey`/`RoleArn` (`smithy_query(query="operation",operation="CreateHarvestJob")`). The sibling service's recording proves MediaPackage pre-flights the destination: the open issue `20261022T103649Z-mediapackagev2-...` documents AWS answering `ValidationException` with `ValidationExceptionType = HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION` ("Unable to write to S3 destination bucket. Verify MediaPackage... is allow listed write access to the bucket") for exactly this input, and the recording's next step shows the job ABSENT from `ListHarvestJobs`.

Handler: `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_harvest_job.rs` lines ~208-330 — the only destination check is `crate::state::harvest_role_refusal(&input.s3_destination.role_arn, &iam.lock())`; `state.rs` ~1448-1476 documents that an ARN the IAM store holds no role for is accepted, and nothing in the crate references the S3 store (`grep` for `s3` in `src/` finds only the modelled `S3Destination` members).

Test coverage: no test covers a destination the service could not write to. On the contrary, `tests/all/harvest_jobs.rs` submits with placeholder destinations (`"bucketName": "b", "roleArn": "r"`) and expects 200, so the current behaviour is pinned by the emulator's own fixtures only.

Not a re-file: the closed issue `20261022T213438Z` (cause 3) was about the operation answering `NotImplemented` for an UNASSUMABLE role and was closed by the trust-policy check above; the destination bucket — named in that issue only as a parenthetical in its fix note — and the nonexistent-role-in-own-account case are still unimplemented and are what this issue is about.

## Steps to Reproduce

1. Boot the emulator and point boto3 at it (region us-west-2).
2. mp.create_channel(Id="c1"); mp.create_origin_endpoint(Id="oe1", ChannelId="c1", HlsPackage={}, StartoverWindowSeconds=86400)
3. Do NOT create any S3 bucket or IAM role. Call mp.create_harvest_job(Id="hj1", OriginEndpointId="oe1", StartTime=<past>, EndTime=<past+1h>, S3Destination={"BucketName": "does-not-exist-bucket", "ManifestKey": "out/index.m3u8", "RoleArn": "arn:aws:iam::000000000000:role/never-created"}).
4. Observe HTTP 200 with a persisted job; AWS refuses with UnprocessableEntityException / 422 and persists nothing.
5. Call mp.list_harvest_jobs() and mp.describe_harvest_job(Id="hj1") -> the phantom job is reported (AWS: the listing has no such entry and the describe is a 404).
