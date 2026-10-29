Issue-Source: trace-sweep-divergence
Issue-Trace: verifiedpermissions-batch-get-policy-across-stores
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: verifiedpermissions-batch-get-policy-across-stores verifiedpermissions-identity-source-lifecycle verifiedpermissions-policy-crud-with-schema verifiedpermissions-token-authorization-error-paths
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|BatchGetPolicy|2xx->2xx|$.errors[*].message
Issue-Key: verifiedpermissions|CreateIdentitySource|4xx->4xx|$.message
Issue-Key: verifiedpermissions|CreatePolicy|4xx->4xx|$.fieldList[*].message
Issue-Key: verifiedpermissions|IsAuthorizedWithToken|4xx->4xx|$.message
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Error messages carry internal parser prose instead of the Verified Permissions wording

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetPolicy

## Description
FOUR separate causes, all in verifiedpermissions, all the same shape of defect: the failure path is reached correctly (HTTP status and error/code shape all match the recording) but the `message` string the emulator serves is its own internal prose — a Cedar parser diagnostic, a generic "Resource not found", a generic "No policy exists with..." — rather than the message Verified Permissions words for that error class. A fixer must close all FOUR before submitting; each has its own operation, fixture and step.

--- Cause 1 of 4: BatchGetPolicy per-item error message for a policy that does not exist ---
AWS: Smithy (com.amazonaws.verifiedpermissions): BatchGetPolicy returns HTTP 200 with output member `errors` of BatchGetPolicyErrorList; BatchGetPolicyErrorItem has required members `code` (enum BatchGetPolicyErrorCode: POLICY_STORE_NOT_FOUND | POLICY_NOT_FOUND | POLICY_STORE_ALIAS_NOT_FOUND), `policyStoreId`, `policyId` and `message` ("A detailed error message"). The model does not fix the message text, so this is recorded behavior: for a request item naming a policy id that does not exist in an existing store, the recorded response is 200 with the item's message exactly `Policy not found`. Fixed text: the whole string `Policy not found` — it is constant, it embeds NO value from the request (the requested policy id is already carried separately in the item's `policyId` member, and the code in `code`).
Emulator: answers 200 with the same per-item code/ids but message `No policy exists with the input ID: SPFAKE00000000000000` — a different sentence, and one that interpolates the requested policy id into the message where AWS carries no value at all.
Why: a client that classifies or asserts on per-item batch failure text sees an unexpected sentence; worse, because the emulator interpolates the request's policy id, the message is not stable across requests, so log aggregation, test fixtures and any string comparison on batch error reporting behave differently against the emulator than against AWS.
Fix: BatchGetPolicy's handler, where it builds the BatchGetPolicyErrorItem for the POLICY_NOT_FOUND case — it must emit the constant message, not format the looked-up id into it.
Traces: fixture verifiedpermissions-batch-get-policy-across-stores step task#5.

--- Cause 2 of 4: CreateIdentitySource ResourceNotFoundException message for a missing policy store ---
AWS: Smithy: CreateIdentitySource takes required `policyStoreId` and models ResourceNotFoundException (httpError 404) with required members `message`, `resourceId` and `resourceType`; the exception doc is "The request failed because it references a resource that doesn't exist." The message text is recorded behavior: creating an identity source under a policy store id that does not exist answers 404 ResourceNotFoundException with message exactly `Policy Store does not exist.` Fixed text: the entire string including the trailing period — it names the resource KIND that was missing (the policy store) and takes no value from the request; the specific id and type travel in `resourceId`/`resourceType`.
Emulator: answers 404 with message `Resource not found` — a generic sentence that does not say which kind of resource was missing.
Why: AWS's message distinguishes a missing policy store from a missing identity source / policy / template behind the same 404 and the same exception shape. A client (or an operator reading logs) that relies on that distinction cannot make it against the emulator, and any assertion on the documented sentence fails.
Fix: CreateIdentitySource's handler/validation, at the policy-store resolution step that raises ResourceNotFoundException — the message must be the resource-kind-specific sentence rather than a shared generic default.
Traces: fixture verifiedpermissions-identity-source-lifecycle step task#2.

--- Cause 3 of 4: CreatePolicy ValidationException fieldList message for an unparseable Cedar statement ---
AWS: Smithy: CreatePolicy models ValidationException (httpError 400, defaulted) with required `message` and `fieldList` of ValidationExceptionField, whose members are required `path` ("The path to the specific element that Verified Permissions found to be not valid") and required `message` ("Describes the policy validation error"). The recorded response for a static policy whose Cedar statement does not parse is 400 ValidationException whose fieldList entry message is exactly `Failed to validate policy body`. Fixed text: the whole string — it is the constant classification of the failing element, with the element itself identified by the sibling `path`; it embeds no fragment of the submitted statement.
Emulator: answers 400 with the same status and exception but fieldList[0].message `unexpected token `is`` — i.e. it leaks the raw Cedar parser diagnostic, which varies with whatever token happened to break parsing.
Why: the emulator's text is an implementation detail of its parser, not an API contract: the same malformed-policy class produces a different sentence for every different syntax error, so a client cannot recognise the validation failure class, and the surface differs per parser version. Clients matching the documented sentence break.
Fix: CreatePolicy's policy-body validation, where the Cedar parse error is converted into a ValidationExceptionField — the parser diagnostic must not become the field message; emit the constant classification instead.
Traces: fixture verifiedpermissions-policy-crud-with-schema step task#8.

--- Cause 4 of 4: IsAuthorizedWithToken ValidationException message for an identity token missing a required claim ---
AWS: Smithy: IsAuthorizedWithToken takes `identityToken` (sensitive Token, pattern of three base64url segments) and models ValidationException (httpError 400, defaulted) with required `message`. Message text is recorded behavior: an identityToken whose decoded payload lacks the `iss` claim answers 400 ValidationException with message exactly `Failed to decode IdentityToken: Invalid body: JSON error: missing field `iss` at line 1 column 15`. Structure of that message: the fixed text is the DECODE-failure framing `Failed to decode IdentityToken: Invalid body: JSON error: missing field ` plus ` at line <line> column <column>`; its variable parts are the claim name taken from the token's payload requirements (here `iss`) and the line/column position taken from the offending payload being decoded — so the position numbers come from the request's own token, they are not a constant to copy.
Emulator: answers 400 with message `Failed to verify IdentityToken: the token names no issuer` — it reports the failure as VERIFICATION rather than DECODING and replaces the claim-level decode detail with its own prose, naming no claim and no position.
Why: AWS separates "the token could not be decoded into the expected claim set" from "the token was decoded but failed verification" (bad signature, wrong issuer/audience); the emulator collapses a decode failure into the verify wording. A client distinguishing a malformed token (fix the token) from a rejected token (fix trust configuration) draws the wrong conclusion, and the claim name and position a caller needs to repair the token are gone.
Fix: IsAuthorizedWithToken's token-handling path, at the boundary between claim-set deserialization and token verification — a missing required claim must surface the decode-stage message naming the claim and its position, not the verify-stage message.
Traces: fixture verifiedpermissions-token-authorization-error-paths step task#2.

Note: no open verifiedpermissions issue carries these replay keys. The closest open items — 20261029T182116Z-cognito-idp-verifiedpermissions-isauthorizedwithtoken-answers-the-wrong-validationexception (wrong ValidationException message for an ID token whose `aud` is not in the identity source) and 20261029T120600Z-verifiedpermissions-isauthorizedwithtoken-error-response (OIDC sources answering 500) — concern different inputs and different failure stages than Cause 4, which is a missing `iss` claim at decode time, and neither covers Causes 1-3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 91 steps, 86 passed, 5 mismatched, 4 divergence clusters — all four are these causes, all service=verifiedpermissions, verdict=mismatch, no stub_path and no owned_by on any of them).

Cause 1 — BatchGetPolicy, expected_status 200 / actual_status 200, key $.errors[0].message:
  expected: 'Policy not found'
  actual:   'No policy exists with the input ID: SPFAKE00000000000000'
  (divergence_key service=verifiedpermissions|BatchGetPolicy|status=200->200|keys=$.errors[0].message)

Cause 2 — CreateIdentitySource, expected_status 404 / actual_status 404, key $.message:
  expected: 'Policy Store does not exist.'
  actual:   'Resource not found'
  (divergence_key service=verifiedpermissions|CreateIdentitySource|status=404->404|keys=$.message)

Cause 3 — CreatePolicy, expected_status 400 / actual_status 400, key $.fieldList[0].message:
  expected: 'Failed to validate policy body'
  actual:   'unexpected token `is`'
  (divergence_key service=verifiedpermissions|CreatePolicy|status=400->400|keys=$.fieldList[0].message)

Cause 4 — IsAuthorizedWithToken, expected_status 400 / actual_status 400, key $.message:
  expected: 'Failed to decode IdentityToken: Invalid body: JSON error: missing field `iss` at line 1 column 15'
  actual:   'Failed to verify IdentityToken: the token names no issuer'
  (divergence_key service=verifiedpermissions|IsAuthorizedWithToken|status=400->400|keys=$.message)

In every one of the four the HTTP status matches and only the message string diverges, so the failure path is selected correctly and the defect is confined to the message each path emits.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus against trace set simparity-wire-aws-v1), then compare each cause's step:

1. Cause 1: fixture verifiedpermissions-batch-get-policy-across-stores, step task#5 — call BatchGetPolicy with a request item naming an existing policy store and a policy id that does not exist in it. Read errors[0].message: expect the constant 'Policy not found'; the emulator returns 'No policy exists with the input ID: <requested policy id>'.
2. Cause 2: fixture verifiedpermissions-identity-source-lifecycle, step task#2 — call CreateIdentitySource with a policyStoreId that does not exist. Read message on the 404 ResourceNotFoundException: expect 'Policy Store does not exist.'; the emulator returns 'Resource not found'.
3. Cause 3: fixture verifiedpermissions-policy-crud-with-schema, step task#8 — call CreatePolicy with a static definition whose Cedar statement does not parse. Read fieldList[0].message on the 400 ValidationException: expect 'Failed to validate policy body'; the emulator returns the raw parser diagnostic ('unexpected token `is`' for this statement, and a different one for any other syntax error).
4. Cause 4: fixture verifiedpermissions-token-authorization-error-paths, step task#2 — call IsAuthorizedWithToken with an identityToken whose payload omits the `iss` claim. Read message on the 400 ValidationException: expect the decode-stage message 'Failed to decode IdentityToken: Invalid body: JSON error: missing field `iss` at line <line> column <column>' with the position taken from the supplied token; the emulator returns the verify-stage 'Failed to verify IdentityToken: the token names no issuer'.

Each step passes only when the status (unchanged) and the message string both match.
