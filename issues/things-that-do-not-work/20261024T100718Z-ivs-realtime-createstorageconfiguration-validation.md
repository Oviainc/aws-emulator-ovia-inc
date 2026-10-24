Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ca029f426fda2d51f4b9c87edd69d4539a13e33a

# CreateStorageConfiguration accepts an S3 bucket that does not exist and never touches the bucket policy

_Filed: 2026-10-24T10:07:18Z_
_Operation: CreateStorageConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `ca029f426fda2d51f4b9c87edd69d4539a13e33a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation is documented as a cross-service action: "Creates a new storage configuration, used to enable recording to Amazon S3. When a StorageConfiguration is created, IVS will modify the S3 bucketPolicy of the provided bucket. This will ensure that IVS has sufficient permissions to write content to the provided bucket." Consistently, the operation models both ResourceNotFoundException (404) and ValidationException (400) — errors that have no other referent on a create whose only resource reference is the destination bucket.

Expected: a syntactically valid `s3.bucketName` that names no bucket the account holds is refused synchronously (HTTP 400 ValidationException, in the same shape AWS uses for the sibling IVS CreateRecordingConfiguration bucket check) and NOTHING is persisted — no ARN minted, and the configuration is absent from GetStorageConfiguration/ListStorageConfigurations afterwards. A bucket that does exist is accepted and its bucket policy is amended so IVS can write recordings into it.

Actual: the handler mints an ARN and persists the record from the request alone; `bucket_name: input.s3.bucket_name` is stored with an explicit comment that the bucket is never looked up and no S3 state is consulted. Every bucket name is accepted, so a client that mistypes or forgets to create its recording bucket gets a successful create locally (and would later lose recordings silently) instead of AWS's up-front rejection. No bucket policy is written either, so a follow-up GetBucketPolicy against the emulated S3 bucket shows no IVS grant.

## Evidence

Spec: smithy_query(query="doc", shape="CreateStorageConfiguration") — IVS modifies the provided bucket's bucketPolicy; smithy_query(query="operation", operation="CreateStorageConfiguration") — errors include ResourceNotFoundException (404) and ValidationException (400); S3StorageConfiguration$bucketName is @required with @length(3..63)/@pattern.
Well-established behaviour, recorded for the sibling service: the open IVS issue 20261023T004057Z (Cause 4) records real AWS refusing a non-existent destination bucket on CreateRecordingConfiguration with 400 ValidationException and creating nothing — the same bucket-policy mechanism as here.
Feasible in-repo: other crates resolve buckets through the shared engine, e.g. services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:1347 and services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/list_s3_buckets.rs:119 call `service_com_amazonaws_s3_amazons3::state::service_state(engine)`.
Handler: src/ops/create_storage_configuration.rs `handle` (lines 156-223), specifically the record build at lines 204-215 ("The bucket is not looked up: this emulator holds no S3 state…").
Tests: tests/all/storage_configurations.rs creates configurations with bucket names such as "shared" that were never created in S3 and asserts HTTP 200 — the suite codifies the unchecked acceptance; no test covers a missing bucket.

## Steps to Reproduce

1. POST /CreateStorageConfiguration {"name":"rec","s3":{"bucketName":"ivs-bucket-that-was-never-created"}}
2. Expected: HTTP 400 ValidationException naming s3/bucketName, and no configuration persisted
3. Actual: HTTP 200 with a minted storageConfiguration.arn; POST /GetStorageConfiguration with that ARN returns the configuration
4. Create a bucket in the emulated S3, then CreateStorageConfiguration against it and read that bucket's policy → expected an IVS write grant, actual no policy change
