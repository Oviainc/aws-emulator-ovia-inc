Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-oidc.create-token-invalid-grant-unknown-device-code-rejected sso-oidc.create-token-missing-device-code-invalid-request-rejected sso-oidc.create-token-refresh-token-invalid-grant-rejected sso-oidc.create-token-refresh-token-lookup-precedes-client-existence sso-oidc.create-token-unauthorized-client-wrong-grant-type-rejected sso-oidc.create-token-with-iam-invalid-client-rejected sso-oidc.register-client-accepts-malformed-redirect-uri sso-oidc.register-client-confidential-type-rejected sso-oidc.register-client-default-grant-types-allow-device-flow sso-oidc.register-client-distinct-credentials-per-call sso-oidc.register-client-returns-valid-credential-pair sso-oidc.start-device-authorization-starturl-precondition-rejects-any-starturl sso-oidc.start-device-authorization-wrong-client-secret-rejected
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: sso-oidc|CreateToken|InvalidGrantException
Issue-Key: sso-oidc|CreateToken|InvalidRequestException
Issue-Key: sso-oidc|CreateTokenWithIAM|InvalidGrantException
Issue-Key: sso-oidc|RegisterClient|InvalidClientMetadataException
Issue-Key: sso-oidc|StartDeviceAuthorization|InvalidClientException
Issue-Key: sso-oidc.start-device-authorization-wrong-client-secret-rejected|run#9|AssertionError|assert message == "Invalid client secret provided", f"malformed-secret message was {message!r}"
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# sso-oidc: 8 net-new behavioral root causes — every error_description is emulator prose instead of AWS's fixed per-error-class text (CreateToken, CreateTokenWithIAM, RegisterClient, StartDeviceAuthorization)

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateToken

## Description
Replaying simparity-behavioral-aws-v1 against the emulator failed 16 of 17 sso-oidc checks. Thirteen of them fail on the SAME shape of defect, in EIGHT independent places: AWS SSO OIDC's `error_description` (the member botocore surfaces as Error.Message) is a FIXED, short, sentence-case English string per error class, with no request-derived parts; this emulator instead synthesises its own diagnostic prose (and interpolates request values and its own grant-type lists into it). Every error CODE below is already correct — only the text diverges — so each cause is a one-line text change in the named handler helper call site. A fixer must close ALL EIGHT before submitting.

Related but NOT covering this: the open WIRE issue 20261029T180355Z-oidc-sso-oidc-responses-diverge-from-aws-error-description.md (service token `oidc`) reports the same text defect for two of these call sites (RegisterClient clientType, CreateToken unknown deviceCode) from the wire corpus. It is a different work item under a different service token and does not carry these behavioral check keys; causes 5 and 1 here overlap with it and may be fixed by the same edit.

=== Cause 1 of 8: CreateToken invalid_grant for an unknown deviceCode ===
AWS: CreateToken with grantType `urn:ietf:params:oauth:grant-type:device_code` and a `deviceCode` that names no device authorization answers HTTP 400, error `invalid_grant`, error_description `Invalid device code provided` (Smithy: CreateToken errors include InvalidGrantException, httpError 400, members {error, error_description}; doc "Indicates that a request contains an invalid grant"). Recorded behavior: the whole string is FIXED text for this error class — the rejected deviceCode is NOT echoed (it is a credential), and no part of it comes from the request or from state.
Emulator: HTTP 400, error `invalid_grant` (correct), error_description `the deviceCode does not name a device authorization of this service`.
Why: a device-flow client or test that distinguishes invalid_grant causes by AWS's description text — the normal pattern inside a device-code polling loop — never matches, so the emulator's whole negative device-code path is unusable for that logic. This single string is what breaks 7 otherwise-unrelated checks, including three that only use CreateToken as an independent read to prove a RegisterClient credential pair is live.
Fix: the CreateToken handler's device-code grant-resolution branch — the `invalid_grant(...)` call sites, which should pass AWS's fixed text.
Traces: sso-oidc.create-token-invalid-grant-unknown-device-code-rejected, sso-oidc.create-token-unauthorized-client-wrong-grant-type-rejected, sso-oidc.register-client-accepts-malformed-redirect-uri, sso-oidc.register-client-default-grant-types-allow-device-flow, sso-oidc.register-client-distinct-credentials-per-call, sso-oidc.register-client-returns-valid-credential-pair, sso-oidc.register-client-confidential-type-rejected (second half). Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token.rs lines 260-264 and 396-399, helper at 195-201.

=== Cause 2 of 8: CreateToken invalid_request for a device_code grant with no deviceCode member ===
AWS: CreateToken with grantType device_code and the optional `deviceCode` member absent is rejected HTTP 400, error `invalid_request`, error_description `Device code is required` (Smithy: deviceCode is NOT @required on CreateTokenRequest, so this is a per-grant requirement the handler enforces; InvalidRequestException httpError 400, doc "something is wrong with the input to the request. For example, a required parameter might be missing"). Recorded behavior: the message is wholly fixed text; it names the member in AWS's own words ("Device code"), not the wire member name, and does not quote the grant type.
Emulator: HTTP 400, error `invalid_request` (correct), error_description `deviceCode is required for the urn:ietf:params:oauth:grant-type:device_code grant` — a format! that interpolates the grant-type constant.
Why: a client that branches on the missing-member message cannot tell this class apart from AWS's wording, and the emulator teaches a different contract than AWS states.
Fix: the CreateToken handler's device-code branch, missing-member arm (the `invalid_request(format!(...))` call) — emit the fixed text.
Traces: sso-oidc.create-token-missing-device-code-invalid-request-rejected. Code: create_token.rs lines 255-258.

=== Cause 3 of 8: CreateToken invalid_request for a refresh_token grant with no refreshToken member ===
AWS: CreateToken with grantType `refresh_token` and `refreshToken` absent answers HTTP 400, error `invalid_request`, error_description `Refresh token is required` — fixed text, no request-derived part.
Emulator: HTTP 400, error `invalid_request` (correct), error_description `refreshToken is required for the refresh_token grant`.
Why: same as cause 2 — the missing-member class is unrecognisable to a client matching AWS's text.
Fix: the CreateToken handler's refresh-token branch, missing-member arm.
Traces: sso-oidc.create-token-refresh-token-invalid-grant-rejected (first assertion). Code: create_token.rs lines 273-276.

=== Cause 4 of 8: CreateToken invalid_grant for an unknown refreshToken ===
AWS: CreateToken with grantType refresh_token and a `refreshToken` value that is not a previously issued token answers HTTP 400, error `invalid_grant`, error_description `Invalid refresh token provided` — fixed text for the class; the rejected token is not echoed. This is the same answer whether or not the presenting `clientId` was ever registered (AWS resolves the opaque refresh token independently of client existence), so the text must not mention the client.
Emulator: HTTP 400, error `invalid_grant` (correct), error_description `the refreshToken does not name a refresh grant of this service` (and, from the rotation path, `the refreshToken is unknown, has already been rotated, or was not issued to the authenticating client`).
Why: a client refreshing a stale cached token distinguishes "refresh token no longer valid, re-run the device flow" from other invalid_grant causes by this text; the emulator's prose also conflates three distinct conditions into one sentence AWS never emits.
Fix: the CreateToken handler's refresh-token grant-resolution and rotation-failure arms — emit AWS's fixed text for both.
Traces: sso-oidc.create-token-refresh-token-lookup-precedes-client-existence, sso-oidc.create-token-refresh-token-invalid-grant-rejected (second assertion). Code: create_token.rs lines 278-282 and 503-507.

=== Cause 5 of 8: RegisterClient InvalidClientMetadataException for a non-public clientType ===
AWS: RegisterClient rejects any `clientType` other than `public` (Smithy: RegisterClientRequest$clientType @required; its doc: "The service supports only public as a client type. Anything other than public will be rejected by the service") with HTTP 400, error `invalid_client_metadata`, error_description `Invalid client type provided` — entirely fixed text; the rejected value is NOT echoed.
Emulator: HTTP 400, error `invalid_client_metadata` (correct), error_description `clientType must be 'public', not 'confidential'` — a template interpolating both the supported and the rejected value from the request.
Why: a client asserting on AWS's metadata-rejection text never matches, and the emulator states a more specific contract than AWS does. (Also reported for the wire corpus by the open issue named above.)
Fix: the RegisterClient handler's clientType validation branch.
Traces: sso-oidc.register-client-confidential-type-rejected. Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/register_client.rs lines ~200-212.

=== Cause 6 of 8: StartDeviceAuthorization InvalidClientException for an unregistered clientId ===
AWS: StartDeviceAuthorization presented a `clientId` that was never issued by RegisterClient answers InvalidClientException (Smithy: StartDeviceAuthorization errors include InvalidClientException, httpError 401, members {error, error_description}; doc "the clientId or clientSecret in the request is invalid") with error_description `Invalid client provided` — fixed text, no echo of the rejected id. (Recorded behavior: real AWS may instead answer InvalidRequestException / `Invalid request` for this request when its startUrl precondition is evaluated first; the check accepts either CODE but requires the matching fixed text.)
Emulator: InvalidClientException (code accepted) with error_description `no client is registered with the given clientId`.
Why: the caller is told a different thing than AWS says, and credential-triage logic keyed on AWS's text falls through to its unknown-error path.
Fix: the shared client-authentication failure description used by StartDeviceAuthorization — it must map each failure to AWS's fixed text rather than to its own diagnostic sentence.
Traces: sso-oidc.start-device-authorization-starturl-precondition-rejects-any-starturl. Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs lines 387-396 (`ClientAuthFailure::description`), reached from the StartDeviceAuthorization handler.

=== Cause 7 of 8: StartDeviceAuthorization InvalidClientException for a wrong clientSecret ===
AWS: StartDeviceAuthorization with a known `clientId` and a secret that is not that registration's own answers InvalidClientException (HTTP 401, error `invalid_client`). The text is fixed per sub-case: a malformed/garbage secret gives error_description `Invalid client secret provided`, while another registration's VALID secret presented against this clientId gives `Invalid client ID provided`. The variable parts are none — both strings are wholly fixed text for their sub-case.
Emulator: InvalidClientException (code correct) with error_description `the clientSecret does not match the registered clientId` for both sub-cases, so the two sub-cases are also indistinguishable.
Why: a client cannot tell "this secret is junk" from "this id/secret pair belongs to two different registrations", and no assertion on AWS's text matches. The same check then exercises the positive path (the client's own secret mints a deviceCode/userCode with verificationUriComplete ending in the userCode), so the fixer must keep that path working.
Fix: the same shared client-authentication failure mapping as cause 6, with distinct AWS text for the malformed-secret and the cross-registration-secret sub-cases.
Traces: sso-oidc.start-device-authorization-wrong-client-secret-rejected. Code: state.rs lines 387-396, reached from the StartDeviceAuthorization handler.

=== Cause 8 of 8: CreateTokenWithIAM refuses on clientId resolution with a clientId-flavoured invalid_grant text instead of AWS's refresh-token text ===
AWS: CreateTokenWithIAM with grantType `refresh_token`, a well-formed but never-existing IAM Identity Center application ARN as `clientId`, and a `refreshToken` that was never issued answers HTTP 400, error `invalid_grant`, error_description `Invalid refresh token provided` (Smithy: CreateTokenWithIAM errors include InvalidGrantException, httpError 400). Recorded behavior: the refusal is attributed to the refresh token, NOT to the unknown application ARN — refresh-token validation runs independently of application-ARN resolution for this grant — and the text is fixed, echoing neither value.
Emulator: HTTP 400, error `invalid_grant` (code correct) with error_description `the clientId does not name a client with OAuth grants configured in this service`, because the handler resolves `clientId` against its own RegisterClient registry BEFORE looking at the grant.
Why: the caller is pointed at the wrong request member and no text assertion matches; a client that trusts the message would go re-register an application instead of re-running its authorization flow.
Fix: the CreateTokenWithIAM handler — resolve/validate the presented grant before (or independently of) the clientId registry lookup for the refresh_token grant, and emit AWS's fixed refresh-token text for the unknown-grant refusal.
Traces: sso-oidc.create-token-with-iam-invalid-client-rejected. Code: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/create_token_with_iam.rs lines 360-370 (clientId lookup ahead of the grant) and 428-432.

## Evidence
Suite: simparity-behavioral-aws-v1, sso-oidc 1/17 passed (6%), 16 mismatched, 0 errored, 0 skipped, bootstrap_signal false (the service answers — RegisterClient succeeds and every refusal below carries the right error code).

Cause 1 — assertion `err.response['Error']['Message'] == 'Invalid device code provided'`:
  AssertionError: unexpected message: 'the deviceCode does not name a device authorization of this service'
  (identical text reported by sso-oidc.create-token-invalid-grant-unknown-device-code-rejected line 53, sso-oidc.create-token-unauthorized-client-wrong-grant-type-rejected line 81, sso-oidc.register-client-accepts-malformed-redirect-uri line 61, sso-oidc.register-client-default-grant-types-allow-device-flow line 58, sso-oidc.register-client-returns-valid-credential-pair line 67, and sso-oidc.register-client-distinct-credentials-per-call line 62: "expected message 'Invalid device code provided' for clientId=<minted id>, got 'the deviceCode does not name a device authorization of this service'" — the clientId in that text is a per-run minted value and is NOT part of the finding.)

Cause 2 — sso-oidc.create-token-missing-device-code-invalid-request-rejected line 43:
  expected 'Device code is required', got 'deviceCode is required for the urn:ietf:params:oauth:grant-type:device_code grant'

Cause 3 — sso-oidc.create-token-refresh-token-invalid-grant-rejected line 41:
  expected 'Refresh token is required', got 'refreshToken is required for the refresh_token grant'

Cause 4 — sso-oidc.create-token-refresh-token-lookup-precedes-client-existence line 44:
  expected 'Invalid refresh token provided', got 'the refreshToken does not name a refresh grant of this service' (error code InvalidGrantException was correct, so the documented ordering guarantee holds — only the text diverges)

Cause 5 — sso-oidc.register-client-confidential-type-rejected line 28:
  expected 'Invalid client type provided', got "clientType must be 'public', not 'confidential'" (code InvalidClientMetadataException correct)

Cause 6 — sso-oidc.start-device-authorization-starturl-precondition-rejects-any-starturl line 51:
  expected 'Invalid client provided' for InvalidClientException, got 'no client is registered with the given clientId'

Cause 7 — sso-oidc.start-device-authorization-wrong-client-secret-rejected line 87:
  malformed-secret message was 'the clientSecret does not match the registered clientId' (expected 'Invalid client secret provided'); the check's next assertion requires 'Invalid client ID provided' for client A's id paired with client B's valid secret, which the emulator answers with the same single sentence

Cause 8 — sso-oidc.create-token-with-iam-invalid-client-rejected line 47:
  expected 'Invalid refresh token provided', got 'the clientId does not name a client with OAuth grants configured in this service' (code InvalidGrantException correct)

Model references used: services/service-com-amazonaws-ssooidc-awsssooidcservice/smithy-model/com-amazonaws-ssooidc-awsssooidcservice.json — restJson1; CreateToken POST /token errors {InvalidGrantException 400, InvalidRequestException 400, InvalidClientException 401, AuthorizationPendingException 400, UnsupportedGrantTypeException 400, ...}; CreateTokenRequest members clientId/clientSecret/grantType @required, deviceCode/refreshToken/code optional; RegisterClient POST /client/register error InvalidClientMetadataException 400; StartDeviceAuthorization POST /device_authorization errors {InvalidClientException 401, InvalidRequestException 400, ...}; every one of these error shapes carries exactly {error, error_description} (InvalidRequestException also `reason`), and the model fixes `error` but documents error_description only as "Human-readable text providing additional information" — the exact strings above are recorded AWS behavior.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator (sso-oidc is bootstrapped).
2. Replay the simparity-behavioral-aws-v1 behavioral suite, sso-oidc checks.
3. Cause 1 (representative): RegisterClient a public client, then CreateToken with grantType=urn:ietf:params:oauth:grant-type:device_code and deviceCode='sim2real-fabricated-device-code-does-not-exist'; assert Error.Code == InvalidGrantException AND Error.Message == 'Invalid device code provided'.
4. Cause 2: same CreateToken call with the deviceCode member omitted; assert InvalidRequestException / 'Device code is required'.
5. Cause 3: CreateToken with grantType=refresh_token and no refreshToken; assert InvalidRequestException / 'Refresh token is required'.
6. Cause 4: CreateToken with grantType=refresh_token and refreshToken='sim2real-bogus-refresh-token-value', both for a registered client and for a clientId that was never registered; assert InvalidGrantException / 'Invalid refresh token provided' in BOTH cases.
7. Cause 5: RegisterClient with clientType='confidential'; assert InvalidClientMetadataException / 'Invalid client type provided'.
8. Cause 6: StartDeviceAuthorization with a never-registered clientId, a bogus secret and startUrl='https://example.awsapps.com/start'; assert the message is 'Invalid client provided' when the code is InvalidClientException (or 'Invalid request' when it is InvalidRequestException).
9. Cause 7: register two public clients A and B (needs an ACTIVE IAM Identity Center instance via sso-admin); StartDeviceAuthorization for A's clientId with (a) a garbage secret -> InvalidClientException / 'Invalid client secret provided', (b) B's valid secret -> InvalidClientException / 'Invalid client ID provided', (c) A's own secret -> 200 with deviceCode, userCode, expiresIn > 0 and verificationUriComplete ending in the userCode.
10. Cause 8: CreateTokenWithIAM with clientId='arn:aws:sso::111122223333:application/ssoins-0000000000000000/apl-0000000000000000', grantType=refresh_token, refreshToken='sim2real-bogus-refresh-token'; assert InvalidGrantException / 'Invalid refresh token provided'.
11. All eight causes must pass before closing; re-replay every check id in Trace-Ids, not just the representative. Note that sso-oidc.create-token-unsupported-grant-type-rejected (filed separately for its grant-type-class defect) also depends on cause 1.
