Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-grants-get-data-access-ungranted-prefix-rejected
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-grants-get-data-access-ungranted-prefix-rejected|_retry_on_propagation#14|Timeout|time.sleep(3)
Issue-Commit: 5b50be90bb844e905a41063b16fb5454d8305c48

# s3control S3 Access Grants data plane unimplemented: CreateAccessGrant and GetDataAccess answer 501, so no grant-scope authorization exists

> _Note: this issue was identified against the code base at commit `5b50be90bb844e905a41063b16fb5454d8305c48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataAccess

## Description
The S3 Access Grants grant/data-plane chain in s3control is not implemented, so an Access Grants client can never create a grant nor have a target evaluated against it. 2 causes.

Note on neighbouring issues: the open issue `20261024T005340Z-s3control-createaccessgrantslocation-stub.md` covers `CreateAccessGrantsLocation` only, and carries the replay keys `s3control.access-grants-get-data-access-returns-scoped-credentials` and `s3control.access-grants-location-and-resource-policy-lifecycle`. It does NOT carry the check this issue is grounded in, and it does not name either operation below, so neither cause is covered by it.

Cause 1 of 2 — `CreateAccessGrant` is an unimplemented scaffold.
AWS: Smithy `com.amazonaws.s3control#CreateAccessGrant`, POST /v20180820/accessgrantsinstance/grant, success status 200; required input `AccountId` (header x-amz-account-id, pattern ^\d{12}$), `AccessGrantsLocationId`, `Grantee`, `Permission` (READ|READWRITE|WRITE); optional `AccessGrantsLocationConfiguration` (its `S3SubPrefix`). Output returns `AccessGrantId`, `AccessGrantArn`, `GrantScope` (the model: "the result of appending the Subprefix to the location scope"), `Grantee`, `Permission`. The model declares no errors for the operation, so a well-formed create is expected to succeed with 200.
Emulator: the handler is still the generated stub and answers HTTP 501 `NotImplemented` with message "operation CreateAccessGrant is scaffolded but not implemented in this emulator", persisting nothing. No grant record is ever created, so `GrantScope` — the value the whole authorization decision keys off — does not exist in state.
Why: a client that registers a location and then grants a grantee READ on a sub-prefix gets a 501 instead of a grant id/ARN. Nothing downstream (GetDataAccess, ListAccessGrants, DeleteAccessGrant) can be exercised, and a client that treats 501 as retryable spins until its own deadline rather than failing fast.
Fix: implement the operation's handler and give the service state an access-grant store keyed by grants-location id, recording the grantee, permission and the computed grant scope (location scope + S3SubPrefix); emit the modeled 200 output shape.
Traces: `s3control.access-grants-get-data-access-ungranted-prefix-rejected`. Scaffold marker and the 501 return are at services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/create_access_grant.rs:4 and :215.

Cause 2 of 2 — `GetDataAccess` is an unimplemented scaffold, so there is no grant-scope policy evaluation.
AWS: Smithy `com.amazonaws.s3control#GetDataAccess`, GET /v20180820/accessgrantsinstance/dataaccess, success status 200; required `AccountId`, `Target` (query `target`, S3 URI), `Permission`; optional `DurationSeconds` (range 900..43200), `Privilege`, `TargetType`. Output carries `Credentials` (sensitive), `MatchedGrantTarget` and `Grantee`. The model declares no errors for this operation, so the refusal class is recorded behavior only: for a target that no grant covers, AWS refuses with error code `AccessDenied` (HTTP 403) and vends no credentials, while a target inside a grant's scope under the same location, instance and caller returns 200 with `Credentials.AccessKeyId` populated. The refusal message is built from the request — permission plus the requested prefix, e.g. "You do not have READ permissions to the requested S3 Prefix: s3://<bucket>/team-b/*" — so that construction rule is the requirement, not the recorded text.
Emulator: the handler is the generated stub and answers HTTP 501 `NotImplemented` ("operation GetDataAccess is scaffolded but not implemented in this emulator") for every target. Both arms are therefore wrong: the granted prefix yields no credentials, and the ungranted sibling prefix yields 501 rather than 403 `AccessDenied`. There is no code path that compares `Target` against any grant's scope, so an implementation that keyed off the registered location instead of the grant scope would be indistinguishable here.
Why: Access Grants exists to vend scoped, short-lived credentials; a client gets neither the credentials for data it is granted nor the authorization refusal for data it is not. A caller that treats absence of an error as permission would proceed as if the ungranted prefix were allowed, and a caller retrying the 501 as a transient/propagation error never terminates — observed here as the check exhausting its 60 s budget inside its propagation-retry loop instead of reaching an answer.
Fix: implement the operation's handler plus the authorizer it needs — resolve the Access Grants instance for the account, select the most specific grant whose scope covers `Target` for the requested `Permission`, and on a match return `Credentials` (honouring `DurationSeconds`, default 1 hour), `MatchedGrantTarget` and `Grantee`; on no match return the 403 `AccessDenied` refusal, built from the requested permission and target, with no credentials in the body.
Traces: `s3control.access-grants-get-data-access-ungranted-prefix-rejected`. Scaffold marker and the 501 return are at services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/get_data_access.rs:4 and :185. The modeled contract is reproduced in that file's header, lines 42-101.

## Evidence
Replay of `s3control.access-grants-get-data-access-ungranted-prefix-rejected` (verdict: mismatch) reported `Check timed out after 60.0s`. The check never reaches its assertions: it wraps the Access Grants setup and the positive GetDataAccess call in a retry loop that re-tries any ClientError against a 60 s propagation deadline, so the emulator's HTTP 501 `NotImplemented` responses are retried until the harness budget expires rather than surfacing once.

The refusals behind that timeout are in the checkout: both handlers still carry the generated marker `//! SCAFFOLDED, NOT IMPLEMENTED:` and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` —
  services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/create_access_grant.rs (marker line 4, 501 return line 215)
  services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/get_data_access.rs (marker line 4, 501 return line 185)
The same suite shows the sibling 501 text verbatim for this service's Access Grants ops, e.g. "An error occurred (NotImplemented) when calling the CreateAccessGrantsLocation operation: operation CreateAccessGrantsLocation is scaffolded but not implemented in this emulator".

Expected (AWS), per the check's two arms against one instance/location/caller where only the target differs:
  CreateAccessGrant(AccessGrantsLocationId=<loc>, AccessGrantsLocationConfiguration={S3SubPrefix: "team-a/*"}, Grantee={IAM, <caller>}, Permission=READ) -> 200 with AccessGrantId/AccessGrantArn/GrantScope
  GetDataAccess(Target="s3://<bucket>/team-a/*", Permission=READ) -> 200, Credentials.AccessKeyId present
  GetDataAccess(Target="s3://<bucket>/team-b/*", Permission=READ) -> 403, Error.Code == "AccessDenied", no credentials, message naming the permission and the requested prefix (example: "You do not have READ permissions to the requested S3 Prefix: s3://<bucket>/team-b/*")
Actual (emulator):
  CreateAccessGrant -> 501 NotImplemented, nothing persisted
  GetDataAccess (both the granted and the ungranted target) -> 501 NotImplemented, no credentials and no AccessDenied

Model basis (smithy_query on services/service-com-amazonaws-s3control-awss3controlservicev20180820/smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json): GetDataAccess GET /v20180820/accessgrantsinstance/dataaccess code 200, required AccountId/Target/Permission, output Credentials+MatchedGrantTarget+Grantee, errors []; CreateAccessGrant POST /v20180820/accessgrantsinstance/grant code 200, required AccountId/AccessGrantsLocationId/Grantee/Permission, errors [].

## Steps to Reproduce
1. Boot the s3control emulator from this checkout.
2. Ensure an Access Grants instance exists for the account: GetAccessGrantsInstance, and CreateAccessGrantsInstance if it answers AccessGrantsInstanceNotExistsError.
3. Create a bucket and put two objects under sibling prefixes, e.g. keys `team-a/file.txt` and `team-b/file.txt`.
4. Register a grants location spanning the whole bucket (CreateAccessGrantsLocation with LocationScope=s3://<bucket>/ and an IAM role trusting access-grants.s3.amazonaws.com). This currently answers 501 and is tracked separately by 20261024T005340Z-s3control-createaccessgrantslocation-stub.md.
5. Call CreateAccessGrant for that location with AccessGrantsLocationConfiguration={"S3SubPrefix": "team-a/*"}, Grantee={GranteeType: IAM, GranteeIdentifier: <caller arn>}, Permission=READ. Observe HTTP 501 NotImplemented instead of a 200 carrying AccessGrantId/AccessGrantArn/GrantScope (cause 1).
6. Call GetDataAccess(Target="s3://<bucket>/team-a/*", Permission=READ). Observe HTTP 501 instead of 200 with Credentials.AccessKeyId (cause 2, positive arm).
7. Call GetDataAccess(Target="s3://<bucket>/team-b/*", Permission=READ) — a prefix under the same location that no grant covers. Observe HTTP 501 instead of 403 with Error.Code "AccessDenied" and no credentials (cause 2, refusal arm).
8. Replay `s3control.access-grants-get-data-access-ungranted-prefix-rejected`; it currently exhausts its 60 s budget because it retries these 501s as propagation errors.
