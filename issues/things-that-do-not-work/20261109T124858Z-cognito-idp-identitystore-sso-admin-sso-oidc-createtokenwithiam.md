Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__identitystore__sso-admin__sso-oidc.create-token-with-iam-jwt-bearer-issues-identity-bound-token cognito-idp__identitystore__sso-admin__sso-oidc.create-token-with-iam-jwt-bearer-unresolved-identity-rejected
Issue-Key: sso-oidc|CreateTokenWithIAM|InvalidGrantException
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# cognito-idp__identitystore__sso-admin__sso-oidc: CreateTokenWithIAM refuses a valid Cognito ID token as a jwt-bearer assertion with InvalidGrantException 'Invalid assertion provided'

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTokenWithIAM

## Description
Cause 1 of 1: the JWT-bearer grant of CreateTokenWithIAM never accepts an assertion, so no trusted-token-issuer exchange can happen.
AWS: Smithy: CreateTokenWithIAM (POST /token?aws_iam=t) takes clientId (the Identity Center application ARN), grantType and assertion, and returns accessToken/tokenType/expiresIn/idToken/issuedTokenType/scope (+awsAdditionalDetails). With grantType='urn:ietf:params:oauth:grant-type:jwt-bearer' and an ID token minted by a registered trusted token issuer (here a Cognito user pool), the call SUCCEEDS and vends an identity-bound token for the Identity Store user the assertion's claims resolve to. InvalidGrantException (httpError 400) is modelled for an assertion that is actually invalid — expired, wrongly signed, replayed, or from an unregistered issuer; the unresolved-identity case has its own distinct refusal rather than the same generic one.
Emulator: every exchange is refused InvalidGrantException with message 'Invalid assertion provided', even for a freshly minted, single-use, correctly-signed Cognito ID token from the pool registered as the application's trusted token issuer. The same refusal is also what comes back for the unresolved-identity scenario, so the two outcomes are indistinguishable.
Why: the whole trusted-identity-propagation path is dead — an application cannot exchange an external OIDC token for an Identity Center token, so neither the positive exchange (identity-bound token whose claims name the Identity Store user) nor the negative contract (an assertion whose identity does not resolve) can be exercised by a client.
Fix: the sso-oidc CreateTokenWithIAM handler's jwt-bearer grant path (assertion verification against the registered trusted token issuer, and the identity resolution against identitystore) in the sso-oidc service crate's ops.
Traces: cognito-idp__identitystore__sso-admin__sso-oidc.create-token-with-iam-jwt-bearer-issues-identity-bound-token, cognito-idp__identitystore__sso-admin__sso-oidc.create-token-with-iam-jwt-bearer-unresolved-identity-rejected.

## Evidence
Both checks fail on the exchange with:
botocore.errorfactory.InvalidGrantException: An error occurred (InvalidGrantException) when calling the CreateTokenWithIAM operation: Invalid assertion provided
The assertion is a freshly minted Cognito ID token obtained from AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) for a pool registered as the application's trusted token issuer; the call is made with clientId=<application ARN>, grantType='urn:ietf:params:oauth:grant-type:jwt-bearer'.

## Steps to Reproduce
1) Ensure an ACTIVE IAM Identity Center instance; create an Identity Store user with an email.
2) Create a Cognito user pool + app client and a confirmed user with that same email; register the pool as a trusted token issuer for an Identity Center application and grant the jwt-bearer grant plus the actor policy.
3) AdminInitiateAuth to mint an ID token, then sso-oidc CreateTokenWithIAM(clientId=<app ARN>, grantType='urn:ietf:params:oauth:grant-type:jwt-bearer', assertion=<ID token>).
4) AWS vends an identity-bound token; the emulator answers InvalidGrantException 'Invalid assertion provided'.
