Issue-Source: trace-sweep-divergence
Issue-Trace: sso-oidc-start-device-authorization-invalid-request-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-oidc-start-device-authorization-invalid-request-error sso-oidc-register-client-invalid-metadata-error sso-oidc-register-client-with-scopes-and-redirects sso-oidc-register-public-client-basic
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: oidc|StartDeviceAuthorization|4xx->2xx|$.error
Issue-Key: oidc|StartDeviceAuthorization|4xx->2xx|$.error_description
Issue-Key: oidc|StartDeviceAuthorization|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: oidc|StartDeviceAuthorization|4xx->2xx|status
Issue-Key: oidc|RegisterClient|4xx->4xx|$.error_description
Issue-Key: oidc|RegisterClient|2xx->2xx|$.authorizationEndpoint
Issue-Key: oidc|RegisterClient|2xx->2xx|$.tokenEndpoint
Issue-Key: oidc|RegisterClient|2xx->2xx|$.clientSecret
Issue-Key: oidc|CreateToken|4xx->4xx|$.error_description
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sso-oidc responses diverge from AWS: error_description text, extra RegisterClient endpoint members, missing StartDeviceAuthorization validation

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeviceAuthorization

## Description
Replaying the simparity-wire-aws-v1 SSO OIDC fixtures against the emulator produced 5 mismatched steps out of 6 (score 16.7%). They reduce to FOUR independent causes in the `oidc` service, all in the restJson1 handlers of service-com-amazonaws-ssooidc-awsssooidcservice. A fixer must close ALL FOUR before submitting.

The common shape of causes 1, 3 and 4: AWS SSO OIDC's `error_description` is a FIXED, short, sentence-case English string per error class, with no request-derived parts. This emulator instead synthesises its own diagnostic prose (and interpolates request values into it), so no client that matches on AWS's text sees what AWS says. Cause 2 is an output-shape defect.

=== Cause 1 of 4: RegisterClient InvalidClientMetadataException error_description is emulator prose, not AWS's fixed text ===
AWS: RegisterClient rejects any `clientType` other than `public` (Smithy: RegisterClientRequest$clientType is @required; its doc states "The service supports only public as a client type. Anything other than public will be rejected by the service"), with error InvalidClientMetadataException, HTTP 400, body member `error` = "invalid_client_metadata" and `error_description` = "Invalid client type provided". The recorded AWS response gives exactly that text: the whole string is FIXED text for this error class — it contains no part taken from the request (the rejected clientType value is NOT echoed).
Emulator: answers HTTP 400 with `error` = "invalid_client_metadata" (correct) but `error_description` = "clientType must be '<supported>', not '<rejected>'" — a template that interpolates the supported and the rejected clientType from the request.
Why: a client or test that asserts on AWS's `error_description` for an unsupported client type never matches, and a human reading the emulator's reply is told a different (and more specific) contract than AWS states.
Fix: the RegisterClient handler's clientType validation branch — emit the fixed AWS text instead of a formatted string.
Traces: sso-oidc-register-client-invalid-metadata-error#task#1 (RegisterClient).

=== Cause 2 of 4: RegisterClient always emits authorizationEndpoint and tokenEndpoint, which AWS omits ===
AWS: RegisterClientResponse$authorizationEndpoint and $tokenEndpoint are OPTIONAL members (Smithy: RegisterClientResponse, members clientId, clientSecret, clientIdIssuedAt, clientSecretExpiresAt, authorizationEndpoint (URI), tokenEndpoint (URI) — only the two timestamps carry defaults; neither endpoint is @required). In both recorded RegisterClient successes — the basic public-client registration and the one that passes `scopes` and `redirectUris` — AWS returned HTTP 200 with NEITHER member present; recorded behavior only as to the exact condition under which AWS does populate them. Hypothesis: AWS only returns them for a registration configured for the authorization-code flow (one carrying `issuerUrl`/`grantTypes` naming authorization_code), which neither recorded request is.
Emulator: unconditionally populates both on every successful RegisterClient, derived from the request's region or the supplied issuerUrl, so the response carries two members AWS left absent.
Why: a client that treats a present `tokenEndpoint`/`authorizationEndpoint` as the signal that the registration supports the authorization-code flow (and uses those URLs instead of its configured endpoint) takes a code path AWS never puts it on, and round-trip/shape assertions on the response see extra members.
Fix: the RegisterClient handler's output construction — populate the two endpoint members only under the condition that makes them meaningful, and leave them None otherwise.
Traces: sso-oidc-register-client-with-scopes-and-redirects#task#1 and sso-oidc-register-public-client-basic#task#1 (both RegisterClient, both 200->200 with $.authorizationEndpoint and $.tokenEndpoint type_mismatch expected None).
NOTE for the fixer: the second of those two clusters also reports a `$.clientSecret` value mismatch against the fixture's `{{client_secret_basic}}` capture. That is a freshly generated per-run secret, NOT a requirement, and is deliberately excluded from this issue — do not try to make the emulator mint a specific secret.

=== Cause 3 of 4: CreateToken InvalidGrantException error_description for an unknown deviceCode is emulator prose ===
AWS: CreateToken with grantType urn:ietf:params:oauth:grant-type:device_code and a `deviceCode` that names no device authorization fails with InvalidGrantException (Smithy: CreateToken errors include InvalidGrantException, httpError 400; its doc: "Indicates that a request contains an invalid grant"), HTTP 400, `error` = "invalid_grant", `error_description` = "Invalid device code provided". The recorded response gives exactly that text; it is entirely FIXED text for this class and does NOT echo the rejected deviceCode (which is correct — the code is a credential).
Emulator: answers HTTP 400 with `error` = "invalid_grant" (correct) and `error_description` = "the deviceCode does not name a device authorization of this service".
Why: a device-flow client distinguishing invalid_grant causes by AWS's description text (a common pattern in device-code polling loops) never matches, so the emulator's negative path is unusable for that logic.
Fix: the CreateToken handler's grant-resolution path (the invalid_grant helper's call site for the device-code branch) — emit AWS's fixed text for this error class.
Traces: sso-oidc-register-client-with-scopes-and-redirects#task#2 (CreateToken).

=== Cause 4 of 4: StartDeviceAuthorization accepts an invalid request and answers 200 instead of 400 invalid_request ===
AWS: StartDeviceAuthorization validates its input and rejects a malformed one with InvalidRequestException (Smithy: StartDeviceAuthorization errors include InvalidRequestException, httpError 400; doc: "Indicates that something is wrong with the input to the request. For example, a required parameter might be missing or out of range"), HTTP 400, `error` = "invalid_request", `error_description` = "Invalid request" — both wholly FIXED text for this class, no request-derived part — plus the restJson1 error-shape header `x-amzn-errortype` naming the error. That is what the recording shows for this request.
Emulator: answers HTTP 200 with a full device authorization (deviceCode, userCode, verificationUri, verificationUriComplete, expiresIn, interval) and NO `error`, `error_description` or `x-amzn-errortype` at all. The handler's only input check is an absolute-URI test on `startUrl`; every other malformed input (notably a `startUrl` that is a syntactically valid absolute URI but not an IAM Identity Center access-portal URL) sails through and mints state.
Why: this is the worst of the four — a negative-path call SUCCEEDS. A client exercising its error handling gets a usable deviceCode/userCode, polls CreateToken against an authorization AWS would never have created, and any test asserting the 400 passes vacuously against the emulator; the emulator also accumulates device-authorization state for requests AWS refuses.
Fix: StartDeviceAuthorization's input validation, ahead of state mutation — widen it beyond the bare absolute-URI test so a malformed request raises InvalidRequestException with AWS's fixed `error`/`error_description`, and ensure the error is serialized with the `x-amzn-errortype` header.
Traces: sso-oidc-start-device-authorization-invalid-request-error#task#2 (StartDeviceAuthorization) — the representative of this issue.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, steps_total 6, steps_passed 1, steps_mismatched 5, score_pct 16.7, divergence_count 5. All five carry service "oidc", empty stub_path and empty owned_by.

Cause 1 — fixture sso-oidc-register-client-invalid-metadata-error, step task#1, RegisterClient, status 400 -> 400:
  $.error_description: expected 'Invalid client type provided', got "clientType must be 'public', not 'confidential'"

Cause 2a — fixture sso-oidc-register-client-with-scopes-and-redirects, step task#1, RegisterClient, status 200 -> 200:
  $.authorizationEndpoint: expected None (member absent), got 'https://oidc.<region>.amazonaws.com/authorize'
  $.tokenEndpoint: expected None (member absent), got 'https://oidc.<region>.amazonaws.com/token'
Cause 2b — fixture sso-oidc-register-public-client-basic, step task#1, RegisterClient, status 200 -> 200:
  $.authorizationEndpoint: expected None (member absent), got 'https://oidc.<region>.amazonaws.com/authorize'
  $.tokenEndpoint: expected None (member absent), got 'https://oidc.<region>.amazonaws.com/token'
  ($.clientSecret also differed — fixture capture {{client_secret_basic}} vs a freshly minted 64-hex secret. Per-run generated value, NOT part of this finding.)

Cause 3 — fixture sso-oidc-register-client-with-scopes-and-redirects, step task#2, CreateToken, status 400 -> 400:
  $.error_description: expected 'Invalid device code provided', got 'the deviceCode does not name a device authorization of this service'

Cause 4 — fixture sso-oidc-start-device-authorization-invalid-request-error, step task#2, StartDeviceAuthorization, status expected 400, got 200:
  $.error: expected 'invalid_request', got None
  $.error_description: expected 'Invalid request', got None
  $headers.x-amzn-errortype: expected present (shape asserted), got None

Model references used: Smithy model services/service-com-amazonaws-ssooidc-awsssooidcservice/smithy-model/com-amazonaws-ssooidc-awsssooidcservice.json — sdkId "SSO OIDC", protocol restJson1; RegisterClient POST /client/register errors {InvalidClientMetadataException 400, InvalidRedirectUriException 400, InvalidRequestException 400, InvalidScopeException 400, SlowDownException 400, UnsupportedGrantTypeException 400, InternalServerException 500}; RegisterClientResponse members authorizationEndpoint/tokenEndpoint optional (target URI); CreateToken POST /token error InvalidGrantException 400 with members {error, error_description}; StartDeviceAuthorization POST /device_authorization error InvalidRequestException 400 with members {error, reason, error_description}.

Emulator code locations backing the "Emulator:" lines: services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/register_client.rs lines 200-212 (clientType message) and 280-287 (unconditional authorization_endpoint/token_endpoint); src/ops/create_token.rs lines 260-264 (unknown deviceCode message) with the invalid_grant helper at 195-201; src/ops/start_device_authorization.rs lines 187-200 (the sole startUrl check) with src/state.rs lines 692-696 (is_absolute_uri).

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from the checkout (the SSO OIDC service is bootstrapped — its bootstrap issue 20261021T215310Z-bootstrap-service-com-amazonaws-ssooidc-awsssooidcservice is resolved).
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Cause 4 (representative): replay fixture sso-oidc-start-device-authorization-invalid-request-error step task#2 — StartDeviceAuthorization returns HTTP 200 with a minted deviceCode/userCode where the recording has HTTP 400, error 'invalid_request', error_description 'Invalid request' and an x-amzn-errortype header.
4. Cause 1: replay sso-oidc-register-client-invalid-metadata-error step task#1 — POST /client/register with a clientType other than 'public'; compare $.error_description against 'Invalid client type provided'.
5. Cause 2: replay sso-oidc-register-public-client-basic step task#1 and sso-oidc-register-client-with-scopes-and-redirects step task#1 — a successful RegisterClient; assert $.authorizationEndpoint and $.tokenEndpoint are ABSENT from the 200 body. Ignore $.clientSecret (per-run generated value).
6. Cause 3: replay sso-oidc-register-client-with-scopes-and-redirects step task#2 — CreateToken with grantType urn:ietf:params:oauth:grant-type:device_code and a deviceCode the service never issued; compare $.error_description against 'Invalid device code provided'.
7. All four causes must pass before the issue is closed; re-replay every fixture/step listed above, not just the representative.
