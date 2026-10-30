Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-grants-operations-require-instance-rejected
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-grants-operations-require-instance-rejected|run#5|AssertionError|assert code == "AccessGrantsInstanceNotExistsError", (
Issue-Commit: bd90440bd38277082eb102c17511325b543b3bd6

# s3control Access Grants read surface is scaffolded: ListAccessGrants answers 501 NotImplemented instead of AccessGrantsInstanceNotExistsError

> _Note: this issue was identified against the code base at commit `bd90440bd38277082eb102c17511325b543b3bd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccessGrants

## Description
Cause 1 of 1: the instance-scoped S3 Access Grants read/dissociate operations are still scaffold stubs, so they answer HTTP 501 `NotImplemented` for every request instead of the Access Grants "no instance yet" refusal. Failing check: s3control.access-grants-operations-require-instance-rejected.

AWS: Smithy model `com.amazonaws.s3control#ListAccessGrants` — GET /v20180820/accessgrantsinstance/grants, hostPrefix `{AccountId}.`, input `AccountId` (@required, @httpHeader x-amz-account-id, @hostLabel, pattern ^\d{12}$), success 200 with `AccessGrantsList`/`NextToken`. The model declares NO errors for this operation (`errors: []`), so the refusal codes are recorded behavior only: against real AWS, when the account has no S3 Access Grants instance, the instance-scoped reads ListAccessGrants, ListAccessGrantsLocations and ListCallerAccessGrants (and GetAccessGrantsInstance) are refused with error code `AccessGrantsInstanceNotExistsError` and message `Access Grants Instance does not exist`, and DissociateAccessGrantsIdentityCenter is refused with code `InvalidRequest` and message `The specified Access Grants Instance does not exist`. Both messages are entirely FIXED text — they interpolate no request value, no account id and no instance id. Once CreateAccessGrantsInstance has run, all four reads answer 200: GetAccessGrantsInstance reports the singleton `AccessGrantsInstanceId` `default`, and the three lists return empty lists on a fresh instance. A second CreateAccessGrantsInstance for the same account is refused with code `AccessGrantsInstanceAlreadyExists` and the fixed message `Access Grants Instance already exists`, leaving the existing instance untouched.

Emulator: GetAccessGrantsInstance already refuses correctly with AccessGrantsInstanceNotExistsError (the check's precondition and its first read both passed), but ListAccessGrants returns error code `NotImplemented` — the scaffold's HTTP 501 channel — so the check aborts there. By inspection of the checkout the same stub serves ListAccessGrantsLocations, ListCallerAccessGrants and DissociateAccessGrantsIdentityCenter, so the remainder of the Access Grants read surface diverges the same way and the check never reaches its positive-control and singleton-duplicate assertions.

Why: a client cannot distinguish "this account has no Access Grants instance" from "this API does not exist". SDKs and tools classify 501 NotImplemented as a non-retryable server fault rather than a modeled client refusal, so the standard bootstrap pattern — probe the instance-scoped reads, catch AccessGrantsInstanceNotExistsError, then CreateAccessGrantsInstance — aborts with a server error instead of provisioning. After an instance exists, the same calls still 501 rather than returning the (empty) grant and location lists, so nothing built on Access Grants enumeration can work at all.

Fix: implement the handlers in services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/list_access_grants.rs, .../list_access_grants_locations.rs, .../list_caller_access_grants.rs and .../dissociate_access_grants_identity_center.rs against the per-account Access Grants instance singleton in .../src/state.rs — reading the instance there and refusing with the recorded codes/messages above when it is absent — and remove each file's `SCAFFOLDED, NOT IMPLEMENTED` marker. The duplicate-create refusal belongs in the CreateAccessGrantsInstance handler.

Traces: s3control.access-grants-operations-require-instance-rejected (assertion at check_s3control_access_grants_operations_require_instance_rejected.py line 43: "ListAccessGrants with no instance returned 'NotImplemented', expected AccessGrantsInstanceNotExistsError").

## Evidence
Failing check s3control.access-grants-operations-require-instance-rejected [mismatch]:

  AssertionError: ListAccessGrants with no instance returned 'NotImplemented', expected AccessGrantsInstanceNotExistsError
  (check_s3control_access_grants_operations_require_instance_rejected.py, line 43)

Expected (real AWS, account with no Access Grants instance):
  ListAccessGrants        -> Error.Code = AccessGrantsInstanceNotExistsError, Error.Message = "Access Grants Instance does not exist"
  ListAccessGrantsLocations -> AccessGrantsInstanceNotExistsError, "Access Grants Instance does not exist"
  ListCallerAccessGrants  -> AccessGrantsInstanceNotExistsError, "Access Grants Instance does not exist"
  DissociateAccessGrantsIdentityCenter -> Error.Code = InvalidRequest, Error.Message = "The specified Access Grants Instance does not exist"
All four messages are fixed text with no interpolated request or state values.

Actual (emulator):
  ListAccessGrants -> HTTP 501, Error.Code = NotImplemented (scaffold stub channel; see the
  `SCAFFOLDED, NOT IMPLEMENTED` marker and the
  `Err(crate::scaffolded::Scaffolded::not_implemented("ListAccessGrants"))` at
  services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/list_access_grants.rs:182).
  The same stub body is present in list_access_grants_locations.rs, list_caller_access_grants.rs
  and dissociate_access_grants_identity_center.rs.

Control within the same check: GetAccessGrantsInstance DOES refuse with AccessGrantsInstanceNotExistsError, so only the listed operations are missing. Because the check aborts at ListAccessGrants, the post-create assertions (GetAccessGrantsInstance -> AccessGrantsInstanceId "default"; the three lists -> empty; duplicate CreateAccessGrantsInstance -> AccessGrantsInstanceAlreadyExists / "Access Grants Instance already exists" with the original instance intact) were never exercised and must be confirmed by the same replay.

## Steps to Reproduce
1. Use an account that has no S3 Access Grants instance (the instance is a per-account singleton).
2. Confirm the precondition: s3control.get_access_grants_instance(AccountId=<account>) is refused with code AccessGrantsInstanceNotExistsError.
3. Call s3control.list_access_grants(AccountId=<account>).
   Expected: ClientError with Error.Code AccessGrantsInstanceNotExistsError and Error.Message "Access Grants Instance does not exist".
   Actual: HTTP 501 with Error.Code NotImplemented.
4. Repeat with list_access_grants_locations(AccountId=<account>) and list_caller_access_grants(AccountId=<account>) — same 501 NotImplemented expected from the stubs.
5. Call dissociate_access_grants_identity_center(AccountId=<account>).
   Expected: Error.Code InvalidRequest, Error.Message "The specified Access Grants Instance does not exist".
6. Call create_access_grants_instance(AccountId=<account>), then re-run steps 3-4: each list must answer 200 with an empty list, and get_access_grants_instance must report AccessGrantsInstanceId "default".
7. Call create_access_grants_instance(AccountId=<account>) a second time.
   Expected: Error.Code AccessGrantsInstanceAlreadyExists, Error.Message "Access Grants Instance already exists", and get_access_grants_instance still reports "default".
