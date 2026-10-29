Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-self-service-signup-and-token-session
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-self-service-signup-and-token-session cognito-idp-self-service-token-refresh-and-revocation cognito-idp-federated-provider-user-linking cognito-idp-advanced-security-not-enabled cip-chal-mfa-setup-required cognito-idp-app-client-crud-and-secret-lifecycle cognito-idp-app-client-secret-lifecycle cognito-idp-client-secret-error-paths cognito-idp-client-secret-limit-exceeded cognito-idp-user-pool-client-secret-rotation cognito-idp-self-service-mfa-preference-and-auth-factors cip-autherr-forgot-password-ghost-enabled-fabricated cip-autherr-forgot-password-ghost-legacy-not-found
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|GetUser|4xx->2xx|$.__type
Issue-Key: cognito-idp|GetUser|4xx->2xx|$.message
Issue-Key: cognito-idp|GetUser|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetUser|4xx->2xx|status
Issue-Key: cognito-idp|GetTokensFromRefreshToken|4xx->2xx|$.__type
Issue-Key: cognito-idp|GetTokensFromRefreshToken|4xx->2xx|$.message
Issue-Key: cognito-idp|GetTokensFromRefreshToken|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetTokensFromRefreshToken|4xx->2xx|status
Issue-Key: cognito-idp|AdminLinkProviderForUser|4xx->2xx|$
Issue-Key: cognito-idp|AdminLinkProviderForUser|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminLinkProviderForUser|4xx->2xx|status
Issue-Key: cognito-idp|SetRiskConfiguration|2xx->4xx|$.RiskConfiguration
Issue-Key: cognito-idp|SetRiskConfiguration|2xx->4xx|status
Issue-Key: cognito-idp|DescribeRiskConfiguration|2xx->2xx|$
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.ChallengeName
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.Session
Issue-Key: cognito-idp|ListUserPoolClientSecrets|2xx->2xx|$.ClientSecrets[*]
Issue-Key: cognito-idp|GetUserAuthFactors|2xx->2xx|$.ConfiguredUserAuthFactors[*]
Issue-Key: cognito-idp|GetUserAuthFactors|2xx->2xx|$.PreferredMfaSetting
Issue-Key: cognito-idp|GetUserAuthFactors|2xx->2xx|$.UserMFASettingList
Issue-Key: cognito-idp|ForgotPassword|2xx->4xx|$.CodeDeliveryDetails
Issue-Key: cognito-idp|ForgotPassword|2xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-authentication-success-responses-omit-challengeparameters-and.md
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: revocation, provider linking, risk and MFA state are not honored

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUser

## Description
Ten recorded causes in which the emulator's stored state is not consulted (or not stored at all) by the operation that is supposed to read it: revoked credentials keep working, a second provider link is accepted, risk configuration is dropped, the MFA_SETUP challenge is never raised, secret listings grow, and the auth-factor read is incomplete.

Cause 1 of 10: GetUser accepts an access token whose session was revoked. AWS: 400 NotAuthorizedException, message `Access Token has been revoked` (fixed text, no variable part). Emulator: 200 with the user profile. Why: a client that signs a user out and relies on the token being dead keeps serving that session. Fix: GetUser authorization must check the revocation state the revoke path writes. Fixture cognito-idp-self-service-signup-and-token-session, step task#15.
Cause 2 of 10: GetTokensFromRefreshToken accepts a revoked refresh token. AWS: 400 NotAuthorizedException, message `Refresh Token has been revoked`. Emulator: 200 with a fresh token set. Fix: the refresh-exchange handler must check the revocation state. Fixture cognito-idp-self-service-token-refresh-and-revocation, step task#10.
Cause 3 of 10: AdminLinkProviderForUser accepts linking a source user that is already linked. AWS: 400 InvalidParameterException, message `SourceUser is already linked to DestinationUser`. Emulator: 200 with an empty body. Fix: AdminLinkProviderForUser must reject when the identity already carries a link. Fixture cognito-idp-federated-provider-user-linking, step task#5.
Cause 4 of 10: SetRiskConfiguration is refused 400 on a pool without the advanced-security add-on, although AWS accepts it and returns the stored RiskConfiguration (CompromisedCredentialsRiskConfiguration with Actions.EventAction, plus UserPoolId). Emulator: 400 with no RiskConfiguration. Fix: remove the add-on gate from SetRiskConfiguration and persist the configuration. Fixture cognito-idp-advanced-security-not-enabled, step task#4.
Cause 5 of 10: DescribeRiskConfiguration answers 200 with an empty body where AWS returns the previously stored RiskConfiguration. Fix: DescribeRiskConfiguration must project the stored risk state. Fixture cognito-idp-advanced-security-not-enabled, step task#5.
Cause 6 of 10: InitiateAuth on a pool with MfaConfiguration ON and no configured method never issues the MFA_SETUP challenge. AWS returns 200 with ChallengeName `MFA_SETUP`, a Session, and ChallengeParameters carrying `MFAS_CAN_SETUP` (a JSON array of the methods available for setup) and `USER_ID_FOR_SRP` (the username). Emulator: 200 with none of the three. Fix: the challenge state machine in the InitiateAuth handler. Fixture cip-chal-mfa-setup-required, step task#7.
Cause 7 of 10: ListUserPoolClientSecrets returns an extra secret entry after a rotation; AWS lists one. Fix: the client-secret state — rotation must replace rather than accumulate. Fixture cognito-idp-app-client-crud-and-secret-lifecycle, step task#9 (5 recorded steps).
Cause 8 of 10: GetUserAuthFactors returns ConfiguredUserAuthFactors in the wrong order/content — the second entry is SMS_OTP where AWS reports EMAIL_OTP. Fix: GetUserAuthFactors projection must derive the factor list from the user's configured factors in the service's order. Fixture cognito-idp-self-service-mfa-preference-and-auth-factors, step task#10.
Cause 9 of 10: GetUserAuthFactors omits the rest of the factor list and omits PreferredMfaSetting and UserMFASettingList entirely after an MFA preference is set. AWS returns PreferredMfaSetting `SMS_MFA` and UserMFASettingList `['SMS_MFA']` alongside the full ConfiguredUserAuthFactors. Fix: GetUserAuthFactors must read the user's MFA preference state. Fixture cognito-idp-self-service-mfa-preference-and-auth-factors, step task#12.
Cause 10 of 10: ForgotPassword for an existing, enabled user answers 403 instead of 200 with CodeDeliveryDetails (AttributeName `email`, DeliveryMedium `EMAIL`, masked Destination). Fix: the ForgotPassword handler must resolve the user and deliver, not refuse. Fixture cip-autherr-forgot-password-ghost-enabled-fabricated, step task#4 (3 recorded steps).

Related open issues that do NOT carry these replay keys and so do not cover them: 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md, 20261029T182116Z-cognito-idp-provisioned-limit-api-diverges-no-default.md, 20261029T182116Z-cognito-idp-stored-state-diverges-empty-absent-normalization.md.

## Evidence
GetUser after revocation (cognito-idp-self-service-signup-and-token-session#task#15): status expected 400 got 200; $.__type expected 'NotAuthorizedException' got None; $.message expected 'Access Token has been revoked' got None.
GetTokensFromRefreshToken after revocation (cognito-idp-self-service-token-refresh-and-revocation#task#10): expected 400 'Refresh Token has been revoked', got 200.
AdminLinkProviderForUser (cognito-idp-federated-provider-user-linking#task#5): expected 400 InvalidParameterException 'SourceUser is already linked to DestinationUser', got 200 with {}.
SetRiskConfiguration (cognito-idp-advanced-security-not-enabled#task#4): status expected 200 got 400; $.RiskConfiguration missing.
DescribeRiskConfiguration (cognito-idp-advanced-security-not-enabled#task#5): $: empty_body — expected the stored RiskConfiguration, got {}.
InitiateAuth (cip-chal-mfa-setup-required#task#7): $.ChallengeName expected 'MFA_SETUP' got None; $.ChallengeParameters expected MFAS_CAN_SETUP/USER_ID_FOR_SRP got None; $.Session missing.
ListUserPoolClientSecrets (cognito-idp-app-client-crud-and-secret-lifecycle#task#9): $.ClientSecrets[1]: extra entry (expected None).
GetUserAuthFactors (cognito-idp-self-service-mfa-preference-and-auth-factors#task#10/#task#12): ConfiguredUserAuthFactors[1] expected 'EMAIL_OTP' got 'SMS_OTP'; PreferredMfaSetting expected 'SMS_MFA' got None; UserMFASettingList expected ['SMS_MFA'] got None.
ForgotPassword (cip-autherr-forgot-password-ghost-enabled-fabricated#task#4): status expected 200 got 403; $.CodeDeliveryDetails missing.

## Steps to Reproduce
1. Replay cognito-idp-self-service-signup-and-token-session step task#15 and cognito-idp-self-service-token-refresh-and-revocation step task#10 — revoked credentials still authorize.
2. Replay cognito-idp-advanced-security-not-enabled steps task#4 and task#5.
3. Replay cip-chal-mfa-setup-required step task#7 and inspect ChallengeName.
4. Replay the remaining fixture#step pairs named in the causes.
