Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceac96a2525794129e4bd0e80b1424d3e5c0e5c2

# An EXPORT job is created as a single plain job: no main job / job parts, so IsMaster is always false and SubJobMetadata never appears

_Filed: 2026-10-24T04:03:04Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceac96a2525794129e4bd0e80b1424d3e5c0e5c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

On the real service a successful request to create an EXPORT job produces a *main* job that is not associated with any device, plus at least one job part per device; the main job is reported by ListJobs with `IsMaster=true`, and DescribeJob on it returns the parts in `SubJobMetadata`.

The emulator creates one flat record for every job regardless of `JobType`: `StoredJob::to_job_list_entry` hardcodes `is_master: false` and `DescribeJob` hardcodes `sub_job_metadata: None`, so an EXPORT job is indistinguishable from an IMPORT job and no job part ever exists. A client that keys off `IsMaster` (to tell a main job from its parts) or that walks `SubJobMetadata` to find the device-bearing parts of an export order sees nothing to walk.

## Evidence

Spec: `JobListEntry$IsMaster` — "A value that indicates that this job is a main job. A main job represents a successful request to create an export job. Main jobs aren't associated with any Snowballs. Instead, each main job will have at least one job part, and each job part is associated with a Snowball" (smithy_query doc shape=JobListEntry member=IsMaster); `DescribeJobResult$SubJobMetadata` — "Information about a specific job part (in the case of an export job)". Handler/state: src/ops/create_job.rs (mints a `JID` job and stores it flat, no parts) ; src/state.rs `to_job_list_entry` sets `is_master: false` with a comment that this service never mints a main job; src/ops/describe_job.rs line 166 sets `sub_job_metadata: None` unconditionally. Tests: tests/all/job_lifecycle.rs asserts the opposite — `describe_job_reports_the_job_create_job_stored` requires `SubJobMetadata` to be absent for every row, including the `JobType=EXPORT` rows — so no test covers the export main-job/job-part structure.

## Steps to Reproduce

1. CreateAddress, keep the minted AddressId
2. CreateJob with {"AddressId": "<id>", "JobType": "EXPORT", "SnowballType": "EDGE_S", "ShippingOption": "STANDARD", "RoleARN": "arn:aws:iam::123456789012:role/snowball", "Resources": {"S3Resources": [{"BucketArn": "arn:aws:s3:::export-src"}]}}
3. ListJobs — observe the entry for that job with IsMaster absent/false, where a main export job is reported with IsMaster=true
4. DescribeJob on that JobId — observe no SubJobMetadata member at all, i.e. the export order has no job parts
