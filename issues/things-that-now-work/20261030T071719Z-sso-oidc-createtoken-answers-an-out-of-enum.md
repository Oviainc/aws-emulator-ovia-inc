Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-oidc.create-token-unsupported-grant-type-rejected
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: sso-oidc|CreateToken|UnsupportedGrantTypeException
Issue-Commit: e635fc8db7925c597029a243d97ecda6419e6d1a

# sso-oidc CreateToken answers an out-of-enum grantType (client_credentials) with UnsupportedGrantTypeException instead of InvalidRequestException / "Invalid request"

> _Note: this issue was identified against the code base at commit `e635fc8db7925c597029a243d97ecda6419e6d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateToken

## Description
Cause 1 of 1: CreateToken's grant-type fall-through raises the modeled UnsupportedGrantTypeException for a grantType outside the three it serves, where real AWS refuses the request as a malformed one.

NOTE ON PRIOR ART: the open issue 20261029T184601Z-sso-oidc-the-modeled-unsupportedgranttypeexception-is-raised-where.md describes this same call site as its "Cause 2 of 2", but it was written from source only — in that replay the check aborted at its positive control and this arm was never reached, so the behavior was a hypothesis there, and the suite does not report this check as owned by that issue (its Issue-Key names CreateTokenWithIAM). This issue files the now-OBSERVED divergence on CreateToken: the positive control passes (device_code with a fabricated code → InvalidGrantException / "Invalid device code provided"), and the client_credentials arm is the only remaining failure in the check. A fixer closing that other issue's CreateTokenWithIAM cause does NOT close this one; both call sites must change.

AWS: CreateToken serves exactly three grant types — authorization_code, urn:ietf:params:oauth:grant-type:device_code and refresh_token (Smithy: CreateTokenRequest$grantType is required, target GrantType; its doc enumerates those three; CreateToken's modeled errors include InvalidRequestException httpError 400, InvalidGrantException 400, UnauthorizedClientException 400 and UnsupportedGrantTypeException 400, and the operation is POST /token with optionalAuth). Recorded behavior: a grantType outside those three — `client_credentials` — is refused with HTTP 400, Error.Code `InvalidRequestException`, OAuth `error` = `invalid_request`, and `error_description` = `Invalid request`. That message is WHOLLY FIXED text of the error class: the rejected grantType is not echoed and no part of it is taken from the request or from emulator state. AWS applies this regardless of what grant types the presenting client registered, so it is distinct from UnauthorizedClientException (registered but not entitled) and from InvalidGrantException (entitled grant type, unknown grant value). The modeled UnsupportedGrantTypeException is never the answer AWS gives for this input.

Emulator: answers HTTP 400 with Error.Code `UnsupportedGrantTypeException`, `error` = `unsupported_grant_type`, and an emulator-authored error_description naming the value and listing the supported grants (st::DESCRIPTION_INVALID_GRANT_TYPE). The code path is an up-front membership match on the service's own supported-grant list whose fall-through arm returns unsupported_grant_type().

Why: a client probing its grant configuration, or any error-handling branch written against AWS, sees an error code AWS never returns for this request. It cannot tell a malformed request (invalid_request — fix the request) from a grant flow the service does not serve, and OAuth clients that branch on the `invalid_request` error value never take that branch. The error_description also differs from AWS's fixed sentence, so message assertions fail too.

Fix: the CreateToken handler's grant-type resolution — the unmatched-grantType arm must refuse with InvalidRequestException (HTTP 400, `invalid_request`, fixed text `Invalid request`) rather than UnsupportedGrantTypeException, while the three supported grant types continue to flow to grant resolution (the positive control, device_code with an unknown code → InvalidGrantException / `Invalid device code provided`, must keep passing).

Traces: sso-oidc.create-token-unsupported-grant-type-rejected. Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token.rs line 282 (the `_ =>` fall-through in presented_grant) and lines 205-213 (the unsupported_grant_type helper).

## Evidence
Suite: simparity-behavioral-aws-v1 — sso-oidc 14/17 passed, 3 mismatched, bootstrap_signal false.

[mismatch] sso-oidc.create-token-unsupported-grant-type-rejected, check_sso_oidc_create_token_unsupported_grant_type_rejected.py line 64:
  AssertionError: expected InvalidRequestException, got 'UnsupportedGrantTypeException'

Request that produced it: RegisterClient(clientName=<unique>, clientType='public'), then CreateToken(clientId=<that client>, clientSecret=<that secret>, grantType='client_credentials') with no other grant members.
Expected (AWS): HTTP 400, Error.Code 'InvalidRequestException', Error.Message 'Invalid request' (entirely fixed text).
Actual (emulator): HTTP 400, Error.Code 'UnsupportedGrantTypeException' (error 'unsupported_grant_type', error_description listing the supported grant types and echoing the rejected value).

The check's in-file positive control PASSED in this replay: CreateToken with grantType='urn:ietf:params:oauth:grant-type:device_code' and deviceCode='sim2real-fabricated-device-code-does-not-exist' answered InvalidGrantException / 'Invalid device code provided', so enum validation versus grant-lookup is correctly separated for the supported grants and the failure is specific to the out-of-enum arm.

Model references used: services/service-com-amazonaws-ssooidc-awsssooidcservice/smithy-model/com-amazonaws-ssooidc-awsssooidcservice.json — CreateToken POST /token (success 200), required input members clientId/clientSecret/grantType, errors include InvalidRequestException (client, httpError 400), InvalidGrantException (400), UnauthorizedClientException (400), UnsupportedGrantTypeException (400), InvalidClientException (401).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator (sso-oidc is bootstrapped).
2. Call RegisterClient(clientName='sim2real-ssooidc-client-<unique>', clientType='public') and keep clientId/clientSecret.
3. Positive control (must keep passing): CreateToken(clientId, clientSecret, grantType='urn:ietf:params:oauth:grant-type:device_code', deviceCode='sim2real-fabricated-device-code-does-not-exist') → HTTP 400, Error.Code 'InvalidGrantException', Error.Message 'Invalid device code provided'.
4. The divergence: CreateToken(clientId, clientSecret, grantType='client_credentials') with no other grant members. Assert HTTP 400, Error.Code 'InvalidRequestException', Error.Message 'Invalid request'. The emulator returns Error.Code 'UnsupportedGrantTypeException' instead.
5. Re-replay sso-oidc.create-token-unsupported-grant-type-rejected to confirm the fix.
