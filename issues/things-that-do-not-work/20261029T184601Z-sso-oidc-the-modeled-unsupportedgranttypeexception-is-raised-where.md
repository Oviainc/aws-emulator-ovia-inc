Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-oidc.create-token-with-iam-unsupported-grant-type-rejected sso-oidc.create-token-unsupported-grant-type-rejected
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: sso-oidc|CreateTokenWithIAM|UnsupportedGrantTypeException
Issue-Related: 20261029T184601Z-sso-oidc-8-net-new-behavioral-root-causes.md
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# sso-oidc: the modeled UnsupportedGrantTypeException is raised where real AWS never uses it — CreateTokenWithIAM answers a device_code grantType with unsupported_grant_type 400 instead of InvalidClientException 401

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTokenWithIAM

## Description
Both token operations gate `grantType` against their own supported-grant list and answer anything outside it with the modeled UnsupportedGrantTypeException. Real AWS does not use that error for either of these requests: it refuses them with a different error CLASS (and, for CreateTokenWithIAM, a different HTTP STATUS). Two independent call sites; a fixer must close BOTH.

=== Cause 1 of 2: CreateTokenWithIAM with grantType=device_code ===
AWS: CreateTokenWithIAM serves only the authorization-code, refresh-token, JWT-bearer and token-exchange grants — the device-code grant is CreateToken's and CreateTokenWithIAMRequest has no `deviceCode` member at all (Smithy: CreateTokenWithIAMRequest$grantType doc lists exactly those four; the request shape's members are clientId, grantType, code, refreshToken, assertion, scope, redirectUri, subjectToken, subjectTokenType, requestedTokenType, codeVerifier). Recorded behavior: AWS rejects grantType=`urn:ietf:params:oauth:grant-type:device_code` with InvalidClientException — HTTP 401, error `invalid_client`, error_description `Invalid client provided` (fixed text) — and does so even for a well-formed but never-existing application ARN, i.e. before any application-ARN existence nuance can matter. The contrast in the same check pins this down: the SAME bogus ARN under the supported refresh_token grant instead reaches grant-lookup and answers InvalidGrantException / `Invalid refresh token provided`.
Emulator: answers UnsupportedGrantTypeException — HTTP 400, error `unsupported_grant_type`, error_description `grantType '<value>' is not supported; supported grant types are authorization_code, refresh_token, urn:ietf:params:oauth:grant-type:jwt-bearer, urn:ietf:params:oauth:grant-type:token-exchange` — from an up-front membership test on its own grant list.
Why: a SigV4 client gets the wrong status (400 vs 401), the wrong error code, and prose instead of AWS's fixed text; retry/credential-refresh logic that branches on invalid_client never fires, and any test asserting AWS's 401 fails against the emulator.
Fix: the CreateTokenWithIAM handler's grant-type gate — an unsupported grant on this operation must be refused as InvalidClientException (401, `invalid_client`, `Invalid client provided`), not as UnsupportedGrantTypeException, while the four supported grants keep flowing to grant resolution.
Traces: sso-oidc.create-token-with-iam-unsupported-grant-type-rejected. Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token_with_iam.rs lines 353-358 (the gate) and 237-248 (the unsupported_grant_type helper).

=== Cause 2 of 2: CreateToken with an out-of-enum grantType (client_credentials) ===
AWS: CreateToken supports only authorization_code, urn:ietf:params:oauth:grant-type:device_code and refresh_token (Smithy: CreateTokenRequest$grantType doc). Recorded behavior: a grantType outside those three — `client_credentials` — is refused with InvalidRequestException, HTTP 400, error `invalid_request`, error_description `Invalid request` (wholly fixed text, the rejected grantType is not echoed) — NOT with the modeled UnsupportedGrantTypeException, and regardless of what the presenting client registered.
Emulator: this arm was not reached in this replay (the check aborted at its positive control, which hits the deviceCode-message defect filed separately). From the handler source the fall-through arm answers UnsupportedGrantTypeException — HTTP 400, error `unsupported_grant_type`, error_description `grantType 'client_credentials' is not supported; supported grant types are authorization_code, urn:ietf:params:oauth:grant-type:device_code, refresh_token`. Hypothesis: the handler was written from the modeled error list rather than from recorded behavior, which is also what produced cause 1.
Why: a client that validates its grant configuration by probing CreateToken sees an error code AWS never returns for this input, so it cannot distinguish a malformed request from a grant the service does not serve.
Fix: the CreateToken handler's grant-type match fall-through arm — refuse an out-of-enum grantType as InvalidRequestException with error `invalid_request` and AWS's fixed `Invalid request` text.
Traces: sso-oidc.create-token-unsupported-grant-type-rejected (this check also depends on the unknown-deviceCode message fixed under the sso-oidc error_description issue filed alongside this one — both must land for it to pass). Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token.rs line 299 (the `other =>` arm) and 203-213 (the unsupported_grant_type helper).

## Evidence
Suite: simparity-behavioral-aws-v1, sso-oidc 1/17 passed, 16 mismatched, bootstrap_signal false.

Cause 1 — sso-oidc.create-token-with-iam-unsupported-grant-type-rejected, line 44:
  AssertionError: expected InvalidClientException, got 'UnsupportedGrantTypeException'
  (request: create_token_with_iam(clientId='arn:aws:sso::111122223333:application/ssoins-0000000000000000/apl-0000000000000000', grantType='urn:ietf:params:oauth:grant-type:device_code'); the check then requires Error.Message == 'Invalid client provided', and, as a contrast, the same ARN with grantType=refresh_token + a bogus refreshToken answering InvalidGrantException / 'Invalid refresh token provided'.)

Cause 2 — sso-oidc.create-token-unsupported-grant-type-rejected: the check's positive control (grantType=device_code with a fabricated deviceCode) failed first at line 48 — "unexpected message: 'the deviceCode does not name a device authorization of this service'" — so the client_credentials assertion (expects InvalidRequestException / 'Invalid request') was never evaluated. The emulator's answer for it is read from create_token.rs:299, which routes every unmatched grantType to unsupported_grant_type().

Model references used: com-amazonaws-ssooidc-awsssooidcservice.json — CreateTokenWithIAM POST /token?aws_iam=t, errors {InvalidClientException httpError 401, InvalidGrantException 400, InvalidRequestException 400, UnsupportedGrantTypeException 400, ...}; InvalidClientException members {error, error_description}, errorKind client, httpError 401; CreateTokenWithIAMRequest has no deviceCode member; CreateToken errors include InvalidRequestException 400 and UnsupportedGrantTypeException 400.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator (sso-oidc is bootstrapped).
2. Cause 1: call CreateTokenWithIAM (SigV4) with clientId='arn:aws:sso::111122223333:application/ssoins-0000000000000000/apl-0000000000000000' and grantType='urn:ietf:params:oauth:grant-type:device_code'. Assert HTTP 401, Error.Code == 'InvalidClientException', Error.Message == 'Invalid client provided'. Then assert the contrast still holds: the same clientId with grantType='refresh_token' and refreshToken='sim2real-bogus-refresh-token' gives InvalidGrantException / 'Invalid refresh token provided'.
3. Cause 2: RegisterClient a public client, then call CreateToken with that client's clientId/clientSecret and grantType='client_credentials' (no other grant members). Assert HTTP 400, Error.Code == 'InvalidRequestException', Error.Message == 'Invalid request'.
4. Re-replay sso-oidc.create-token-with-iam-unsupported-grant-type-rejected and sso-oidc.create-token-unsupported-grant-type-rejected; the latter also needs the unknown-deviceCode message fix from the companion sso-oidc error_description issue.
