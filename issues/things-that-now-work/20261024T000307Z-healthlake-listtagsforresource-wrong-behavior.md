Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# ListTagsForResource cannot report tags of a data transformation profile

_Filed: 2026-10-24T00:03:07Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListTagsForResource$ResourceARN accepts both the `datastore/fhir/...` and the `dataTransformationProfile/...` resource forms, but the handler resolves only data store ARNs. A profile created with `Tags` (which the emulator stores on the profile record) therefore has no way to report them: the ARN of a live profile answers `ResourceNotFoundException`. Expected: the profile's stored tag set, as a `TagList`. Actual: 400 / `ResourceNotFoundException`.

## Evidence

Spec: the `AmazonResourceName` pattern shared by ListTagsForResource/TagResource/UntagResource admits `dataTransformationProfile/[a-f0-9]{32}`; `CreateDataTransformationProfile` declares `Tags` and the emulator persists them (`DataTransformationProfile.tags` in `src/state.rs`, written in `src/ops/create_data_transformation_profile.rs`). Handler: `src/ops/list_tags_for_resource.rs` lines ~150-182 use `datastore_id_from_arn` only and 404 otherwise; `state::tag_list()` exists only on `Datastore`. Tests: existing suites only exercise a profile ARN whose id names no profile, so the live-profile case is untested.

## Steps to Reproduce

1. CreateDataTransformationProfile with Tags={"env":"test"} and note the ProfileId
2. Call ListTagsForResource with ResourceARN=arn:aws:healthlake:us-east-1:<account>:dataTransformationProfile/<ProfileId>
3. Observe 400 ResourceNotFoundException instead of a Tags list containing env=test
