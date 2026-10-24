Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# TagResource rejects data transformation profile ARNs with ResourceNotFoundException

_Filed: 2026-10-24T00:03:07Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `AmazonResourceName` for TagResource$ResourceARN accepts BOTH HealthLake resource forms: `datastore/fhir/{32 chars}` and `dataTransformationProfile/[a-f0-9]{32}`. The handler resolves ONLY the data store form, so tagging an existing data transformation profile — one minted by CreateDataTransformationProfile, whose record already carries an `arn` and a `tags` map in state — is refused with `ResourceNotFoundException` instead of applying the tags. Expected: a profile ARN that names a profile the service holds has the request's tags merged onto that profile (observable through ListTagsForResource). Actual: 400 / `ResourceNotFoundException` with the fixed data-store not-found sentence, and no tag is stored.

## Evidence

Spec: model pattern on `AmazonResourceName` admits `dataTransformationProfile/[a-f0-9]{32}` (quoted verbatim in `src/state.rs` near `PROFILE_ARN_RESOURCE_PREFIX`), and `CreateDataTransformationProfile` declares a `Tags` input that the emulator does persist (`crate::state::DataTransformationProfile.tags`, set in `src/ops/create_data_transformation_profile.rs`, and `state::profile_arn()` mints the profile ARN). Handler: `src/ops/tag_resource.rs` lines ~150-175 call `crate::state::datastore_id_from_arn(&input.resource_arn)` only — that helper documents that it returns `None` for a `dataTransformationProfile` ARN — and returns `ResourceNotFoundException` on `None`. Tests: `tests/all/fhir_datastore_lifecycle.rs::an_arn_naming_no_data_store_is_not_found` and `tests/all/error_envelopes.rs` only use a profile ARN whose id NO profile was minted under, so no test covers (and none asserts) the behaviour for an existing profile's ARN.

## Steps to Reproduce

1. CreateDataTransformationProfile with SourceFormat=CCDA, ProfileName=p, Source={ProfileMapping:{ProfileMapping:{"a.json":"{}"}}} and note the returned ProfileId
2. Call TagResource with ResourceARN=arn:aws:healthlake:us-east-1:<account>:dataTransformationProfile/<ProfileId> and Tags=[{Key:env,Value:test}]
3. Observe 400 ResourceNotFoundException instead of a successful tag write
4. Call ListTagsForResource on the same ARN and observe the tag was never stored
