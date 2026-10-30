Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-password-reset-and-user-settings
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-password-reset-and-user-settings cognito-idp-auth-event-feedback-requires-advanced-security-addon cognito-idp-mfa-setup-and-preference-workflow cognito-idp-self-service-signup-and-token-session cognito-idp-user-mfa-settings-and-refresh-token-exchange cognito-idp-user-settings-and-mfa cognito-idp-webauthn-credential-management cognito-idp-attribute-management-edge-cases-and-invalid-input cognito-idp-self-service-mfa-preference-and-auth-factors cognito-idp-self-service-profile-view-update-and-attribute-verification cognito-idp-self-service-token-refresh-and-revocation cognito-idp-user-session-mfa-and-refresh cognito-idp-admin-auth-challenge-flow cognito-idp-auth-challenge-and-token-refresh cognito-idp-admin-user-auth-workflow cognito-idp-device-and-pool-diagnostics cip-autherr-admin-reset-password-required cip-autherr-enabled-ghost-not-authorized cip-autherr-enabled-wrong-password-matches-ghost cip-autherr-legacy-ghost-user-not-found cip-chal-mfa-setup-required cognito-idp-account-recovery-and-attribute-verification cognito-idp-self-service-account-deletion-lifecycle cip-cfg-case-insensitive-username-resolution cip-cfg-revoke-token-disabled-on-client cip-cfg-revoke-token-rejects-access-token cognito-idp-user-device-management-errors
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminInitiateAuth|2xx->2xx|$.ChallengeParameters
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.ChallengeParameters
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|2xx->2xx|$.ChallengeParameters
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.ChallengeParameters.userAttributes
Issue-Key: cognito-idp|AdminInitiateAuth|2xx->2xx|$.ChallengeParameters.userAttributes
Issue-Key: cognito-idp|AdminInitiateAuth|2xx->2xx|$.AuthenticationResult.NewDeviceMetadata.DeviceGroupKey
Issue-Key: cognito-idp|AdminInitiateAuth|2xx->2xx|$.AuthenticationResult.RefreshToken
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.AuthenticationResult.NewDeviceMetadata.DeviceGroupKey
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.AuthenticationResult.RefreshToken
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: authentication success responses omit ChallengeParameters and use wrong token shapes

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
AWS: a successful InitiateAuth / AdminInitiateAuth / AdminRespondToAuthChallenge response always carries `ChallengeParameters` — an empty map `{}` when authentication completed and no challenge is outstanding (Smithy: InitiateAuthResponse.ChallengeParameters, target ChallengeParametersType, a map). When a CUSTOM_CHALLENGE is outstanding, the recorded `ChallengeParameters.userAttributes` is `{}` for a user whose only attribute is the service-generated `sub`. The refresh token AWS issues is an opaque credential, not a readable JWT, and NewDeviceMetadata.DeviceGroupKey is a UUID-shaped value.
Emulator: ChallengeParameters is dropped entirely from successful responses, userAttributes leaks the internal `sub`, the refresh token is a signed JWT whose payload is plainly readable, and DeviceGroupKey is a 9-character hex string.
Why: SDK and client code reads `ChallengeParameters` unconditionally after an auth call to decide whether a challenge is pending; a missing key is a null dereference or a 'challenge pending' misread where AWS hands back an empty map. A refresh token that is a decodable JWT means any client (and any log scraper) can read the subject, client id and username out of a credential AWS treats as opaque, so code written against the emulator can come to depend on contents the real service does not expose. A DeviceGroupKey of the wrong shape breaks clients that parse or key on it.
Fix: the InitiateAuth/AdminInitiateAuth/AdminRespondToAuthChallenge handlers and the auth-result serializer, plus the token minting in the auth state (refresh-token format, device-group-key format). Note: this issue covers the response shape only; the existing open issue 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md covers the signing algorithm and JWKS and does not carry these replay keys.
Per-run values deliberately NOT claimed here: the `Session` string and the concrete token/key values differ on every run; only their presence and shape are requirements.

Cause 1 of 10: AdminInitiateAuth success omits `ChallengeParameters: {}`. Fixture cognito-idp-admin-password-reset-and-user-settings, step task#7 (7 recorded steps).
Cause 2 of 10: InitiateAuth success omits `ChallengeParameters: {}`. Fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step setup#5 (6 recorded steps).
Cause 3 of 10: AdminRespondToAuthChallenge success omits `ChallengeParameters: {}`. Fixture cognito-idp-admin-auth-challenge-flow, step task#6.
Cause 4 of 10: InitiateAuth CUSTOM_CHALLENGE returns `ChallengeParameters.userAttributes` containing the user's `sub`; AWS returns `{}` for a user with no other attributes. Fixture cognito-idp-auth-challenge-and-token-refresh, step task#5.
Cause 5 of 10: the same `userAttributes` leak on AdminInitiateAuth. Fixture cognito-idp-auth-challenge-and-token-refresh, step task#7.
Cause 6 of 10: AdminInitiateAuth NewDeviceMetadata.DeviceGroupKey is a 9-char hex string; AWS issues a UUID-shaped key. Fixture cognito-idp-admin-user-auth-workflow, step task#7.
Cause 7 of 10: the same DeviceGroupKey shape on InitiateAuth. Fixture cognito-idp-device-and-pool-diagnostics, step task#5.
Cause 8 of 10: InitiateAuth AuthenticationResult.RefreshToken is a signed JWT with a readable payload; AWS issues an opaque refresh credential. Fixture cip-autherr-admin-reset-password-required, step task#5 (7 recorded steps).
Cause 9 of 10: the same refresh-token shape on AdminInitiateAuth. Fixture cip-cfg-case-insensitive-username-resolution, step task#9.
Cause 10 of 10: AdminInitiateAuth with device tracking returns both the wrong DeviceGroupKey shape and no ChallengeParameters. Fixture cognito-idp-user-device-management-errors, step setup#5.

## Evidence
AdminInitiateAuth (cognito-idp-admin-password-reset-and-user-settings#task#7, 200 both sides): $.ChallengeParameters: expected {}, got None.
InitiateAuth (cognito-idp-auth-challenge-and-token-refresh#task#5): $.ChallengeParameters.userAttributes: expected '{}', got '{"sub":"..."}'.
AdminInitiateAuth (cognito-idp-admin-user-auth-workflow#task#7): $.AuthenticationResult.NewDeviceMetadata.DeviceGroupKey: expected a UUID-shaped value, got '7a2b82d06' (9 hex chars); $.AuthenticationResult.RefreshToken: expected an opaque token, got 'eyJhbGciOiJFUzI1NiIs...' (a decodable JWT with sub/iss/client_id/username/token_use=refresh claims).

## Steps to Reproduce
1. Replay cognito-idp-admin-password-reset-and-user-settings step task#7 and check that the 200 body has no ChallengeParameters key.
2. Replay cognito-idp-auth-challenge-and-token-refresh steps task#5 and task#7 and read ChallengeParameters.userAttributes.
3. Replay cognito-idp-admin-user-auth-workflow step task#7 and cognito-idp-device-and-pool-diagnostics step task#5; base64-decode AuthenticationResult.RefreshToken and inspect NewDeviceMetadata.DeviceGroupKey.
