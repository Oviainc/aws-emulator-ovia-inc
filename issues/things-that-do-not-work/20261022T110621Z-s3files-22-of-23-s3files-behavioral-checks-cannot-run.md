Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3files.access-point-lifecycle-creating-to-available-then-deleted s3files.concurrent-synchronization-config-updates-optimistic-lock s3files.create-access-point-invalid-root-directory-path-rejected s3files.create-access-point-unknown-file-system-not-found s3files.create-file-system-token-reuse-different-params-rejected s3files.create-file-system-unknown-role-not-found s3files.create-mount-target-duplicate-az-conflict-rejected s3files.delete-file-system-transitions-to-deleted s3files.delete-file-system-with-mount-target-conflict-rejected s3files.delete-mount-target-frees-availability-zone-for-recreation s3files.file-system-lifecycle-creating-to-available s3files.file-system-policy-round-trip s3files.get-file-system-policy-not-found-when-absent s3files.list-access-points-filters-by-file-system s3files.list-file-systems-filters-by-bucket s3files.list-mount-targets-filters-by-file-system s3files.mount-target-lifecycle-creating-to-available-then-deleted s3files.put-synchronization-configuration-multiple-expiration-rules-rejected s3files.put-synchronization-configuration-stale-version-conflict-rejected s3files.synchronization-configuration-round-trip s3files.tag-list-untag-round-trip s3files.update-mount-target-security-groups-persisted
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: s3files|PutBucketVersioning|NotImplemented
Issue-Commit: 92ad2730926ec3062b662e938e8ed9f3fac68577

# 22 of 23 s3files behavioral checks cannot run: the S3 dependency answers NotImplemented for PutBucketVersioning during setup

> _Note: this issue was identified against the code base at commit `92ad2730926ec3062b662e938e8ed9f3fac68577`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketVersioning

## Description
Cause 1 of 1: every s3files check except one is blocked in its shared setup. Each check first creates a versioned S3 bucket to back the file system, and the S3 PutBucketVersioning call is refused by an unimplemented scaffold stub, so the check aborts before any s3files operation is exercised. 22 of 23 s3files checks error this way; the s3files behavioral surface (file-system lifecycle, mount targets, access points, policies, synchronization configuration, tagging) is therefore entirely unverified.

AWS: PutBucketVersioning is a normal, implemented S3 bucket subresource operation; enabling versioning on an existing bucket returns HTTP 200 with an empty body. It is the documented prerequisite for an s3files file system, whose bucket must be versioned.

Emulator: the request is rejected with error code 'NotImplemented' and the message 'operation PutBucketVersioning is scaffolded but not implemented in this emulator' (recorded message, as an example of the stub's response). The stub lives in the s3 service, which has not been stood up yet — tracked by the open issues 'Bootstrap the Amazon S3 emulator' (20261021T221645Z-bootstrap-service-com-amazonaws-s3-amazons3.md) and 'Implement s3 ListObjectsV2: unimplemented scaffold stub' (20261022T064119Z-s3-listobjectsv2-stub.md). Neither of those names PutBucketVersioning or the s3files checks it blocks, so this is filed to carry the s3files replay keys; it is the same class of dependency blockage already filed for b2bi (20261022T031134Z-b2bi-...) and transcribe (20261022T044732Z-...).

Why: a client cannot follow the normal s3files workflow at all against this emulator — the first step of provisioning (a versioned bucket) fails with NotImplemented, so no file system, mount target or access point can ever be created. And for this project it means 22 s3files checks report 'error' for a reason outside s3files, masking any real s3files divergence behind them.

Fix: implement PutBucketVersioning in the s3 service's bucket-subresource handler (store the versioning status on bucket state, return 200 with an empty body, and have GetBucketVersioning read it back). No change is needed in the s3files service itself; once S3 answers, re-replay the s3files checks listed below to surface the s3files-level behavior they actually assert.

Traces: the 22 s3files checks listed in trace_ids, all erroring with ClientError (NotImplemented) on PutBucketVersioning in setup.

## Evidence
Suite report: s3files 0/23 (0%), 23 failed — 22 of them with the identical traceback tail:

  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutBucketVersioning operation: operation PutBucketVersioning is scaffolded but not implemented in this emulator

The call originates in each check's inlined setup helper (_create_versioned_bucket / _create_versioned_bucket_named), which does s3.create_bucket(...) then s3.put_bucket_versioning(Bucket=..., VersioningConfiguration={'Status': 'Enabled'}).

Expected (AWS): PutBucketVersioning -> HTTP 200, empty body, bucket versioning Enabled; setup proceeds to s3files CreateFileSystem.
Actual (emulator): error code 'NotImplemented' from an unimplemented scaffold stub; the check aborts with zero s3files operations exercised.

Affected checks (all 22): access-point-lifecycle-creating-to-available-then-deleted, concurrent-synchronization-config-updates-optimistic-lock, create-access-point-invalid-root-directory-path-rejected, create-access-point-unknown-file-system-not-found, create-file-system-token-reuse-different-params-rejected, create-file-system-unknown-role-not-found, create-mount-target-duplicate-az-conflict-rejected, delete-file-system-transitions-to-deleted, delete-file-system-with-mount-target-conflict-rejected, delete-mount-target-frees-availability-zone-for-recreation, file-system-lifecycle-creating-to-available, file-system-policy-round-trip, get-file-system-policy-not-found-when-absent, list-access-points-filters-by-file-system, list-file-systems-filters-by-bucket, list-mount-targets-filters-by-file-system, mount-target-lifecycle-creating-to-available-then-deleted, put-synchronization-configuration-multiple-expiration-rules-rejected, put-synchronization-configuration-stale-version-conflict-rejected, synchronization-configuration-round-trip, tag-list-untag-round-trip, update-mount-target-security-groups-persisted.

## Steps to Reproduce
1. Using the S3 client against the emulator, create a bucket: CreateBucket(Bucket='<name>').
2. Call PutBucketVersioning(Bucket='<name>', VersioningConfiguration={'Status': 'Enabled'}).
   Expected: HTTP 200, empty body (versioning Enabled; GetBucketVersioning returns Status 'Enabled').
   Observed: ClientError with code 'NotImplemented' — 'operation PutBucketVersioning is scaffolded but not implemented in this emulator'.
3. Because every s3files check performs that call in setup before touching s3files, replaying any of the 22 checks in trace_ids reproduces the abort; e.g. replay s3files.file-system-lifecycle-creating-to-available.
4. After implementing PutBucketVersioning, re-replay all 22 checks in trace_ids to confirm and to surface the s3files behavior they assert.
