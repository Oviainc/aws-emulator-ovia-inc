Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3files.access-point-lifecycle-creating-to-available-then-deleted s3files.put-synchronization-configuration-stale-version-conflict-rejected
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: s3files.access-point-lifecycle-creating-to-available-then-deleted|run#17|AssertionError|assert message == "Access point does not exist.", f"expected message 'Access point does not exist.', got {message!r}"
Issue-Key: s3files.put-synchronization-configuration-stale-version-conflict-rejected|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3files: two error-message fidelity defects — access-point not-found message is an internal diagnostic, and the stale-latestVersionNumber ConflictException message is request/state-derived instead of AWS's fixed sentence

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint, DeleteAccessPoint, PutSynchronizationConfiguration

## Description
Two distinct net-new root causes in the s3files emulator, both grounded in failing behavioral checks of this suite. Each must be closed before submitting.

SCOPE NOTE: the open issue '20261022T204614Z-s3files-six-control-plane-fidelity-defects-missing-nested.md' owns six other s3files causes, including (its Cause 5) the FILE-SYSTEM not-found message ('File system does not exist.' for an unknown fileSystemId, grounded in create-access-point-unknown-file-system-not-found / delete-file-system-transitions-to-deleted). Cause 1 below is a DIFFERENT condition and a different resolver — the ACCESS POINT not-found message, which that issue neither names nor carries a replay key for — and Cause 2 is a ConflictException message on PutSynchronizationConfiguration, unrelated to that issue's Cause 6 (which is the ValidationException list-rendering formatter). Neither cause is covered by that issue, and the suite did not mark either of these checks OWNED or ALREADY FILED.

--- Cause 1 of 2: ResourceNotFoundException for an unknown or already-deleted accessPointId carries an internal diagnostic message instead of the service's fixed sentence ---
AWS: Smithy: GetAccessPoint (GET /access-points/{accessPointId}) and DeleteAccessPoint (DELETE /access-points/{accessPointId}, @idempotent, success 204) both model ResourceNotFoundException (com.amazonaws.s3files#ResourceNotFoundException, errorKind client, httpError 404, doc 'The specified resource was not found. Verify that the resource exists and that you have permission to access it.'). Recorded behavior: for an accessPointId that does not exist — including one just removed by DeleteAccessPoint — the service returns 404 ResourceNotFoundException with a FIXED, condition-specific sentence that embeds NO value from the request: the message is exactly 'Access point does not exist.' The whole string is fixed text; there are no variable parts. The same constant sentence is returned by the subsequent GetAccessPoint and by a second DeleteAccessPoint of the same id, so the message is per-condition and id-independent (this mirrors the file-system condition's 'File system does not exist.').
Emulator: the error CODE and status are right (ResourceNotFoundException), but the message is a formatted internal diagnostic that embeds the requested id: 'no access point fsap-de9f6aaa8879281f9' (the 'fsap-...' part is the id from the request; 'no access point ' is the emulator's own prefix). Expected the fixed sentence 'Access point does not exist.'
Why: AWS's message is constant per condition, the emulator's varies with every request, so a client or fixture that branches on, logs, or normalizes the not-found reason sees a different string on every call and cannot match it; it also exposes internal phrasing on the API surface instead of the documented wording. The delete→get→re-delete idempotency flow (the standard way to confirm a resource is gone) therefore cannot be asserted against.
Fix: the not-found error construction in the access-point resolver used by this service's handlers (GetAccessPoint and DeleteAccessPoint, and any other operation resolving an accessPointId) must emit the fixed condition sentence instead of formatting the looked-up id — the same treatment the file-system resolver needs.
Traces: s3files.access-point-lifecycle-creating-to-available-then-deleted

--- Cause 2 of 2: the stale-latestVersionNumber ConflictException message is derived from the request and state instead of AWS's fixed remediation sentence ---
AWS: Smithy: PutSynchronizationConfigurationRequest.latestVersionNumber (Integer, optional) is documented as 'The version number of the current synchronization configuration. Omit this value when creating a synchronization configuration for the first time. For subsequent updates, provide this value for optimistic concurrency control. If the version number does not match the current configuration, the request fails with a ConflictException.' ConflictException is errorKind client, httpError 409. Recorded behavior: when latestVersionNumber does not match the configuration's current version, the service returns 409 ConflictException with a FIXED remediation sentence that embeds neither the submitted version nor the current one: 'Invalid latestVersionNumber. Retrieve the most recent latestVersionNumber and retry.' The entire string is fixed text; there are no variable parts taken from the request or state.
Emulator: the status/code are right (ConflictException), but the message is built from the submitted and stored version numbers and reads as an internal comparison: 'latestVersionNumber 1 does not match the current synchronization configuration version 2' (the '1' is the request's latestVersionNumber and the '2' is the emulator's stored version; neither belongs in AWS's message). Expected 'Invalid latestVersionNumber. Retrieve the most recent latestVersionNumber and retry.'
Why: AWS's message tells the caller the remediation (re-read the version and retry) in constant text a client can match; the emulator's varies with the state of every file system, so optimistic-concurrency retry logic keyed on the message cannot recognize the condition, and the message leaks internal version bookkeeping.
Fix: the optimistic-concurrency check in the PutSynchronizationConfiguration handler (its ConflictException construction) must emit the fixed remediation sentence rather than interpolating the submitted and current version numbers. The surrounding behavior is already correct — the conflict is detected, the configuration is left at the accepted version, and the identical body with the current version succeeds — only the message diverges.
Traces: s3files.put-synchronization-configuration-stale-version-conflict-rejected

## Evidence
Suite: simparity-behavioral-aws-v1 — s3files 9/24 passed, 15 failed. All other failures in this service are marked ALREADY FILED (unimplemented CreateMountTarget, ec2 CreateInternetGateway) or OWNED by 20261022T204614Z-s3files-six-control-plane-fidelity-defects-missing-nested.md; the two below are the uncovered remainder.

Cause 1 — s3files.access-point-lifecycle-creating-to-available-then-deleted [mismatch]
  check_s3files_access_point_lifecycle_creating_to_available_then_deleted.py line 55:
  assert message == "Access point does not exist."
  AssertionError: expected message 'Access point does not exist.', got 'no access point fsap-de9f6aaa8879281f9'
  Context: CreateAccessPoint -> poll to 'available' (these passed), DeleteAccessPoint, then GetAccessPoint on the deleted id. Error code was correct (ResourceNotFoundException); only the message diverged. The check then asserts the same fixed sentence from a second DeleteAccessPoint of the same id, which the same defect would also fail.
  Fixed text vs values: the expected message has NO variable parts — 'Access point does not exist.' is entirely fixed. The emulator's string takes 'fsap-de9f6aaa8879281f9' from the request's accessPointId (a per-run generated id, not a requirement) and prefixes its own 'no access point '.

Cause 2 — s3files.put-synchronization-configuration-stale-version-conflict-rejected [mismatch]
  check_s3files_put_synchronization_configuration_stale_version_conflict_rejected.py line 57:
  AssertionError: expected message 'Invalid latestVersionNumber. Retrieve the most recent latestVersionNumber and retry.', got 'latestVersionNumber 1 does not match the current synchronization configuration version 2'
  Context: the check first establishes a positive control (a PutSynchronizationConfiguration with the CURRENT latestVersionNumber is accepted and bumps the version), then replays a well-formed body against the now-stale version. Code was correct (ConflictException); only the message diverged.
  Fixed text vs values: the expected message is entirely fixed text. The emulator's embeds the request's latestVersionNumber (1) and the stored current version (2) — per-run state values, not requirements.

Model references used: smithy_query query=operation for GetAccessPoint, DeleteAccessPoint, PutSynchronizationConfiguration; query=shape for ConflictException, ResourceNotFoundException; query=doc for PutSynchronizationConfigurationRequest.latestVersionNumber — services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json.

## Steps to Reproduce
Boot the emulator and run the s3files behavioral checks. Both flows start from a versioned S3 bucket + an s3files IAM role + a file system polled to status 'available'.

Cause 1: ap = create_access_point(fileSystemId=<available fs>, posixUser={'uid':1000,'gid':1000}, rootDirectory={'path':'/'}); poll get_access_point(accessPointId=ap['accessPointId']) to status 'available'; delete_access_point(accessPointId=ap['accessPointId']); then get_access_point(accessPointId=<that same id>) -> observe 404 ResourceNotFoundException with message 'no access point <that id>'; expect the fixed sentence 'Access point does not exist.'. Then delete_access_point(accessPointId=<that same id>) a second time -> also expect 404 ResourceNotFoundException with the identical fixed sentence.

Cause 2: put_synchronization_configuration(fileSystemId=<fs>, importDataRules=[{'prefix':'','trigger':'ON_FILE_ACCESS','sizeLessThan':1048576}], expirationDataRules=[{'daysAfterLastAccess':30}]); v1 = get_synchronization_configuration(fileSystemId=<fs>)['latestVersionNumber']; put_synchronization_configuration(fileSystemId=<fs>, latestVersionNumber=v1, importDataRules=[{'prefix':'','trigger':'ON_DIRECTORY_FIRST_ACCESS','sizeLessThan':2097152}], expirationDataRules=[{'daysAfterLastAccess':60}]) -> accepted, version becomes v2. Now replay with the STALE version: put_synchronization_configuration(fileSystemId=<fs>, latestVersionNumber=v1, importDataRules=[{'prefix':'','trigger':'ON_FILE_ACCESS','sizeLessThan':4096}], expirationDataRules=[{'daysAfterLastAccess':90}]) -> observe 409 ConflictException with message 'latestVersionNumber <v1> does not match the current synchronization configuration version <v2>'; expect the fixed sentence 'Invalid latestVersionNumber. Retrieve the most recent latestVersionNumber and retry.'
