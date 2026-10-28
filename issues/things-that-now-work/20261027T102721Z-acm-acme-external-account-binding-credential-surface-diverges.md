Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.get-acme-external-account-binding-credentials-cross-account-denied acm.get-eab-credentials-cross-account-denied
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.get-acme-external-account-binding-credentials-cross-account-denied|run#16|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code} message={message!r}"
Issue-Key: acm.get-eab-credentials-cross-account-denied|_error#16|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Commit: c048498fa9bf8a008294e81bd378b5619adc830b

# ACM ACME external-account-binding credential surface diverges in three places: cross-account ARN answered ResourceNotFoundException, credentials served to an unauthorized principal, and newNonce emits a lower-cased Replay-Nonce field name (3 causes, 3 checks; the newNonce cause is split out and still open)

## Resolution

Causes 1 and 2 are FIXED, and their two checks pass. Cause 3 is NOT fixed: the field
name it is about is decided below every service crate, in the read-only `vendor/` tree,
so it has been SPLIT OUT into its own open report rather than counted as resolved here —
and the Issue-Key and trace id it owns moved with it, out of this record's header.

* Cause 1 — an ARN input member that names the resource an operation acts on is held to
  the caller's own account: an ARN whose account segment is another account's is refused
  as invalid input (`ValidationException`, HTTP 400, the wholly fixed sentence `Invalid
  ARN in request: Same account ARN expected but got cross-account ARN`) before any store
  lookup, so the answer never depends on whether a record with that id exists, and only
  an ARN of the caller's own account can be reported as missing. The crate already
  implemented that rule for eight operations
  (`state::arn_is_in_caller_account` + `service::cross_account_arn_refusal`); the gate
  was added to the seven that lacked it (`GetAcmeExternalAccountBindingCredentials`,
  `DescribeAcmeExternalAccountBinding`, `RevokeAcmeExternalAccountBinding`,
  `DescribeAcmeEndpoint`, `UpdateAcmeEndpoint`, `DescribeAcmeDomainValidation`,
  `UpdateAcmeDomainValidation`) and the one wording now carries the recorded sentence for
  all of them.
* Cause 2 — every implemented operation whose model declares `AccessDeniedException`
  now evaluates the IAM identity-based policies in force on the principal the request's
  credentials name, against `acm:<OperationName>` on the ARN the request addressed,
  BEFORE it reads or writes anything, and refuses with the modelled
  `AccessDeniedException` (HTTP 403) naming the principal, the action and that resource
  (`services/service-com-amazonaws-acm-certificatemanager/src/service/authorization.rs`,
  deciding through `service_common::iam_authorization`). The account's own published
  credential, and a principal carrying a grant this emulator cannot read, are served.
  This also satisfies the check carried by
  `20261023T004835Z-acm-serves-calls-from-a-principal-with-no.md`, which stays filed for
  its own keys.
* Cause 3 — NOT FIXED, and SPLIT OUT so it stays filed: its Issue-Key
  (`acm.update-acme-endpoint-on-revoked-account-succeeds`) now has its own open report,
  `issues/things-that-do-not-work/20261027T102721Z-acm-acme-newnonce-emits-a-lower-cased-replay-nonce-field-name.md`.
  The `newNonce` responder already states the field under its canonical `Replay-Nonce`
  spelling, but every field name is normalised to lower case after that by code no fixer
  may change — `aws_smithy_runtime_api::http::Headers` (an `http::HeaderName`, lower-cased
  on construction), `vendor/smithy-server-runtime/src/multi_service_router.rs:411-430`
  (which rebuilds the outgoing response name by name through `http::HeaderName` again) and
  the HTTP/1 writer reached at `vendor/server/src/lib.rs:1205`. Nothing inside `services/`
  reaches those bytes, and no test in the service crate can even observe the casing, so
  the answer is left exactly as it was (200 to `HEAD`, 204 to `GET`, a fresh nonce,
  `Cache-Control: no-store` and the `Link` index) rather than substituting some other
  status or error, and that one check still fails — against the split-out report, not
  this one.

> _Note: this issue was identified against the code base at commit `c048498fa9bf8a008294e81bd378b5619adc830b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAcmeExternalAccountBindingCredentials

## Description
THREE distinct root causes in the ACM ACME surface, each grounded in one failing behavioral check of this suite that no open issue carries as a replay key. All three sit on the ACME external-account-binding path and all three reach a handler that is otherwise implemented (the positive legs pass: CreateAcmeEndpoint reaches ACTIVE, CreateAcmeExternalAccountBinding succeeds, GetAcmeExternalAccountBindingCredentials returns a non-empty KeyId/MacKey for the owner, the ACME directory is served 200), so none of them is a stub. A fixer must close all three before submitting.

Cause 1 of 3: GetAcmeExternalAccountBindingCredentials does not reject a cross-account ARN at input validation; it treats the foreign ARN as an unknown key and reports it as missing.
AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) models GetAcmeExternalAccountBindingCredentials as @readonly, input AcmeExternalAccountBindingArn required @length 1..200 @pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-external-account-binding/[a-zA-Z0-9-]+$, output {KeyId, MacKey (@sensitive)}, errors AccessDeniedException(403) / InternalServerException(500) / ResourceNotFoundException(400) / ThrottlingException(400) / ValidationException(400, awsQueryError ValidationError). An ARN whose account segment is NOT the caller's account is a malformed-input condition, not a lookup miss: AWS answers HTTP 400 with Error.Code 'ValidationException' and the message 'Invalid ARN in request: Same account ARN expected but got cross-account ARN'. That message is entirely FIXED text for this error class — it carries no value from the request or from state; the only request-derived part of the contract is which ARN member was judged. The ARN is rejected before any store lookup, so the answer must not depend on whether a record with that id happens to exist.
Emulator: the same call with the owner's own binding ARN rewritten to a foreign 12-digit account answers ResourceNotFoundException with message 'Could not find ACME external account binding arn:aws:acm:us-west-2:<account>:acme-endpoint/<endpoint-id>/acme-external-account-binding/<binding-id>.' (the account id, endpoint id and binding id there are per-run values from the request, the rest is the emulator's fixed not-found text). The handler's only key is the ARN string: services/service-com-amazonaws-acm-certificatemanager/src/ops/get_acme_external_account_binding_credentials.rs looks input.acme_external_account_binding_arn up in store.acme_external_account_bindings and falls straight into the ResourceNotFoundException arm, so the ARN's account segment is never compared against the caller's account at all.
Why: a client that branches on ValidationException to tell 'you passed an ARN from another account' apart from ResourceNotFoundException's 'this binding is gone' sees the wrong branch — a cross-account configuration bug is misreported as a deleted resource, and retry/backoff logic written for not-found behaves wrongly. It also leaks a different answer shape than AWS for ARN probing: AWS refuses the ARN on its face, the emulator refuses it only because nothing matched, so an attacker who guessed a real foreign binding id would get a different answer here than on AWS.
Fix: the ARN validation that precedes the store lookup in GetAcmeExternalAccountBindingCredentials' handler (compare the ARN's account segment to the request's own account before any map lookup and raise the modeled ValidationException), not the not-found path. Same gate belongs on the sibling ACME binding operations that take the same ARN.
Traces: acm.get-acme-external-account-binding-credentials-cross-account-denied.

Cause 2 of 3: GetAcmeExternalAccountBindingCredentials performs no identity-based authorization — it hands the binding's KeyId and MAC key to a principal with no IAM policies.
AWS: Smithy models AccessDeniedException (HTTP 403, awsQueryError AccessDenied) on this operation, and MacKey is marked @sensitive precisely because it is a credential: an ACME client signs its externalAccountBinding JWS with it. A session for a role that has no attached policies hits IAM's implicit deny, so the call is refused with HTTP 403 Error.Code 'AccessDeniedException' and a message of the standard form 'User: <assumed-role ARN of the calling session> is not authorized to perform: acm:GetAcmeExternalAccountBindingCredentials because no identity-based policy allows the acm:GetAcmeExternalAccountBindingCredentials action' — fixed text apart from the principal ARN (taken from the signing session) and the action name (taken from the request's operation). The binding owner's own credentials keep working unchanged.
Emulator: the identical call, signed with credentials from sts:AssumeRole on a freshly created role with NO policies attached, SUCCEEDS: HTTP 200 content-type application/x-amz-json-1.1 returning {'KeyId': <uuid>, 'MacKey': <base64url MAC key>} (those two values are per-run mints, not part of the contract). The handler in get_acme_external_account_binding_credentials.rs contains no authorization step at all — it goes store lookup, revoked check, return — so every signed request is served as if it were the account's root principal.
Why: overly permissive, and on a credential-bearing read. Any principal that can reach the emulator can extract the MAC key that authorizes ACME account registration against that endpoint, so the whole external-account-binding security boundary is unobservable. A deployment whose role lacks acm:GetAcmeExternalAccountBindingCredentials passes here and is denied in production, and least-privilege policy work cannot be validated at all. Related open filing: 20261023T004835Z-acm-serves-calls-from-a-principal-with-no.md reports the same missing authorization layer for GetAccountConfiguration/PutAccountConfiguration and carries only those two checks as replay keys, so it does not gate this operation; a fixer implementing the shared layer must satisfy this check too.
Fix: the request-authorization layer the ACM handlers are dispatched through (evaluate the signing principal's IAM identity-based policies before dispatch and raise the modeled AccessDeniedException); get_acme_external_account_binding_credentials.rs only needs to surface the denial.
Traces: acm.get-eab-credentials-cross-account-denied.

Cause 3 of 3: the ACME newNonce resource emits its nonce under the lower-cased field name 'replay-nonce', not the 'Replay-Nonce' field name RFC 8555 and AWS's ACME endpoints put on the wire, so a client that matches the field name as written is never handed a nonce and cannot make a single JWS request.
AWS: recorded behavior only (the ACME data plane is not a modeled Smithy operation). RFC 8555 section 7.2 spells the field 'Replay-Nonce' and requires it on both the 200 answer to HEAD <newNonce> and the 204 answer to GET <newNonce>; AWS's ACM ACME endpoints serve it under that exact spelling, which is why real ACME clients read it back by that name. The emulator's own source documents the same contract (services/service-com-amazonaws-acm-certificatemanager/src/service/acme_http.rs module docs: 'HEAD <base>/new-nonce -> 200, GET <base>/new-nonce -> 204 — a fresh anti-replay nonce in the Replay-Nonce header').
Emulator: the nonce is inserted as the literal lower-case name (acme_http.rs, fn nonce: answer.headers_mut().insert("replay-nonce", mint_nonce())), and that lower-cased spelling is what goes out on the HTTP/1.1 wire. A client that reads the response's field names as received and looks up 'Replay-Nonce' finds nothing on the HEAD 200 and nothing on the GET fallback, which answers HTTP 204 with no field under that name: 'AssertionError: no Replay-Nonce returned from newNonce (status=204)'. The statuses themselves (200 for HEAD, 204 for GET) are correct; only the field name diverges. This is distinguishable from the already-filed JWS gap (20261027T001842Z-acm-acme-data-plane-newaccount-answers-http-501.md, whose evidence records 'HEAD newNonce (Replay-Nonce present)' because the checks behind it normalise field names to lower case before reading them) — this check never reaches a POST at all, so that issue's fix does not close it.
Why: an ACME client (certbot, lego, acme.sh) obtains its first nonce from newNonce and cannot send any JWS without it. Against AWS the client gets a nonce and proceeds; against the emulator it gets an empty value and wedges before the first POST, so every downstream ACME contract is unreachable for reasons unrelated to the actual implementation gap. The requirement is the canonical field name 'Replay-Nonce' (fixed text) carrying a freshly minted nonce value (per-request, not part of the contract) on both the HEAD 200 and the GET 204 — alongside the Cache-Control: no-store and Link index fields the same function already sets.
Fix: the newNonce responder in services/service-com-amazonaws-acm-certificatemanager/src/service/acme_http.rs (fn nonce) — emit the field under its canonical 'Replay-Nonce' spelling for both the HEAD and GET arms; if the response-building seam normalises field names, that normalisation is what must preserve the canonical casing for this surface.
Traces: acm.update-acme-endpoint-on-revoked-account-succeeds.

## Evidence
Cause 1 — acm.get-acme-external-account-binding-credentials-cross-account-denied [mismatch]:
  AssertionError: expected ValidationException, got ResourceNotFoundException message='Could not find ACME external account binding arn:aws:acm:us-west-2:111122223333:acme-endpoint/0c2b6d6a-e03d-4eb4-871d-cd77f4dbc700/acme-external-account-binding/64e86502-9978-4728-a3b9-a0415787fcb7.'
  Required: HTTP 400, Error.Code 'ValidationException', message 'Invalid ARN in request: Same account ARN expected but got cross-account ARN' (wholly fixed text for this class). Observed: Error.Code 'ValidationException' absent; ResourceNotFoundException whose message's ids are per-run values of the probe ARN. Everything upstream passed: the owner's baseline GetAcmeExternalAccountBindingCredentials returned a non-empty KeyId and MacKey, and the check's foreign ARN is the owner's own ARN with only the 12-digit account segment swapped.
  Smithy citations: GetAcmeExternalAccountBindingCredentials @readonly, AcmeExternalAccountBindingArn required @length 1..200 @pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-external-account-binding/[a-zA-Z0-9-]+$; errors AccessDeniedException(403)/InternalServerException(500)/ResourceNotFoundException(400)/ThrottlingException(400)/ValidationException(400).

Cause 2 — acm.get-eab-credentials-cross-account-denied [mismatch]:
  AssertionError: expected a rejection, but the call succeeded and returned {'KeyId': '3666d7fe-b49e-44c1-930e-d2317f69dc20', 'MacKey': 'ToXyllIaSAH8HRhrmxY7OPxrrXfBrpnoGJtHTePSECST', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '104', ...}}}
  The denying client is built by the check itself: iam.create_role(RoleName='sim2real-acme-noperm-role-<suffix>', AssumeRolePolicyDocument=<trusts the account root>) with NO policies attached, then sts.assume_role on it, then an ACM client from those temporary credentials. Required: HTTP 403, Error.Code 'AccessDeniedException'; the KeyId/MacKey values quoted above are per-run mints and are not themselves the requirement — the requirement is that no body is returned at all. Positive control in the same check: the owner's session reads the same binding's credentials successfully.
  Smithy citation: AccessDeniedException is modeled on this operation (httpError 403, awsQueryError AccessDenied); MacKey is @sensitive.
  Code: services/service-com-amazonaws-acm-certificatemanager/src/ops/get_acme_external_account_binding_credentials.rs handle() has no authorization step between deserialization and the store read.

Cause 3 — acm.update-acme-endpoint-on-revoked-account-succeeds [mismatch]:
  AssertionError: no Replay-Nonce returned from newNonce (status=204), raised from the check's _acme_register_account after it did HEAD <directory['newNonce']> and then, finding no field named 'Replay-Nonce', retried with GET <newNonce> and got HTTP 204 with still no field under that name.
  Upstream all passed: CreateAcmeEndpoint -> ACTIVE, the EAB created, GetAcmeExternalAccountBindingCredentials returned KeyId/MacKey, DescribeAcmeEndpoint returned EndpointUrl, and GET <EndpointUrl> served the directory 200 with newNonce/newAccount URLs.
  Code: services/service-com-amazonaws-acm-certificatemanager/src/service/acme_http.rs fn nonce inserts the field as "replay-nonce" for both the ('HEAD', NewNonce) -> 200 and ('GET', NewNonce) -> 204 arms; the module's own doc comment states the contract as the 'Replay-Nonce' header.
  Required instead: statuses stay 200 (HEAD) / 204 (GET), with the field emitted under the canonical fixed name 'Replay-Nonce' carrying a freshly minted nonce value (the value is per-request and not part of the contract), together with Cache-Control: no-store and the Link index field.
  Unreached after that point in the check: ACME newAccount registration, DescribeAcmeAccount showing VALID, RevokeAcmeAccount, UpdateAcmeEndpoint(Contact=...) succeeding on an endpoint whose only ACME account is REVOKED while the account stays REVOKED, and the fresh-endpoint positive control.

## Steps to Reproduce
Boot the emulator and point botocore at it.

Cause 1 (cross-account ARN classification):
1. acm.create_acme_endpoint(...), poll acm.describe_acme_endpoint until Status == ACTIVE.
2. iam.create_role(<EAB role>), ensure iam:PassRole, then acm.create_acme_external_account_binding(AcmeEndpointArn=..., ...) and read ExternalAccountBinding.AcmeExternalAccountBindingArn.
3. acm.get_acme_external_account_binding_credentials(AcmeExternalAccountBindingArn=<that ARN>) — succeeds with KeyId/MacKey (positive control).
4. Rewrite only the 12-digit account segment of that ARN to a different account id and call acm.get_acme_external_account_binding_credentials with it.
5. Observe HTTP 400 ResourceNotFoundException 'Could not find ACME external account binding <arn>.' AWS answers HTTP 400 ValidationException 'Invalid ARN in request: Same account ARN expected but got cross-account ARN'.
6. Re-read the real ARN: credentials must still be identical (they are).

Cause 2 (missing identity-based authorization):
1. Create an endpoint and a binding as in steps 1-2 above.
2. iam.create_role(RoleName='noperm', AssumeRolePolicyDocument=<trust the account root>) and attach NO policies; creds = sts.assume_role(RoleArn=<that role>, RoleSessionName='s')['Credentials'].
3. Build an ACM client from those credentials and call get_acme_external_account_binding_credentials(AcmeExternalAccountBindingArn=<the binding ARN>).
4. Observe HTTP 200 with {'KeyId': ..., 'MacKey': ...}. AWS answers HTTP 403 AccessDeniedException naming the assumed-role session and the acm:GetAcmeExternalAccountBindingCredentials action.
5. Confirm the owner's own session still reads the same credentials (it does).

Cause 3 (newNonce field name):
1. Create an endpoint, wait for ACTIVE, read AcmeEndpoint.EndpointUrl.
2. GET <EndpointUrl> — the directory is served 200; take directory['newNonce'].
3. HEAD <newNonce> and read the response's field names exactly as received (do NOT lower-case them, which is what a real ACME client does when it looks for 'Replay-Nonce'). No field named 'Replay-Nonce' is present.
4. GET <newNonce> — HTTP 204, again no field named 'Replay-Nonce'; the nonce is present only under 'replay-nonce'.
5. A client therefore has no nonce and cannot sign any JWS POST. AWS serves the field as 'Replay-Nonce' on both the HEAD 200 and the GET 204.

Finally re-replay the three checks in trace_ids: acm.get-acme-external-account-binding-credentials-cross-account-denied, acm.get-eab-credentials-cross-account-denied, acm.update-acme-endpoint-on-revoked-account-succeeds.
