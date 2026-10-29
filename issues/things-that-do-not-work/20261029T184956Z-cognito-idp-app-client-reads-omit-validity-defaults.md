Issue-Source: trace-sweep-divergence
Issue-Trace: cip-cfg-revoke-token-disabled-on-client
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-cfg-revoke-token-disabled-on-client cognito-idp-account-recovery-and-attribute-verification cognito-idp-admin-password-reset-and-user-settings cognito-idp-admin-user-auth-workflow cognito-idp-admin-user-core-lifecycle cognito-idp-admin-user-signup-confirm-and-mfa cognito-idp-app-client-crud-and-secret-lifecycle cognito-idp-app-client-error-paths cognito-idp-app-client-secret-generated-at-creation cognito-idp-app-client-secret-lifecycle cognito-idp-attribute-management-edge-cases-and-invalid-input cognito-idp-auth-challenge-and-token-refresh cognito-idp-auth-event-feedback-requires-advanced-security-addon cognito-idp-client-credentials-m2m-flow cognito-idp-create-client-omits-unset-optional-blocks cognito-idp-device-and-pool-diagnostics cognito-idp-m2m-client-credentials-token cognito-idp-managed-login-branding-crud cognito-idp-managed-login-branding-lifecycle cognito-idp-mfa-setup-and-preference-workflow cognito-idp-pool-schema-and-ui-customization cognito-idp-resource-server-and-oauth-scope-enforcement cognito-idp-resource-server-and-scoped-client cognito-idp-risk-configuration-lifecycle cognito-idp-self-service-account-deletion-lifecycle cognito-idp-self-service-mfa-preference-and-auth-factors cognito-idp-self-service-password-challenge-and-token-lifecycle cognito-idp-self-service-profile-view-update-and-attribute-verification cognito-idp-self-service-signup-confirm-and-forgot-password cognito-idp-terms-document-lifecycle cognito-idp-terms-documents-error-paths cognito-idp-terms-documents-lifecycle cognito-idp-terms-lifecycle-create-describe-update-delete cognito-idp-terms-not-found-errors cognito-idp-terms-two-types-per-client cognito-idp-ui-customization-lifecycle cognito-idp-user-device-management-errors cognito-idp-user-mfa-settings-and-refresh-token-exchange cognito-idp-user-pool-and-client-lifecycle cognito-idp-user-pool-client-lifecycle cognito-idp-user-pool-client-secret-management cognito-idp-user-session-mfa-and-refresh cognito-idp-webauthn-credential-management cip-autherr-flow-not-enabled-admin-user-password cip-autherr-flow-not-enabled-user-password cip-autherr-flow-not-enabled-user-srp cognito-idp-update-user-pool-client-callback-urls-replaced cognito-idp-admin-auth-challenge-flow cognito-idp-user-settings-and-mfa cip-autherr-enabled-ghost-not-authorized cip-autherr-forgot-password-ghost-enabled-fabricated cip-autherr-legacy-ghost-user-not-found cognito-idp-self-service-signup-and-token-session cognito-idp-user-pool-client-domain-and-replica-configuration cognito-idp-update-user-pool-client-refresh-token-validity-reflected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|CreateUserPoolClient|2xx->2xx|$.UserPoolClient.AuthSessionValidity
Issue-Key: cognito-idp|CreateUserPoolClient|2xx->2xx|$.UserPoolClient.RefreshTokenValidity
Issue-Key: cognito-idp|CreateUserPoolClient|2xx->2xx|$.UserPoolClient.TokenValidityUnits
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.AuthSessionValidity
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.RefreshTokenValidity
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.TokenValidityUnits
Issue-Key: cognito-idp|UpdateUserPoolClient|2xx->2xx|$.UserPoolClient.AuthSessionValidity
Issue-Key: cognito-idp|UpdateUserPoolClient|2xx->2xx|$.UserPoolClient.RefreshTokenValidity
Issue-Key: cognito-idp|UpdateUserPoolClient|2xx->2xx|$.UserPoolClient.TokenValidityUnits
Issue-Key: cognito-idp|CreateUserPoolClient|2xx->2xx|$.UserPoolClient.ExplicitAuthFlows[*]
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.ExplicitAuthFlows[*]
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.AllowedOAuthScopes[*]
Issue-Key: cognito-idp|UpdateUserPoolClient|2xx->2xx|$.UserPoolClient.ExplicitAuthFlows[*]
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: app client reads omit validity defaults and reorder request lists

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolClient

## Description
AWS: the UserPoolClientType returned by CreateUserPoolClient, DescribeUserPoolClient and UpdateUserPoolClient carries the server-applied token-validity defaults, and returns the request's list members in the order the request supplied them.
Emulator: AuthSessionValidity and TokenValidityUnits are absent from every app-client response, RefreshTokenValidity is reported as the raw modeled default 0, and ExplicitAuthFlows / AllowedOAuthScopes come back in a different order than the request supplied.
Why: a client that reads the app client to decide session handling sees no AuthSessionValidity at all, and sees RefreshTokenValidity 0 — which AWS documents as 'cannot be set', since it overrides 0 with 30 days. Code that computes a refresh deadline from that value computes zero lifetime and re-authenticates on every call. Reordered ExplicitAuthFlows/AllowedOAuthScopes break exact-match assertions in IaC drift detection and in any configuration-diffing client.
Fix: CreateUserPoolClient/UpdateUserPoolClient state + the UserPoolClientType serializer — materialise the server defaults at write time and project them on read; store request lists as ordered sequences rather than sets.

Cause 1 of 10: CreateUserPoolClient omits AuthSessionValidity. AWS returns the default 3 (Smithy: AuthSessionValidity @range min 3, max 15, in minutes). Fixture cip-cfg-revoke-token-disabled-on-client, step task#4 (43 recorded steps in this cluster).
Cause 2 of 10: CreateUserPoolClient omits TokenValidityUnits; AWS always returns the block, empty `{}`, when no units were set. Same representative step.
Cause 3 of 10: CreateUserPoolClient reports RefreshTokenValidity 0; AWS returns 30. Smithy: RefreshTokenValidity default 0, and its own documentation states "You can't set RefreshTokenValidity to 0. If you do, Amazon Cognito overrides the value with the default value of 30 days" — so the wire value for an unset refresh validity is 30, never 0. Same representative step.
Cause 4 of 10: DescribeUserPoolClient omits AuthSessionValidity/TokenValidityUnits and reports RefreshTokenValidity 0. Fixture cip-autherr-flow-not-enabled-admin-user-password, step task#5.
Cause 5 of 10: UpdateUserPoolClient omits AuthSessionValidity/TokenValidityUnits and reports RefreshTokenValidity 0. Fixture cognito-idp-app-client-crud-and-secret-lifecycle, step task#4.
Cause 6 of 10: CreateUserPoolClient returns ExplicitAuthFlows in a different order than the request. Fixture cognito-idp-admin-auth-challenge-flow, step setup#2 (expected ['ALLOW_USER_PASSWORD_AUTH','ALLOW_ADMIN_USER_PASSWORD_AUTH'], got them swapped).
Cause 7 of 10: DescribeUserPoolClient returns ExplicitAuthFlows in a different order than the request. Fixture cip-autherr-enabled-ghost-not-authorized, step task#6.
Cause 8 of 10: DescribeUserPoolClient returns AllowedOAuthScopes in a different order than the request. Fixture cognito-idp-app-client-crud-and-secret-lifecycle, step task#3 (expected ['email','openid'], got ['openid','email']).
Cause 9 of 10: the same reordering on CreateUserPoolClient with three flows. Fixture cognito-idp-self-service-signup-and-token-session, step task#2.
Cause 10 of 10: the same reordering plus missing validity defaults on the Describe/Update pair of a domain-configured client, and on an Update that sets RefreshTokenValidity explicitly (AuthSessionValidity/TokenValidityUnits still absent). Fixtures cognito-idp-user-pool-client-domain-and-replica-configuration steps task#3 and task#4, and cognito-idp-update-user-pool-client-refresh-token-validity-reflected step task#3.

## Evidence
CreateUserPoolClient (cip-cfg-revoke-token-disabled-on-client#task#4, 200 both sides):
  $.UserPoolClient.AuthSessionValidity: expected 3, got None
  $.UserPoolClient.TokenValidityUnits: expected {}, got None
  $.UserPoolClient.RefreshTokenValidity: expected 30, got 0
CreateUserPoolClient (cognito-idp-admin-auth-challenge-flow#setup#2):
  $.UserPoolClient.ExplicitAuthFlows[0]: expected 'ALLOW_USER_PASSWORD_AUTH', got 'ALLOW_ADMIN_USER_PASSWORD_AUTH'
  $.UserPoolClient.ExplicitAuthFlows[1]: expected 'ALLOW_ADMIN_USER_PASSWORD_AUTH', got 'ALLOW_USER_PASSWORD_AUTH'
DescribeUserPoolClient (cognito-idp-app-client-crud-and-secret-lifecycle#task#3):
  $.UserPoolClient.AllowedOAuthScopes[0]: expected 'email', got 'openid'
  $.UserPoolClient.AllowedOAuthScopes[1]: expected 'openid', got 'email'

## Steps to Reproduce
1. Replay cip-cfg-revoke-token-disabled-on-client step task#4 (CreateUserPoolClient without token-validity settings) and inspect AuthSessionValidity, TokenValidityUnits and RefreshTokenValidity.
2. Replay cognito-idp-admin-auth-challenge-flow step setup#2 and compare ExplicitAuthFlows order with the request.
3. Replay cognito-idp-app-client-crud-and-secret-lifecycle steps task#3/task#4 and cognito-idp-user-pool-client-domain-and-replica-configuration steps task#3/task#4.
