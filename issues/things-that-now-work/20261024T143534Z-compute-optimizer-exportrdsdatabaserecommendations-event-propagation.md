Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# ExportRDSDatabaseRecommendations never writes the CSV/metadata objects to the destination S3 bucket

_Filed: 2026-10-24T14:35:34Z_
_Operation: ExportRDSDatabaseRecommendations | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Same defect as the other export operations, in this handler: a job is recorded and its `s3Destination` (bucket, key, metadataKey) is answered, but no object is ever written to Amazon S3 and the destination bucket is never checked to exist.

Expected: the Amazon Aurora/RDS database recommendations are exported as a .csv file plus a .json metadata file into the existing bucket named by `s3DestinationConfig`; once `DescribeRecommendationExportJobs` reports the job `Complete`, those two objects are readable at the reported keys. A bucket that does not exist (or that the service cannot write to) must be refused with `InvalidParameterValueException` (400), since Compute Optimizer does not create the bucket.

Actual: any non-blank bucket string is accepted, the job walks to `Complete` with `failureReason: null`, and nothing is ever written to the S3 emulator, so the documented export-then-download workflow cannot complete.

## Evidence

Spec: `ExportRDSDatabaseRecommendations` documentation — recommendations are exported to "an existing Amazon S3 bucket that you specify" as a CSV file with JSON metadata; output `s3Destination` names bucket/key/metadataKey; `InvalidParameterValueException` (400) is modelled.

Handler: `src/ops/export_rds_database_recommendations.rs:264-276` validates only that the bucket name is non-blank; `:326-345` computes the keys and stores the job; there is no S3 write in the crate. `src/state.rs:4482-4493` always reports `failureReason: None`.

Tests: `tests/all/rds_database_recommendations_export.rs` asserts the job record and reported destination; no test asserts the exported objects exist, so the behaviour is unpinned.

## Steps to Reproduce

1. Opt the account in with `UpdateEnrollmentStatus` `{"status":"Active"}`.
2. Create a bucket `co-exports` through the S3 emulator.
3. Call `ExportRDSDatabaseRecommendations` with `{"s3DestinationConfig":{"bucket":"co-exports"},"includeMemberAccounts":false}`; keep `jobId` and `s3Destination`.
4. Poll `DescribeRecommendationExportJobs` until the job reports `Complete`.
5. `GetObject` the reported `key` and `metadataKey` — both 404 instead of returning the export and its metadata.
6. Repeat step 3 naming a bucket that was never created — accepted with 200 instead of `InvalidParameterValueException`.
