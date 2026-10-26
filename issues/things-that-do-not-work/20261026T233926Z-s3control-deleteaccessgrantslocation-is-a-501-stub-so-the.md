Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-grants-location-and-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-grants-location-and-resource-policy-lifecycle|run#26|AssertionError|assert code == "AccessGrantsLocationNotEmptyError", (
Issue-Commit: 697d84198565e6ff2c9331554fb021da14548e49

# s3control: DeleteAccessGrantsLocation is a 501 stub, so the Access Grants location lifecycle never refuses an in-use location with AccessGrantsLocationNotEmptyError

> _Note: this issue was identified against the code base at commit `697d84198565e6ff2c9331554fb021da14548e49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAccessGrantsLocation

## Description
Cause 1 of 1: `DeleteAccessGrantsLocation` is an unimplemented scaffold stub, so the S3 Access Grants location lifecycle has no delete arm at all and the referential-integrity refusal that guards it can never be observed. Failing check: s3control.access-grants-location-and-resource-policy-lifecycle.

AWS: the Smithy model binds `DeleteAccessGrantsLocation` to `DELETE /v20180820/accessgrantsinstance/location/{AccessGrantsLocationId}` with modelled success status 200, required `AccountId` (header `x-amz-account-id`, pattern `^\d{12}$`) and required `AccessGrantsLocationId` (httpLabel, len 1..64, pattern `^[a-zA-Z0-9\-]+$`), and an EMPTY modelled `errors` list — so every failure code for this operation is recorded behavior only, not modelled. The operation's own model documentation states the rule: "You can only delete a location registration from an S3 Access Grants instance if there are no grants associated with this location." Recorded behavior for that rule: while at least one access grant still references the location, the call is REFUSED with error code `AccessGrantsLocationNotEmptyError` and message `Please delete access grants before deleting access grants location`. That message is entirely fixed text — it interpolates no identifier, no location id and no count, so it is the requirement verbatim. The refusal must also be effect-free: a subsequent `GetAccessGrantsLocation` on the same id still returns that location. Once the referencing grant is deleted, the same `DeleteAccessGrantsLocation` call succeeds (200, empty body), and afterwards `GetAccessGrantsLocation` and `UpdateAccessGrantsLocation` on the dead id are both refused with code `NoSuchAccessGrantsLocationError` and the fixed message `The specified access grants location does not exist`.

Emulator: every `DeleteAccessGrantsLocation` request, in-use or not, answers HTTP 501 with error code `NotImplemented`. The handler services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/delete_access_grants_location.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteAccessGrantsLocation"))` — it never reads service state, so it neither consults the grants that reference the location nor removes the location. The replay asserts on the in-use arm first and fails there: "deleting an in-use location returned 'NotImplemented', expected AccessGrantsLocationNotEmptyError". Note this check is NOT covered by the stub-blocked issues the suite filed itself (those name DeleteAccessPointPolicy, UpdateJobStatus and DeleteStorageLensConfiguration), because the check catches the ClientError and the failure surfaced as a mismatch rather than an unhandled error. The preceding steps of the same check — CreateAccessGrantsLocation, GetAccessGrantsLocation (LocationScope and IAMRoleArn round-trip), UpdateAccessGrantsLocation (role swapped in place, location id not re-minted) and CreateAccessGrant — all pass, so the gap is specific to the delete arm.

Why: a client that manages Access Grants locations cannot deprovision one at all, and cannot distinguish "this location still holds grants" from "this service is broken". Teardown code that deletes a location as the last step of a cleanup gets a 501 instead of a 200 and leaks the registration; code that deliberately probes the refusal to decide whether it must enumerate and delete grants first sees `NotImplemented` and either aborts or, worse, treats the location as deleted. Because 501 is not a modelled error code, SDK retry and error-handling branches keyed on `AccessGrantsLocationNotEmptyError` and `NoSuchAccessGrantsLocationError` are dead code against the emulator, and the whole post-delete half of the lifecycle (the dead id refusing reads and mutations with `NoSuchAccessGrantsLocationError`) is unreachable and therefore unverified.

Fix: implement `handle` in the operation's handler, services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/delete_access_grants_location.rs, and delete the SCAFFOLDED marker line. It needs the service state to hold access grants keyed by the location they reference so the handler can test emptiness before mutating: refuse with the `AccessGrantsLocationNotEmptyError` code and its fixed message while any grant references the id (leaving state untouched), refuse an unknown or already-deleted id with `NoSuchAccessGrantsLocationError` and its fixed message, and otherwise remove the location record and answer the modelled 200. Since the model declares no errors for this operation, both refusals must come from the handler's error channel rather than the modelled enum. `GetAccessGrantsLocation` and `UpdateAccessGrantsLocation` must resolve against the same store so a deleted id is refused there too.

Traces: s3control.access-grants-location-and-resource-policy-lifecycle (mismatch; assertion at check_s3control_access_grants_location_and_resource_policy_lifecycle.py line 99).

## Evidence
Check: s3control.access-grants-location-and-resource-policy-lifecycle — verdict: mismatch.

Expected (AWS, per the operation's model documentation plus recorded behavior; the operation models no errors):
  - DeleteAccessGrantsLocation while a grant references the location -> refused, error code `AccessGrantsLocationNotEmptyError`, message `Please delete access grants before deleting access grants location` (fixed text, no interpolated values).
  - The refusal is effect-free: GetAccessGrantsLocation still returns the same AccessGrantsLocationId.
  - After DeleteAccessGrant, DeleteAccessGrantsLocation succeeds with the modelled 200 and an empty body.
  - GetAccessGrantsLocation on the deleted id -> `NoSuchAccessGrantsLocationError` / `The specified access grants location does not exist` (fixed text).
  - UpdateAccessGrantsLocation on the deleted id -> `NoSuchAccessGrantsLocationError` / `The specified access grants location does not exist` (fixed text).

Actual (emulator):
  - HTTP 501, error code `NotImplemented`, for every DeleteAccessGrantsLocation request regardless of whether grants reference the location.
  - Replay assertion: `AssertionError: deleting an in-use location returned 'NotImplemented', expected AccessGrantsLocationNotEmptyError` (check_s3control_access_grants_location_and_resource_policy_lifecycle.py line 99).
  - Source confirmation: services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/delete_access_grants_location.rs line 4 still declares `SCAFFOLDED, NOT IMPLEMENTED`, and line 175 returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteAccessGrantsLocation"))`.

Model citation: Smithy s3control DeleteAccessGrantsLocation — http DELETE /v20180820/accessgrantsinstance/location/{AccessGrantsLocationId}, code 200; errors: [] (none modelled); input AccountId (required, header x-amz-account-id, pattern ^\d{12}$) and AccessGrantsLocationId (required, httpLabel, len 1..64, pattern ^[a-zA-Z0-9\-]+$); doc: "You can only delete a location registration from an S3 Access Grants instance if there are no grants associated with this location."

Not yet exercised (blocked behind this stub, so not claimed as separate defects): the Put/Get/Delete round-trip of the Access Grants instance resource policy and its `AccessGrantsInstanceResourcePolicyNotExists` / `Access Grants Instance Resource Policy does not exist` refusal, and the `NoSuchAccessGrantError` / `The specified access grant does not exist` refusal after DeleteAccessGrant. They become observable once the delete arm answers.

## Steps to Reproduce
1. Create an S3 general-purpose bucket and two IAM roles trusting access-grants.s3.amazonaws.com with s3:GetObject/s3:ListBucket on that bucket.
2. Ensure an S3 Access Grants instance exists for the account (GetAccessGrantsInstance, else CreateAccessGrantsInstance).
3. s3control CreateAccessGrantsLocation with LocationScope=s3://<bucket>/data/ and IAMRoleArn=<role1 arn>; capture AccessGrantsLocationId.
4. s3control CreateAccessGrant against that AccessGrantsLocationId with AccessGrantsLocationConfiguration={S3SubPrefix: "*"}, an IAM grantee, Permission=READ.
5. Call s3control DeleteAccessGrantsLocation with that AccountId and AccessGrantsLocationId while the grant from step 4 still exists.
   Expected: refusal with error code AccessGrantsLocationNotEmptyError and message "Please delete access grants before deleting access grants location"; GetAccessGrantsLocation afterwards still returns the location.
   Observed: HTTP 501 with error code NotImplemented.
6. Delete the grant (DeleteAccessGrant) and call DeleteAccessGrantsLocation again.
   Expected: 200 with an empty body; GetAccessGrantsLocation and UpdateAccessGrantsLocation on that id then refuse with NoSuchAccessGrantsLocationError / "The specified access grants location does not exist".
   Observed: HTTP 501 NotImplemented again.

Replay: run_behavioral_suite and inspect check s3control.access-grants-location-and-resource-policy-lifecycle.
