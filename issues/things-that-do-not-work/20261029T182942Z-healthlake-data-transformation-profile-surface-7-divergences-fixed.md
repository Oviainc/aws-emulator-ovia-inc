Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.data-transformation-profile-draft-update-persists healthlake.delete-profile-removes-all-published-versions healthlake.get-profile-out-of-range-version-not-found healthlake.publish-profile-exceeds-version-quota-rejected healthlake.start-transformation-job-nonexistent-profile-not-found healthlake.start-transformation-job-untrusted-role-rejected healthlake.update-profile-mapping-exceeds-entry-limit-rejected
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake|GetDataTransformationProfile|ResourceNotFoundException
Issue-Key: healthlake.delete-profile-removes-all-published-versions|_assert_message#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: healthlake.get-profile-out-of-range-version-not-found|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: healthlake.publish-profile-exceeds-version-quota-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: healthlake.start-transformation-job-nonexistent-profile-not-found|run#36|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: healthlake.start-transformation-job-untrusted-role-rejected|_error#25|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: healthlake.update-profile-mapping-exceeds-entry-limit-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# HealthLake data-transformation profile surface: 7 divergences — fixed not-found/quota/validation wording, no DRAFT fallback on Get, and StartDataTransformationJob accepts an untrusted DataAccessRoleArn

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataTransformationProfile

## Description
Seven distinct root causes in the HealthLake data-transformation surface, all found by the behavioral replay. They share a service (healthlake) and are bundled here; each is numbered and carries its own failing check id. A fixer must close ALL SEVEN before submitting.

None of the open healthlake issues cover these: 20261022T185152Z-healthlake-startfhirimportjob-stub.md (StartFHIRImportJob stub), 20261024T000307Z-healthlake-createdatatransformationprofile-wrong-behavior.md (501 for StarterProfile/SampleData sources), 20261024T000307Z-healthlake-transformdata-missing.md, 20261024T000307Z-healthlake-updateprofilewithagent-missing.md, 20261028T035414Z-healthlake-startdatatransformationjob-wrong-behavior.md (reused ClientToken), 20261029T115812Z-healthlake-tagresource-missing.md, 20261029T160047Z-healthlake-createdatatransformationprofile-validation.md (retained-byte ceiling), 20261029T165933Z-healthlake-deletefhirdatastore-maintainability.md, 20261029T172219Z-healthlake-createfhirdatastore-wrong-behavior--r2.md.

=== Cause 1 of 7: GetDataTransformationProfile with ProfileVersion omitted does not fall back to the DRAFT ===
Checks: healthlake.data-transformation-profile-draft-update-persists
AWS: recorded behavior only (Smithy: GetDataTransformationProfile.ProfileVersion is OPTIONAL, @httpQuery "version", range 0..99; output.Version required). The recording shows that for a profile created and then updated but NEVER published, GetDataTransformationProfile(ProfileId) with no ProfileVersion returns 200 with Version=0 and the DRAFT's current ProfileMapping and LastUpdatedAt. The DRAFT (version 0) is a real, readable version and is what the service reports when there is no published version.
Emulator: refuses with ResourceNotFoundException (HTTP 404), message "Could not find specified profile version." — the handler resolves an omitted ProfileVersion as latest_published() ONLY and 404s when the profile has only its DRAFT.
Why: a client's normal read-after-write loop (create -> update -> get) on an unpublished profile is broken: the draft content it just wrote is unreadable, so authoring a profile before the first publish is impossible against the emulator.
Fix: the omitted-version resolution in GetDataTransformationProfile's handler — fall back to the DRAFT (version 0) when no published version exists, and keep returning the latest published version when one does.
Traces: healthlake.data-transformation-profile-draft-update-persists (GetDataTransformationProfile after UpdateDataTransformationProfile on a never-published profile).

=== Cause 2 of 7: profile-not-found message on GetDataTransformationProfile is a fixed sentence that does not name the profile ===
Checks: healthlake.delete-profile-removes-all-published-versions
AWS: Smithy: GetDataTransformationProfile errors ResourceNotFoundException (HTTP 404); the model documents no message text, so recorded behavior only. The recording words the 404 for a ProfileId no record holds as: Profile '<ProfileId>' not found — fixed text "Profile '", "' not found"; the variable part is the ProfileId taken from the request.
Emulator: ResourceNotFoundException with the fixed constant 'Could not find specified profile.' (crate::state::PROFILE_NOT_FOUND_MESSAGE), which quotes nothing from the request.
Why: clients and operators that parse or log the message to learn WHICH profile was missing get a constant; message assertions against AWS wording fail.
Fix: the not-found wording used by GetDataTransformationProfile's handler (its not_found() helper and the shared message constant it reads) — build the message from the requested ProfileId.
Traces: healthlake.delete-profile-removes-all-published-versions (GetDataTransformationProfile after DeleteDataTransformationProfile).

=== Cause 3 of 7: version-not-found message on GetDataTransformationProfile names neither the profile nor the version ===
Checks: healthlake.get-profile-out-of-range-version-not-found
AWS: Smithy: GetDataTransformationProfile.ProfileVersion range 0..99; error ResourceNotFoundException (HTTP 404). Message text is recorded behavior only: for an in-range version that was never published, AWS answers Profile '<ProfileId>' version <ProfileVersion> not found — fixed text "Profile '", "' version ", " not found"; the variable parts are the ProfileId and the ProfileVersion from the request.
Emulator: ResourceNotFoundException with the fixed constant 'Could not find specified profile version.' (crate::state::PROFILE_VERSION_NOT_FOUND_MESSAGE).
Why: a caller cannot tell from the refusal whether the profile or the version was the problem, nor which version it asked for.
Fix: the version-resolution refusal in GetDataTransformationProfile's handler and the shared constant it uses — interpolate ProfileId and the requested version.
Traces: healthlake.get-profile-out-of-range-version-not-found (GetDataTransformationProfile ProfileVersion=5 on a profile with only version 1 published).

=== Cause 4 of 7: PublishDataTransformationProfile's version-quota refusal is worded wrongly ===
Checks: healthlake.publish-profile-exceeds-version-quota-rejected
AWS: Smithy: PublishDataTransformationProfile errors ServiceQuotaExceededException (HTTP 400); ProfileVersion range 0..99 is the bound. Message text is recorded behavior only: the 100th publish is refused with ServiceQuotaExceededException and the message "Maximum of 99 published versions exceeded" — fixed text "Maximum of ", " published versions exceeded" with the limit as its variable part.
Emulator: correct error code, wrong message: 'The profile already holds the maximum number of published versions (99)'.
Why: a client that matches on the quota sentence (or surfaces it to a user) sees text real AWS never emits for this quota.
Fix: the quota refusal in PublishDataTransformationProfile's handler (its quota_exceeded() message for the version ceiling).
Traces: healthlake.publish-profile-exceeds-version-quota-rejected (99 publishes, then one more).

=== Cause 5 of 7: StartDataTransformationJob's profile-not-found message does not name the profile ===
Checks: healthlake.start-transformation-job-nonexistent-profile-not-found
AWS: Smithy: StartDataTransformationJob errors ResourceNotFoundException (HTTP 404); ProfileId required, pattern ^[a-f0-9]{32}$. Message text is recorded behavior only: AWS words this refusal as "Data transformation profile not found: <ProfileId>" — fixed text "Data transformation profile not found: " with the request's ProfileId appended.
Emulator: ResourceNotFoundException with the shared constant 'Could not find specified profile.' — this operation reuses the generic profile sentence rather than its own wording.
Why: the refusal does not identify the resource or the operation's own wording, so client-side error handling that distinguishes a missing profile from a missing job/datastore cannot key on it.
Fix: the profile lookup refusal in StartDataTransformationJob's handler (its not_found() call) — use this operation's wording with the requested ProfileId.
Traces: healthlake.start-transformation-job-nonexistent-profile-not-found (StartDataTransformationJob with ProfileId 00000000000000000000000000000000 and valid S3 locations).

=== Cause 6 of 7: StartDataTransformationJob accepts a DataAccessRoleArn the caller may not pass / that does not trust HealthLake ===
Checks: healthlake.start-transformation-job-untrusted-role-rejected
AWS: Smithy: StartDataTransformationJob models AccessDeniedException (HTTP 403) and DataAccessRoleArn is required (pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$). Recorded behavior: a role whose trust policy allows only an unrelated principal (ec2.amazonaws.com) instead of healthlake.amazonaws.com is refused with AccessDeniedException and the message "User: <caller-arn> is not authorized to perform: iam:PassRole on resource: <role-arn>" — fixed text "User: ", " is not authorized to perform: iam:PassRole on resource: ", with the caller's identity ARN and the passed role ARN as its variable parts. No job is created: ListDataTransformationJobs(JobName=...) returns no item for the rejected submission. A role that DOES trust healthlake.amazonaws.com is accepted.
Emulator: the submission SUCCEEDS with HTTP 200 and a JobId/JobStatus=SUBMITTED — the handler performs no role validation at all (it never consults the role's trust policy or the caller's identity), so a job record is minted for a role AWS refuses.
Why: this is an overly-permissive refusal gap. Code that is tested against the emulator will believe a mis-configured DataAccessRoleArn is fine and will only discover the AccessDeniedException in real AWS; negative tests for the PassRole/trust-policy boundary cannot be written at all.
Fix: a role-trust/PassRole gate in StartDataTransformationJob's handler validation, run before the job record is minted (it must read the role's trust policy from the IAM state and refuse with the modelled AccessDeniedException, leaving no job behind).
Traces: healthlake.start-transformation-job-untrusted-role-rejected (role trusting ec2.amazonaws.com, real S3 in/out buckets and a customer-managed KMS key).

=== Cause 7 of 7: the ProfileMapping constraint violation echoes a synthetic value token AWS omits ===
Checks: healthlake.update-profile-mapping-exceeds-entry-limit-rejected
AWS: Smithy: UpdateDataTransformationProfile.ProfileMapping is required, @sensitive, length 1..500; error ValidationException (HTTP 400). Recorded wording for a 501-entry mapping: "1 validation error detected: Value at 'profileMapping' failed to satisfy constraint: Member must have length less than or equal to 500" — the standard constraint sentence with NO rendered value, because the member's value is not echoed; the variable parts are the member path ('profileMapping') and the bound (500).
Emulator: correct code (ValidationException) but inserts a synthetic value token: "1 validation error detected: Value '501 entries' at 'profileMapping' failed to satisfy constraint: Member must have length less than or equal to 500".
Why: the message diverges from AWS's wording for every over-length map member on this service, and the invented "'501 entries'" rendering also leaks a shape detail about a @sensitive member that AWS deliberately does not render.
Fix: the constraint-violation message builder used by this service's boundary validation (the length-violation branch for map/sensitive members) — emit "Value at '<member>' ..." with no rendered value rather than fabricating a count token.
Traces: healthlake.update-profile-mapping-exceeds-entry-limit-rejected (UpdateDataTransformationProfile with a 501-entry ProfileMapping).

## Evidence
All verdicts from the behavioral replay (healthlake 10/20, 9 failed; StartFHIRImportJob-blocked checks excluded as already filed).

Cause 1 — healthlake.data-transformation-profile-draft-update-persists [error]:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetDataTransformationProfile operation: Could not find specified profile version.
  Expected: 200 with Version=0 and ProfileMapping == {"Resources/Patient.vm": "#set($x = 'sim2real-marker-updated')"} (the content the preceding update wrote), LastUpdatedAt not regressing.

Cause 2 — healthlake.delete-profile-removes-all-published-versions [mismatch]:
  AssertionError: expected message "Profile '<ProfileId>' not found", got 'Could not find specified profile.'
  (code ResourceNotFoundException was correct; only the message diverges)

Cause 3 — healthlake.get-profile-out-of-range-version-not-found [mismatch]:
  AssertionError: expected message "Profile '<ProfileId>' version 5 not found", got 'Could not find specified profile version.'

Cause 4 — healthlake.publish-profile-exceeds-version-quota-rejected [mismatch]:
  AssertionError: expected message 'Maximum of 99 published versions exceeded', got 'The profile already holds the maximum number of published versions (99)'

Cause 5 — healthlake.start-transformation-job-nonexistent-profile-not-found [mismatch]:
  AssertionError: expected message 'Data transformation profile not found: 00000000000000000000000000000000', got 'Could not find specified profile.'

Cause 6 — healthlake.start-transformation-job-untrusted-role-rejected [mismatch]:
  AssertionError: expected a rejection, but the call SUCCEEDED and returned {'JobId': '<generated>', 'JobStatus': 'SUBMITTED', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected: AccessDeniedException (HTTP 403), message "User: <caller-arn> is not authorized to perform: iam:PassRole on resource: <untrusted-role-arn>", and no job created under the submitted JobName.

Cause 7 — healthlake.update-profile-mapping-exceeds-entry-limit-rejected [mismatch]:
  AssertionError: expected message "1 validation error detected: Value at 'profileMapping' failed to satisfy constraint: Member must have length less than or equal to 500", got "1 validation error detected: Value '501 entries' at 'profileMapping' failed to satisfy constraint: Member must have length less than or equal to 500"

Code locations consulted (citations only):
  services/service-com-amazonaws-healthlake-healthlake/src/ops/get_data_transformation_profile.rs:191-197 (omitted version -> latest_published() only; both fixed not-found constants)
  services/service-com-amazonaws-healthlake-healthlake/src/state.rs:272 and :278 (PROFILE_NOT_FOUND_MESSAGE, PROFILE_VERSION_NOT_FOUND_MESSAGE — documented as deliberately fixed, never quoting the request)
  services/service-com-amazonaws-healthlake-healthlake/src/ops/publish_data_transformation_profile.rs:250-255 (version-quota message)
  services/service-com-amazonaws-healthlake-healthlake/src/ops/start_data_transformation_job.rs:239-241 (shared profile not-found sentence) and :195-311 (no DataAccessRoleArn trust/PassRole validation anywhere before the job is inserted)

## Steps to Reproduce
Boot the emulator and point a botocore healthlake client at it.

Cause 1: CreateDataTransformationProfile(SourceFormat=CCDA, ProfileName=<name>, Source={ProfileMapping:{ProfileMapping:{"Resources/Patient.vm":"a"}}}) -> UpdateDataTransformationProfile(ProfileId, ProfileMapping={"Resources/Patient.vm":"b"}) -> GetDataTransformationProfile(ProfileId) with NO ProfileVersion. Emulator 404s; AWS returns Version=0 with content "b".

Cause 2: create a profile, DeleteDataTransformationProfile(ProfileId), then GetDataTransformationProfile(ProfileId) and read Error.Message.

Cause 3: create a profile, PublishDataTransformationProfile(ProfileId, SourceFormat=CCDA), then GetDataTransformationProfile(ProfileId, ProfileVersion=5) and read Error.Message.

Cause 4: create a profile and PublishDataTransformationProfile 99 times, then publish once more; read Error.Code/Message.

Cause 5: StartDataTransformationJob(ProfileId="00000000000000000000000000000000", valid InputDataConfig/OutputDataConfig S3 URIs, DataAccessRoleArn, ClientToken, JobName); read Error.Message.

Cause 6: iam.CreateRole with a trust policy allowing only ec2.amazonaws.com; create+publish a profile; create real in/out S3 buckets and a KMS key; StartDataTransformationJob with that role ARN. Emulator returns 200/SUBMITTED; AWS returns AccessDeniedException and creates no job (verify with ListDataTransformationJobs(JobName=...)).

Cause 7: create a profile, then UpdateDataTransformationProfile(ProfileId, ProfileMapping={f"Resources/File{i}.vm": "x" for i in range(501)}); read Error.Message.
