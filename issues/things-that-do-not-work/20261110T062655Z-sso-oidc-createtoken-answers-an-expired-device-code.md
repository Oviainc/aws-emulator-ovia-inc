Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-oidc.create-token-expired-after-device-code-ttl
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: sso-oidc.create-token-expired-after-device-code-ttl|run#11|AssertionError|raise AssertionError(f"unexpected error code {code!r} ({message!r}) while polling")
Issue-Commit: 1ecf6f83c17663b1cceb804b029a144947c92d6a

# sso-oidc CreateToken answers an EXPIRED device code with ExpiredTokenException / "Expired device code provided" instead of InvalidGrantException / "Invalid device code provided"

> _Note: this issue was identified against the code base at commit `1ecf6f83c17663b1cceb804b029a144947c92d6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateToken

## Description
One net-new root cause in sso-oidc's device-authorization grant.

Cause 1 of 1: the device-code TTL expiry arm of CreateToken raises the wrong error class (and the wrong fixed text) when a device code has outlived its `expiresIn`.

AWS: CreateToken with grantType `urn:ietf:params:oauth:grant-type:device_code` and a deviceCode whose TTL (the `expiresIn` StartDeviceAuthorization advertised, about 10 minutes) has elapsed is refused with InvalidGrantException — HTTP 400, error code `InvalidGrantException` on the wire (Smithy: com.amazonaws.ssooidc#InvalidGrantException, errorKind client, httpError 400, members {error, error_description}; `error` is documented to be exactly `invalid_grant`). Recorded behavior: AWS simply drops the expired authorization, so the expired code is indistinguishable from an unknown one and the refusal carries the SAME fixed error_description as an unknown/spent/other-client device code — `Invalid device code provided`. That string is wholly fixed text for this refusal class: no part of it is taken from the request or from state, and nothing about the code's prior existence or its expiry is disclosed. The model's ExpiredTokenException is documented as "Indicates that the token issued by the service is expired and is no longer valid" (error value `expired_token`) — it describes a TOKEN the service issued (an access/refresh token presented back to it), not an unredeemed device authorization grant, and the recording shows AWS does not use it for a device code.

Emulator: the device-code branch checks `authorization.is_expired(now)` BEFORE looking at the authorization's status and refuses with ExpiredTokenException — error `expired_token`, error_description `Expired device code provided` (st::DESCRIPTION_EXPIRED_DEVICE_CODE). The replay observed exactly that: while polling a never-approved device code past its TTL, the transition out of AuthorizationPendingException was `ExpiredTokenException ('Expired device code provided')` where AWS answers `InvalidGrantException ('Invalid device code provided')`. The expired record is also left in the store rather than dropped, which is what makes the distinct class observable at all.

Why: a device-flow client's polling loop is written against RFC 8628 / AWS's classes — it keeps polling on `authorization_pending`/`slow_down` and terminates on `invalid_grant`. Against the emulator it receives an error class that AWS never produces here, so the loop's terminal branch is not taken: the client either keeps polling a dead code until its own deadline, or crashes on an unexpected exception type (botocore raises ExpiredTokenException, a different exception class from InvalidGrantException, so `except InvalidGrantException` handlers do not catch it). Clients that triage by error_description also read emulator-only text. Conversely a client that DOES handle expired_token specially would be written against behavior real AWS does not have. Note this is a different call site from the already-fixed sso-oidc error_description and auto-approval issues (20261029T184601Z-sso-oidc-createtoken-auto-approves-a-pending-device.md fixed the Pending arm; this is the expiry arm, only reachable once Pending stops short-circuiting it), and it is not covered by the open CreateTokenWithIAM issue.

Fix: the CreateToken handler's device-code grant branch — its expiry check must produce the invalid-grant refusal with the unknown-device-code fixed text, i.e. the same refusal as an unknown, spent or other-client device code, rather than a separate expired-token class; and the expired authorization should be dropped from service state so no later call can distinguish it. The ExpiredTokenException class stays correct for the refresh-token arm, which presents a token the service issued.

Traces: sso-oidc.create-token-expired-after-device-code-ttl. Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token.rs lines 386-396 (the `is_expired` -> ExpiredTokenException arm) with src/state.rs line 132 (DESCRIPTION_INVALID_DEVICE_CODE, the fixed text this arm should use) and the DESCRIPTION_EXPIRED_DEVICE_CODE constant it uses today.

## Evidence
Suite: simparity-behavioral-aws-v1, sso-oidc 17/18 passed, 1 mismatched, bootstrap_signal false.

[mismatch] sso-oidc.create-token-expired-after-device-code-ttl, check_sso_oidc_create_token_expired_after_device_code_ttl.py line 90:
  AssertionError: unexpected error code 'ExpiredTokenException' ('Expired device code provided') while polling

The check RegisterClient(public, grantTypes=[device_code]) -> StartDeviceAuthorization(startUrl of an ACTIVE IAM Identity Center instance) -> polls CreateToken(grantType=device_code, deviceCode) at the advertised `interval`, tolerating AuthorizationPendingException and SlowDownException, until the code's TTL elapses. It observed the pending state first (so setup and the pending arm worked), then at expiry got:
  Error.Code   = 'ExpiredTokenException'   (expected 'InvalidGrantException')
  Error.Message= 'Expired device code provided' (expected 'Invalid device code provided')
HTTP status is 400 in both cases; the divergence is the error class and the error_description, not the status.

Fixed text vs variable parts: for this refusal class AWS's error_description is entirely fixed — 'Invalid device code provided' — with NO variable part (no deviceCode, clientId, TTL or timestamp interpolated). The wire error code is the fixed `InvalidGrantException` / `invalid_grant`.

Model references used: services/service-com-amazonaws-ssooidc-awsssooidcservice/smithy-model/com-amazonaws-ssooidc-awsssooidcservice.json — CreateToken POST /token (200), errors include AuthorizationPendingException, ExpiredTokenException, InvalidGrantException, SlowDownException, all httpError 400; InvalidGrantException.error documented value `invalid_grant`, ExpiredTokenException.error documented value `expired_token` with doc "Indicates that the token issued by the service is expired and is no longer valid". The exact error_description strings are recorded AWS behavior; the model documents error_description only as "Human-readable text providing additional information".

Emulator code backing the "Emulator:" lines: create_token.rs:386-396 returns CreateTokenErrorModeled::ExpiredTokenException{error:"expired_token", error_description: st::DESCRIPTION_EXPIRED_DEVICE_CODE} for `authorization.is_expired(now)`, while the unknown/other-client/consumed arms at :377, :384 and :411 all return invalid_grant(st::DESCRIPTION_INVALID_DEVICE_CODE) = 'Invalid device code provided'.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator (sso-oidc is bootstrapped).
2. Ensure an ACTIVE IAM Identity Center instance exists (sso-admin ListInstances; CreateInstance if none) and derive its start URL `https://<IdentityStoreId>.awsapps.com/start`.
3. RegisterClient(clientName=<unique>, clientType='public', grantTypes=['urn:ietf:params:oauth:grant-type:device_code']).
4. StartDeviceAuthorization(clientId, clientSecret, startUrl); keep deviceCode, interval and expiresIn.
5. Poll CreateToken(clientId, clientSecret, grantType='urn:ietf:params:oauth:grant-type:device_code', deviceCode) every `interval` seconds without ever approving the user code. Each poll must be refused with AuthorizationPendingException / 'Authorization is still pending' (this part passes today).
6. Keep polling past `expiresIn`. Assert the terminal refusal is Error.Code == 'InvalidGrantException' and Error.Message == 'Invalid device code provided' — today it is 'ExpiredTokenException' / 'Expired device code provided'.
7. Optionally re-present the same deviceCode after expiry: it must keep answering the identical invalid-grant refusal (the expired authorization is dropped, so it is indistinguishable from an unknown deviceCode).
8. Re-replay sso-oidc.create-token-expired-after-device-code-ttl; it must pass before closing.
