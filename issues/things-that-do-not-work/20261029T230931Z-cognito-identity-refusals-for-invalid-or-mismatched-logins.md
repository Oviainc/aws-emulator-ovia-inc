Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-login-user-pool-token-link-and-unlink
Issue-Step: task#16
Issue-Severity: CRITICAL
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-login-user-pool-token-link-and-unlink cognito-identity-get-openid-token-error-lifecycle cogid-login-hide-disabled-after-unlink-and-relink cognito-identity-unauthenticated-guest-identity-flow cogid-creds-custom-role-arn-outside-mappings-rejected cogid-creds-developer-authenticated-credentials-envelope cognito-identity-unlink-identity-login-management cogid-merge-idp-proven-identity-login-conflicts cognito-identity-developer-authenticated-identity-flow cogid-merge-developer-login-of-other-identity-refused cogid-merge-getid-blank-absorbed-by-registered-developer-login
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetId|4xx->2xx|$.__type
Issue-Key: cognito-identity|GetId|4xx->2xx|$.message
Issue-Key: cognito-identity|GetId|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetId|4xx->2xx|status
Issue-Key: cognito-identity|GetOpenIdToken|4xx->2xx|$.__type
Issue-Key: cognito-identity|GetOpenIdToken|4xx->2xx|$.message
Issue-Key: cognito-identity|GetOpenIdToken|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetOpenIdToken|4xx->2xx|status
Issue-Key: cognito-identity|GetId|4xx->4xx|$.message
Issue-Key: cognito-identity|GetId|4xx->4xx|status
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->2xx|$.__type
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->2xx|$.message
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->2xx|status
Issue-Key: cognito-identity|UnlinkIdentity|4xx->2xx|$
Issue-Key: cognito-identity|UnlinkIdentity|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|UnlinkIdentity|4xx->2xx|status
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->2xx|$.__type
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->2xx|$.message
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->2xx|status
Issue-Key: cognito-identity|LookupDeveloperIdentity|4xx->2xx|$.__type
Issue-Key: cognito-identity|LookupDeveloperIdentity|4xx->2xx|$.message
Issue-Key: cognito-identity|LookupDeveloperIdentity|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|LookupDeveloperIdentity|4xx->2xx|status
Issue-Key: cognito-identity|UnlinkIdentity|4xx->4xx|$.__type
Issue-Key: cognito-identity|UnlinkIdentity|4xx->4xx|$.message
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->4xx|$.__type
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->4xx|$.__type
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->4xx|$.message
Issue-Related: 20261029T230931Z-cognito-identity-constraint-violation-refusals-are-re-worded.md
Issue-Related: 20261029T230931Z-cognito-identity-error-responses-omit-the-x-amzn.md
Issue-Related: 20261029T230931Z-cognito-identity-identity-scoped-refusals-answer-403-404.md
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: refusals for invalid or mismatched logins are missing or mis-classified

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetId

## Description
This issue bundles 10 root causes on the identity/credential operations where the emulator either ACCEPTS a request AWS refuses (causes 1-5, security-relevant) or refuses it with the wrong error class and message (causes 6-10). Smithy: GetId, GetOpenIdToken, UnlinkIdentity and GetCredentialsForIdentity are @optionalAuth operations whose modeled errors include NotAuthorizedException, ResourceConflictException, InvalidParameterException and (for GetCredentialsForIdentity) InvalidIdentityPoolConfigurationException; the recorded AWS responses are HTTP 400 carrying `__type` plus `x-amzn-errortype`. Fix: the login-token validation and authorization checks in each named operation's handler (which login is acceptable for which identity), and the error selection those checks feed.

Cause 1 of 10: GetId with a login token that is not a valid OpenID Connect identity token. AWS: 400, `__type` NotAuthorizedException, message fixed text 'Invalid login token. Not a valid OpenId Connect identity token.' Emulator: HTTP 200 — it mints an identity for an unvalidatable token. Why: a caller with a forged/garbage token obtains a pool identity and, through it, credentials. Fix: token validation in the GetId handler. Traces: cogid-login-user-pool-token-link-and-unlink#task#16.

Cause 2 of 10: GetOpenIdToken with Logins that do not match the named identity. AWS: 400, NotAuthorizedException, message fixed text "Logins don't match. Please include at least one valid login for this identity or identity pool." Emulator: 200 with a token. Why: one identity's token can be obtained with another identity's login. Fix: the logins-vs-identity check in the GetOpenIdToken handler. Traces: cognito-identity-get-openid-token-error-lifecycle#task#6.

Cause 3 of 10: GetId with a login whose issuer does not match the provider name it is presented under. AWS: 400, message fixed text "Invalid login token. Issuer doesn't match providerName". Emulator: 403 with 'Token is not from a supported provider of this identity pool.' — wrong status and a different reason (it rejects on provider support rather than on issuer/providerName mismatch). Why: the refusal reason a client reads is wrong, and the status is not AWS's. Fix: issuer-vs-providerName validation in GetId. Traces: cogid-login-hide-disabled-after-unlink-and-relink#task#16 (2 steps).

Cause 4 of 10: GetCredentialsForIdentity with a CustomRoleArn that is not among the pool's role mappings. AWS: 400, `__type` InvalidParameterException, message fixed text 'Only SAML providers and providers with RoleMappings support custom role ARN.' Emulator: HTTP 200 — it vends credentials for an unmapped role ARN. Why: a caller escalates to any role ARN it names. Fix: the CustomRoleArn validation in the GetCredentialsForIdentity handler. Traces: cogid-creds-custom-role-arn-outside-mappings-rejected#task#6 (2 steps).

Cause 5 of 10: UnlinkIdentity for an unauthenticated (guest) identity. AWS: 400, NotAuthorizedException, message fixed text 'Cannot unlink unauthenticated user.' Emulator: HTTP 200 with an empty body — the unlink is performed. Why: guest identities can be mutated by anyone holding the id. Fix: the UnlinkIdentity handler's unauthenticated-identity guard. Traces: cognito-identity-unlink-identity-login-management#task#4.

Cause 6 of 10: GetOpenIdTokenForDeveloperIdentity whose Logins would link a developer user to logins already proven for another identity. AWS: 400, `__type` ResourceConflictException, message fixed text 'Cannot link these logins.' Emulator: HTTP 200 — the conflicting link is created. Fix: the link-conflict check in the GetOpenIdTokenForDeveloperIdentity handler. Traces: cogid-merge-idp-proven-identity-login-conflicts#task#13 (2 steps).

Cause 7 of 10: LookupDeveloperIdentity for an identity the caller's logins do not own. AWS: 400, NotAuthorizedException, message fixed text 'You do not have access to this identity'. Emulator: HTTP 200 — it answers the lookup. Why: identity enumeration across identities. Fix: the ownership check in the LookupDeveloperIdentity handler. Traces: cogid-merge-idp-proven-identity-login-conflicts#task#14 (2 steps).

Cause 8 of 10: UnlinkIdentity naming an identity the supplied logins do not own. AWS: 400, `__type` NotAuthorizedException, message fixed text "Access to Identity '" + the IdentityId from the request + "' is forbidden." Emulator: `__type` InvalidParameterException with 'Please provide a valid public provider' — wrong error class and reason. Fix: UnlinkIdentity ownership check and its error selection. Traces: cogid-login-user-pool-token-link-and-unlink#task#12 (2 steps).

Cause 9 of 10: GetOpenIdTokenForDeveloperIdentity for a developer user identifier already registered under a DIFFERENT identity, presented with logins that do not match. AWS: 400, `__type` NotAuthorizedException, message fixed text "Logins don't match. Please include at least one valid login for this identity or identity pool." Emulator: `__type` DeveloperUserAlreadyRegisteredException with "The developer user identifier '<identifier>' is already registered under identity '<identity-id>'." — it reports a registration conflict instead of refusing the caller, and in doing so discloses the other identity's id. Fix: order the checks so the logins/identity authorization check precedes the already-registered check in GetOpenIdTokenForDeveloperIdentity. Traces: cogid-merge-developer-login-of-other-identity-refused#task#5 (2 steps).

Cause 10 of 10: GetCredentialsForIdentity for an identity the caller may not use. AWS: 400, `__type` NotAuthorizedException, message fixed text "Access to Identity '" + the IdentityId from the request + "' is forbidden." Emulator: `__type` InvalidIdentityPoolConfigurationException with 'Invalid identity pool configuration. Check assigned IAM roles for this pool.' — it blames the pool configuration instead of refusing the caller. Fix: the authorization check (and its error selection) in the GetCredentialsForIdentity handler, before the role-resolution path. Traces: cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#11.

Related open issues that do NOT carry these replay keys: '20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md' and '20261029T173827Z-cognito-identity-10-net-new-token-credential-refusal.md'.

## Evidence
Replay of simparity-wire-aws-v1, expected vs actual:
- GetId: expected 400 / NotAuthorizedException / 'Invalid login token. Not a valid OpenId Connect identity token.', got 200 with no error body (cogid-login-user-pool-token-link-and-unlink#task#16).
- GetOpenIdToken: expected 400 / NotAuthorizedException / "Logins don't match. Please include at least one valid login for this identity or identity pool.", got 200 (cognito-identity-get-openid-token-error-lifecycle#task#6).
- GetId: expected 400 / "Invalid login token. Issuer doesn't match providerName", got 403 / 'Token is not from a supported provider of this identity pool.' (cogid-login-hide-disabled-after-unlink-and-relink#task#16, 2 steps).
- GetCredentialsForIdentity: expected 400 / InvalidParameterException / 'Only SAML providers and providers with RoleMappings support custom role ARN.', got 200 (cogid-creds-custom-role-arn-outside-mappings-rejected#task#6, 2 steps).
- UnlinkIdentity: expected 400 / NotAuthorizedException / 'Cannot unlink unauthenticated user.', got 200 with empty body (cognito-identity-unlink-identity-login-management#task#4).
- GetOpenIdTokenForDeveloperIdentity: expected 400 / ResourceConflictException / 'Cannot link these logins.', got 200 (cogid-merge-idp-proven-identity-login-conflicts#task#13, 2 steps).
- LookupDeveloperIdentity: expected 400 / NotAuthorizedException / 'You do not have access to this identity', got 200 (cogid-merge-idp-proven-identity-login-conflicts#task#14, 2 steps).
- UnlinkIdentity: expected NotAuthorizedException / "Access to Identity '<identity-id>' is forbidden.", got InvalidParameterException / 'Please provide a valid public provider' (cogid-login-user-pool-token-link-and-unlink#task#12, 2 steps).
- GetOpenIdTokenForDeveloperIdentity: expected NotAuthorizedException / "Logins don't match. ...", got DeveloperUserAlreadyRegisteredException / "The developer user identifier '<identifier>' is already registered under identity '<identity-id>'." (cogid-merge-developer-login-of-other-identity-refused#task#5, 2 steps).
- GetCredentialsForIdentity: expected NotAuthorizedException / "Access to Identity '<identity-id>' is forbidden.", got InvalidIdentityPoolConfigurationException / 'Invalid identity pool configuration. Check assigned IAM roles for this pool.' (cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#11).
In every cluster the x-amzn-errortype header was also absent (see the separate header issue). Smithy: GetId/GetOpenIdToken/UnlinkIdentity/GetCredentialsForIdentity are optionalAuth with NotAuthorizedException, ResourceConflictException and ResourceNotFoundException modeled; InvalidParameterException is a service-common error.

## Steps to Reproduce
1. Replay cogid-login-user-pool-token-link-and-unlink step task#16: GetId with a login token that is not a valid OIDC identity token -> emulator answers 200 and mints an identity; AWS answers 400 NotAuthorizedException.
2. Replay cognito-identity-get-openid-token-error-lifecycle#task#6 (GetOpenIdToken with non-matching Logins), cogid-creds-custom-role-arn-outside-mappings-rejected#task#6 (CustomRoleArn outside the role mappings), cognito-identity-unlink-identity-login-management#task#4 (UnlinkIdentity on a guest identity) and cogid-merge-idp-proven-identity-login-conflicts#task#13/#task#14 -> each is accepted instead of refused.
3. Replay cogid-login-hide-disabled-after-unlink-and-relink#task#16, cogid-login-user-pool-token-link-and-unlink#task#12, cogid-merge-developer-login-of-other-identity-refused#task#5 and cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#11 and compare __type and message with the recordings.
