Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-oidc.create-token-device-code-bound-to-issuing-client
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: sso-oidc.create-token-device-code-bound-to-issuing-client|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {sorted(result)!r}")
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# sso-oidc CreateToken auto-approves a pending device authorization after 500 ms, so polling a never-approved deviceCode returns a real access token instead of AuthorizationPendingException

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateToken

## Description
The device-authorization grant's whole point is that no token exists until a human approves the user code at the verification URI. This emulator instead hands the Pending->Approved transition to a clock (a 500 ms delay fixed in service state), so the issuing client's very first poll after the advertised `interval` succeeds. That turns a refusal into a success and destroys the rest of the device-flow state machine: the code is marked Consumed, so the cross-client and re-poll assertions that follow can never be reached. Three causes in this one flow; a fixer must close ALL THREE.

=== Cause 1 of 3: a pending device authorization is approved by elapsed time, not by a user ===
AWS: CreateToken with grantType `urn:ietf:params:oauth:grant-type:device_code` and a live deviceCode whose user code has NOT been approved answers AuthorizationPendingException — HTTP 400, error `authorization_pending`, error_description `Authorization is still pending` (Smithy: AuthorizationPendingException, errorKind client, httpError 400, members {error, error_description}; doc "Indicates that a request to authorize a client with an access user session token is pending"; `error` is documented to be exactly `authorization_pending`). Recorded behavior: it keeps answering that for every poll, however many `interval`s elapse, until the authorization is approved out of band or expires; nothing in the API surface approves it, and no access token is issued.
Emulator: StartDeviceAuthorization stamps an `approval_due_at` 500 ms in the future, and CreateToken's Pending arm flips the stored record to Approved as soon as that instant has passed, then mints and returns a token. The issuing client's poll (the check waits `interval`+1 s) returns HTTP 200 with accessToken, expiresIn, refreshToken and tokenType, and the record is then marked Consumed.
Why: this is the worst of the three — a call AWS refuses SUCCEEDS. Any device-flow client or test exercising its pending/poll loop against the emulator gets credentials for an authorization no user ever approved, so the loop's pending branch and its timeout handling are never exercised and pass vacuously; conversely a test that asserts the pending refusal fails. It also silently models an authorization server that grants tokens without user consent, which is the opposite of the security property the grant exists for.
Fix: the CreateToken handler's device-code Pending arm, together with the clock-driven approval field on the stored device authorization in service state — a Pending authorization must stay Pending for the life of the device code and answer AuthorizationPendingException; approval must come from an explicit out-of-band transition, never from elapsed time.
Traces: sso-oidc.create-token-device-code-bound-to-issuing-client. Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token.rs lines 435-456 (the `approval_is_due` -> `status = Approved` transition) with src/state.rs lines 115-120 (APPROVAL_DELAY) and 235-257 (`approval_due_at` / `approval_is_due`).

=== Cause 2 of 3: the pending refusal's error_description is emulator prose ===
AWS: the pending refusal's error_description is the fixed string `Authorization is still pending` — no part of it comes from the request or from state (the polling interval is NOT interpolated; it is already advertised in StartDeviceAuthorization's `interval`).
Emulator: error_description is `the device authorization is pending; poll again in <interval> seconds`, a format! interpolating the stored interval.
Why: the device-code polling loop is the one place clients routinely match on this text; none of them match the emulator's. (This is the same family of defect as the sso-oidc error_description issue filed alongside this one, but a different call site, and it only becomes observable once cause 1 stops short-circuiting it.)
Fix: the same Pending arm — emit AWS's fixed text with error `authorization_pending`.
Traces: sso-oidc.create-token-device-code-bound-to-issuing-client. Code: create_token.rs lines 436-449.

=== Cause 3 of 3: a device code presented by a client other than the issuing one ===
AWS: a registered client B that presents client A's still-live deviceCode (with B's own clientId/clientSecret) is refused with InvalidGrantException — HTTP 400, error `invalid_grant`, error_description `Invalid device code provided`, the same fixed text as an entirely unknown device code, so nothing about another client's authorization leaks. The refused attempt neither consumes nor invalidates the code: A's next poll still gets AuthorizationPendingException / `Authorization is still pending`.
Emulator: the handler does compare the stored `client_id` to the presenting one, but refuses with error_description `the deviceCode was not issued to the authenticating client` — prose, and it discloses that the code exists and belongs to someone else. In this replay the branch was never reached because cause 1 had already consumed the code.
Why: a client gets non-AWS text, and the emulator's message tells an attacker-shaped caller that a guessed device code is real. Without cause 1 fixed the binding cannot be observed at all.
Fix: the CreateToken device-code branch's issuing-client comparison — keep the comparison, emit the unknown-device-code fixed text, and leave the authorization Pending (not Consumed, not Approved) after a refused cross-client attempt.
Traces: sso-oidc.create-token-device-code-bound-to-issuing-client. Code: create_token.rs lines 400-404.

## Evidence
Suite: simparity-behavioral-aws-v1, sso-oidc 1/17 passed, 16 mismatched, bootstrap_signal false.

sso-oidc.create-token-device-code-bound-to-issuing-client, line 94 (client A polling its OWN pending device code, one `interval`+1 s after StartDeviceAuthorization):
  AssertionError: expected the call to be refused, but it succeeded: ['ResponseMetadata', 'accessToken', 'expiresIn', 'refreshToken', 'tokenType']
Expected at that point: Error.Code == 'AuthorizationPendingException' and Error.Message == 'Authorization is still pending'.
The check's two later steps, unreachable because of the above:
  - client B (a second registered public client) presenting A's live deviceCode must get InvalidGrantException / 'Invalid device code provided';
  - A's next poll must STILL get AuthorizationPendingException / 'Authorization is still pending' (B's refused attempt neither consumed nor invalidated the code).
The check runs against an ACTIVE IAM Identity Center instance obtained through sso-admin ListInstances/CreateInstance, and its StartDeviceAuthorization call succeeded — so the divergence is CreateToken's, not a setup failure.

Emulator code backing the "Emulator:" lines: state.rs:115-120 — "How long after StartDeviceAuthorization the pending authorization becomes approved. The emulator has no browser to approve it in, so the clock owes the Pending -> Approved transition the way a human would have made it: CHOSEN at half a second, so a client polling straight away still observes AuthorizationPendingException and its next poll (at interval) succeeds." That CHOSEN behavior is the divergence: on AWS the next poll does not succeed. create_token.rs:453 sets `authorization.status = st::DeviceAuthStatus::Approved`; :476-489 mints the token and marks the record Consumed.

Model references used: com-amazonaws-ssooidc-awsssooidcservice.json — CreateToken POST /token, errors include AuthorizationPendingException (httpError 400) and InvalidGrantException (httpError 400); AuthorizationPendingException members {error (documented value `authorization_pending`), error_description}; StartDeviceAuthorizationResponse members {deviceCode, userCode, verificationUri, verificationUriComplete, expiresIn, interval}. The exact error_description strings are recorded AWS behavior; the model documents error_description only as "Human-readable text providing additional information".

## Steps to Reproduce
1. Clone the repo, build and boot the emulator (sso-oidc is bootstrapped).
2. Ensure an ACTIVE IAM Identity Center instance exists (sso-admin ListInstances; CreateInstance if none) and derive its start URL.
3. RegisterClient two public clients A and B (distinct clientIds).
4. StartDeviceAuthorization with A's clientId/clientSecret and that startUrl; keep the deviceCode and interval.
5. Wait interval+1 seconds and call CreateToken with A's credentials, grantType='urn:ietf:params:oauth:grant-type:device_code' and that deviceCode. Assert it is REFUSED with AuthorizationPendingException / 'Authorization is still pending' — today it returns HTTP 200 with accessToken/expiresIn/refreshToken/tokenType. Repeat the poll several intervals later: it must still be pending (no elapsed-time approval).
6. Call CreateToken with B's credentials and A's deviceCode. Assert InvalidGrantException / 'Invalid device code provided'.
7. Poll again as A. Assert AuthorizationPendingException / 'Authorization is still pending' — i.e. B's refused attempt neither consumed nor invalidated the code.
8. Re-replay sso-oidc.create-token-device-code-bound-to-issuing-client; all three causes must pass before closing.
