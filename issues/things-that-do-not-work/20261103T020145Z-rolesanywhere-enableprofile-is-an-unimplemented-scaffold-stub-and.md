Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rolesanywhere.profile-delete-removes-resource
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere.profile-delete-removes-resource|run#16|AssertionError|assert mutation_error["Code"] == "ResourceNotFoundException", (
Issue-Commit: 085aca81986f06b9d958c778a1487573b77f6255

# rolesanywhere EnableProfile is an unimplemented scaffold stub and answers 501 NotImplemented instead of enabling the profile or refusing a deleted id with ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `085aca81986f06b9d958c778a1487573b77f6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableProfile

## Description
Cause 1 of 1: `EnableProfile` (POST /profile/{profileId}/enable) is not served at all — it is still a scaffold stub, so every call, valid or not, is answered with the scaffold's 501 `NotImplemented` error. Failing check ids: rolesanywhere.profile-delete-removes-resource.

AWS: Smithy (services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json) models `EnableProfile` with http {method: POST, uri: /profile/{profileId}/enable, code: 200}, input shape `ScalarProfileRequest` (`profileId`: Uuid, @required, @httpLabel, @length 36..36, @pattern `[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}`), output shape `ProfileDetailResponse` (`profile`: ProfileDetail), and exactly two modeled errors: `AccessDeniedException` (HTTP 403) and `ResourceNotFoundException` (HTTP 404, documented "The resource could not be found."). For a well-formed `profileId` that no longer exists (here: one whose profile was just deleted with DeleteProfile), AWS refuses with HTTP 404 and error code `ResourceNotFoundException`. Its message follows the same not-found template this service uses on reads: fixed text `The item doesn't exist. arn: ` + ` ; aborting.`, with the variable part being the resource ARN built from the request and emulator state — `arn:aws:rolesanywhere:<region>:<accountId>:profile/<profileId>` (region and account from the signing context/state, profileId from the request path). `NotImplemented` is not a modeled error for this operation, and 501 is not a status it can return.

Emulator: the handler body in services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_profile.rs returns `Err(crate::scaffolded::Scaffolded::not_implemented("EnableProfile"))` unconditionally, so the replay saw error code `'NotImplemented'` (the scaffold's 501) where `ResourceNotFoundException`/404 was required. Because the stub short-circuits before any state access, the happy path is equally unserved: an existing disabled profile is never re-enabled and no `profile` (ProfileDetail) is ever returned.

Why: a client cannot enable a profile at all, so no profile disabled by DisableProfile (or created disabled) can be returned to service, and the RolesAnywhere enable/disable lifecycle is unusable. Worse for error handling: botocore surfaces `NotImplemented` rather than `ResourceNotFoundException`, so SDK retry/except branches keyed on the modeled not-found error never fire, and 501 is classed as a server-side fault — callers retry a call that can never succeed instead of treating the profile as gone.

Fix: implement the operation's handler in services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_profile.rs — look the profile up in the service state keyed by `profileId`, set its enabled flag true, return `ProfileDetailResponse { profile }` with 200 and the same ProfileDetail representation GetProfile/CreateProfile serve, and when the id is absent from state return the modeled `ResourceNotFoundException` (404) with the service's not-found message wording described above; then delete the SCAFFOLDED marker comment at the top of that file. This is a distinct operation from the already-open EnableTrustAnchor stub issue (20261101T125141Z-rolesanywhere-enabletrustanchor-is-an-unimplemented-scaffold-stub-and.md) and from the DisableProfile stub issue (20261029T195344Z-rolesanywhere-disableprofile-stub.md); neither names EnableProfile.

Traces: rolesanywhere.profile-delete-removes-resource — after CreateProfile/GetProfile/ListProfiles/DeleteProfile all behaved, the check failed at `assert mutation_error["Code"] == "ResourceNotFoundException"` with "expected ResourceNotFoundException from EnableProfile on a deleted id, got 'NotImplemented'". The same check also asserts the EnableProfile not-found Message equals the ARN-bearing template above, which cannot be reached while the stub answers 501.

## Evidence
Check rolesanywhere.profile-delete-removes-resource, line 47 of check_rolesanywhere_profile_delete_removes_resource.py:
  AssertionError: expected ResourceNotFoundException from EnableProfile on a deleted id, got 'NotImplemented'

Expected (AWS): HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "The item doesn't exist. arn: arn:aws:rolesanywhere:<region>:<accountId>:profile/<profileId> ; aborting." (fixed text "The item doesn't exist. arn: " ... " ; aborting."; the ARN's region/account come from state and the profileId from the request path).
Actual (emulator): HTTP 501, Error.Code = "NotImplemented" — the scaffold error, emitted unconditionally by `Err(crate::scaffolded::Scaffolded::not_implemented("EnableProfile"))` in services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_profile.rs:154, whose file header still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4).

For the happy path the same stub means: expected 200 with `{"profile": ProfileDetail}` (enabled=true), actual 501 NotImplemented.

## Steps to Reproduce
1. CreateProfile with a name, one roleArns entry and one tag; note the returned profileId.
2. GetProfile(profileId) and ListProfiles — both show the profile (these pass today).
3. DeleteProfile(profileId).
4. Call EnableProfile(profileId=<deleted id>). AWS answers HTTP 404 ResourceNotFoundException with the "The item doesn't exist. arn: arn:aws:rolesanywhere:<region>:<account>:profile/<profileId> ; aborting." message; the emulator answers HTTP 501 with Error.Code "NotImplemented".
5. Also call EnableProfile on a LIVE profileId from step 1 (before deleting): AWS returns 200 with the ProfileDetail (enabled true); the emulator again answers 501 NotImplemented.
