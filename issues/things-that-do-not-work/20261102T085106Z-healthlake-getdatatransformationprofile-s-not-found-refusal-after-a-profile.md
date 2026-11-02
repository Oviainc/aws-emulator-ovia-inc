Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.delete-profile-removes-all-published-versions
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake.delete-profile-removes-all-published-versions|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# GetDataTransformationProfile's not-found refusal after a profile delete is a fixed sentence that never names the requested ProfileId

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataTransformationProfile

## Description
One net-new root cause, from the behavioral replay of the healthlake data-transformation profile surface.

NOTE ON RELATED OPEN ISSUES: the open issue 20261029T182942Z-healthlake-data-transformation-profile-surface-7-divergences-fixed.md describes the same profile not-found wording as its 'Cause 2 of 7' and lists this check in its Issue-Trace-Ids, but its recorded replay key for this check (`healthlake.delete-profile-removes-all-published-versions|_assert_message#15|AssertionError|...`) no longer matches the check as replayed (the message assertion is now inline in `run`, at `run#54`), so the replay report does NOT report this check as owned by it and a fixer gated on that issue's key would not re-replay this check. The behavior is still diverging on the current build, so it is filed here against the key the suite actually produces. Related but non-covering open issues: 20261029T185351Z-healthlake-data-transformation-wire-responses-10-divergences-in.md (wire/serialization-level refusal wording and the ValidationException message member), 20261022T185152Z-healthlake-startfhirimportjob-stub.md, 20261024T000307Z-healthlake-transformdata-missing.md, 20261024T000307Z-healthlake-createdatatransformationprofile-wrong-behavior.md, 20261024T000307Z-healthlake-updateprofilewithagent-missing.md.

=== Cause 1 of 1: the ResourceNotFoundException message for a ProfileId no record holds does not identify the profile ===
Checks: healthlake.delete-profile-removes-all-published-versions

AWS: Smithy (services/service-com-amazonaws-healthlake-healthlake/smithy-model/com-amazonaws-healthlake-healthlake.json): GetDataTransformationProfile takes a required ProfileId (ProfileIdString, length 32..32, pattern ^[a-f0-9]{32}$) and an OPTIONAL ProfileVersion (range 0..99), and models ResourceNotFoundException at HTTP 404. The model documents no message text for that error, so the message is recorded behavior only. The recording words the 404 for a ProfileId that no record holds (here: a profile that was created, published twice, and then deleted) as:
  Profile '<ProfileId>' not found
Its FIXED text is "Profile '" and "' not found"; the only VARIABLE part is the ProfileId, taken verbatim from the request. The same recording shows the sibling wordings this refusal family uses, also from the request: "Profile '<ProfileId>' version <ProfileVersion> not found" for a read of a specific version of a gone profile, and "Profile '<ProfileId>' version 0 not found" when a publish cannot find the DRAFT. No recording value (the 32-hex ProfileId of any one run) is part of the requirement — it is what fills the template.

Emulator: ResourceNotFoundException is raised with the correct error code, but the message is the crate-wide fixed constant `Could not find specified profile.` — a sentence that quotes nothing from the request. The constant is declared as deliberately fixed and is the single source every profile not-found path in the crate reads, so no caller of it can ever name the ProfileId.

Why: a client or operator that parses or logs Error.Message to learn WHICH profile was missing gets a constant and cannot tell one missing profile from another; message-level assertions and log-scrapers written against AWS's wording fail against the emulator. Because the same constant family also backs the version-scoped and publish/list refusals the same check asserts afterwards, every later not-found message in this delete-then-read sequence diverges from AWS for the same reason, so the whole post-delete read path reports refusals a client cannot attribute to a resource.

Fix: the not-found wording of GetDataTransformationProfile's handler and the shared profile-not-found message it reads — build the message from the requested ProfileId (and, on the version-scoped path, from the requested ProfileVersion) instead of emitting a request-independent constant; the error code (ResourceNotFoundException) and status (404) are already right and must not change.

Traces: healthlake.delete-profile-removes-all-published-versions — GetDataTransformationProfile(ProfileId) issued after DeleteDataTransformationProfile(ProfileId) on a profile with versions 1 and 2 published.

## Evidence
Behavioral replay verdict (healthlake 10/20 passed, 10 failed; the other failures are reported as ALREADY FILED or OWNED):

[mismatch] healthlake.delete-profile-removes-all-published-versions
  File "check_healthlake_delete_profile_removes_all_published_versions.py", line 54, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message "Profile '<ProfileId>' not found", got 'Could not find specified profile.'

Error code was correct (ResourceNotFoundException); only the message diverges. Expected wording, per the recording: fixed text "Profile '" + <ProfileId from the request> + "' not found". Actual: the constant 'Could not find specified profile.'

The check's later assertions in the same sequence (not reached, because the run aborted at line 54) expect the same templated family:
  GetDataTransformationProfile(ProfileId, ProfileVersion=1|2) -> "Profile '<ProfileId>' version <n> not found"
  PublishDataTransformationProfile(ProfileId) -> "Profile '<ProfileId>' version 0 not found"
  ListDataTransformationProfileVersions(ProfileId) -> "Profile '<ProfileId>' not found"

Code locations consulted (citations only):
  services/service-com-amazonaws-healthlake-healthlake/src/state.rs:288-295 — PROFILE_NOT_FOUND_MESSAGE = "Could not find specified profile.", documented as fixed so that "a not-found answer never quotes the request back at the caller", and stated to be the one constant every profile not-found path in the crate uses.
  services/service-com-amazonaws-healthlake-healthlake/src/state.rs:297-302 — PROFILE_VERSION_NOT_FOUND_MESSAGE = "Could not find specified profile version.", fixed for the same stated reason.
  services/service-com-amazonaws-healthlake-healthlake/smithy-model/com-amazonaws-healthlake-healthlake.json — GetDataTransformationProfile: ResourceNotFoundException, httpError 404; no modeled message text.

## Steps to Reproduce
Boot the emulator and point a botocore healthlake client at it.

1. created = healthlake.create_data_transformation_profile(SourceFormat="CCDA", ProfileName=<unique name>, Source={"ProfileMapping": {"ProfileMapping": {"Resources/Patient.vm": "v1-content"}}})
2. profile_id = created["ProfileId"]
3. healthlake.publish_data_transformation_profile(ProfileId=profile_id, SourceFormat="CCDA")  # twice, so versions 1 and 2 exist
4. Confirm with healthlake.list_data_transformation_profile_versions(ProfileId=profile_id) that versions {1, 2} are listed.
5. healthlake.delete_data_transformation_profile(ProfileId=profile_id)
6. healthlake.get_data_transformation_profile(ProfileId=profile_id) and read the ClientError's Error.Code and Error.Message.

Observed: Code=ResourceNotFoundException (correct), Message='Could not find specified profile.'
Expected (AWS): Code=ResourceNotFoundException, Message="Profile '<profile_id>' not found" with profile_id taken from the request.

Then continue the same sequence to see the sibling wordings:
7. healthlake.get_data_transformation_profile(ProfileId=profile_id, ProfileVersion=1) -> expected "Profile '<profile_id>' version 1 not found"
8. healthlake.publish_data_transformation_profile(ProfileId=profile_id, SourceFormat="CCDA") -> expected "Profile '<profile_id>' version 0 not found"
9. healthlake.list_data_transformation_profile_versions(ProfileId=profile_id) -> expected "Profile '<profile_id>' not found"
