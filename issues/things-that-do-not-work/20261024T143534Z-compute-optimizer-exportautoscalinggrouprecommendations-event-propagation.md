Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# ExportAutoScalingGroupRecommendations never writes the CSV/metadata objects to the destination S3 bucket

_Filed: 2026-10-24T14:35:34Z_
_Operation: ExportAutoScalingGroupRecommendations | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler mints a job id, computes the destination object keys and records a job whose status walks Queued -> InProgress -> Complete, but it never writes anything to Amazon S3 and never checks that the destination bucket exists.

Expected: the operation exports the Auto Scaling group recommendations "in a comma-separated values (.csv) file, and its metadata in a JavaScript Object Notation (.json) file, to an existing Amazon S3 bucket that you specify". After the job reports `Complete`, a `GetObject`/`ListObjectsV2` against the bucket and the `s3Destination.key` / `s3Destination.metadataKey` this call answered must return the export file and its metadata. And because "you must create the destination Amazon S3 bucket for your recommendations export before you create the export job — Compute Optimizer does not create the S3 bucket for you", a request naming a bucket that does not exist (or that Compute Optimizer cannot write to) must be refused with `InvalidParameterValueException` (400) rather than accepted.

Actual: any non-empty `s3DestinationConfig.bucket` string is accepted, the response reports `s3Destination` keys, the job reaches `Complete` with `failureReason: null`, and the two objects never appear in the S3 emulator — a client that follows the documented workflow (export, poll `DescribeRecommendationExportJobs` until Complete, download the CSV) gets 404 NoSuchKey/NoSuchBucket from S3 forever. The crate already takes cross-service dependencies (`service-com-amazonaws-ecs-...`, `service-com-amazonaws-cloudwatch-...`) for `GetECSServiceRecommendations`, so writing into the S3 emulator's store is the same mechanism; `src/state.rs:4352` justifies the omission with "the destination is a bucket in the CALLER's account, which this emulator does not serve", which is not true in this repo (`services/service-com-amazonaws-s3-amazons3`).

## Evidence

Spec: `ExportAutoScalingGroupRecommendations` documentation ("Recommendations are exported in a comma-separated values (.csv) file, and its metadata in a .json file, to an existing Amazon S3 bucket that you specify"); output `s3Destination` (`S3Destination` = bucket + key + metadataKey) "describes the destination Amazon S3 bucket of a recommendations export file"; `s3DestinationConfig` doc ("You must create the destination bucket before you create the export job"); modelled errors include `InvalidParameterValueException` (400).

Handler: `src/ops/export_auto_scaling_group_recommendations.rs:259-271` only rejects an empty/blank bucket name; `:321-340` computes `export_destination_keys(...)` and stores an `ExportJob` — there is no S3 write anywhere in the crate (`grep -rn "s3" src/` finds only the destination shapes). `src/state.rs:4482-4493` reports `failureReason: None` for every job and `src/state.rs:4352-4354` states the exported file is deliberately not held.

Tests: `tests/all/auto_scaling_group_recommendations_export.rs` asserts the job record and the reported destination only; no test asserts the object exists in S3, so nothing pins the current behaviour.

## Steps to Reproduce

1. Boot the emulator and opt the account in with `UpdateEnrollmentStatus` `{"status":"Active"}`.
2. Create a bucket `co-exports` through the S3 emulator.
3. Call `ExportAutoScalingGroupRecommendations` with `{"s3DestinationConfig":{"bucket":"co-exports","keyPrefix":"asg"},"includeMemberAccounts":false}` and keep the returned `jobId` and `s3Destination`.
4. Poll `DescribeRecommendationExportJobs` with `{"jobIds":["<jobId>"]}` until `status` is `Complete`.
5. `GetObject` the reported `s3Destination.key` (and `.metadataKey`) from `co-exports` — both 404 instead of returning the CSV/JSON export.
6. Repeat step 3 with `{"s3DestinationConfig":{"bucket":"no-such-bucket-at-all"}}` — the call is accepted with 200 and a job id instead of being refused with `InvalidParameterValueException`.
