Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.revoke-token-kills-only-the-origin-jti-family-of-that-refresh-token
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.revoke-token-kills-only-the-origin-jti-family-of-that-refresh-token|run#29|AssertionError|assert message == "Access Token has been revoked", (
Issue-Commit: 8a5e62a415deb94ea31f987a6913244850fd3b2f

# cognito-idp GetUser refuses a revoked access token with 'Invalid Access Token' instead of AWS's 'Access Token has been revoked'

> _Note: this issue was identified against the code base at commit `8a5e62a415deb94ea31f987a6913244850fd3b2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUser

## Description
One net-new root cause: the revoked-token refusal on the token-authorization path is not distinguished from a malformed/unknown token, so it carries the generic invalid-token sentence.

Cause 1 of 1: after RevokeToken on a refresh token, GetUser called with an access token of that refresh token's origin_jti family is refused with the right error class but the wrong message.

AWS: GetUser with an access token that belongs to a revoked session is refused with NotAuthorizedException and the message `Access Token has been revoked` — entirely FIXED text, with no variable part: it names neither the token, the jti/origin_jti, the client nor the user, so nothing in it is taken from the request or from emulator state. The companion refusal on the refresh path is likewise the fixed `Refresh Token has been revoked`. Smithy: cognito-idp#GetUser input {AccessToken: TokenModelType, required, sensitive}; modelled errors include NotAuthorizedException (errorKind client). The message itself is recorded behavior only — it is the sentence AWS returned in this project's recorded corpus for exactly this refusal (see the AWS side of 20261029T184956Z-cognito-idp-revocation-provider-linking-risk-and-mfa.md, Cause 1: "AWS: 400 NotAuthorizedException, message `Access Token has been revoked` (fixed text, no variable part)").

Emulator: the status class and the error code are already correct — GetUser does refuse the revoked token with NotAuthorizedException, and the untouched twin session's access token keeps working at the same instant — but the message is rendered as `Invalid Access Token`, the emulator's generic bad-token sentence. The revoked case is collapsed into the invalid case, so the refusal does not say why the token was refused.

Why: a client cannot tell a revoked session apart from a corrupt or foreign token. Code that branches on the refusal text — re-authenticate silently on revocation vs. report a malformed credential, SDK error-message assertions, log/alert matching on 'has been revoked', user-facing "you were signed out" messaging — takes the wrong branch against the emulator while working against AWS. Because the error code is already right, the divergence is only reachable through the message.

Fix: the token-authorization path shared by the access-token-bearing operations (the GetUser handler's access-token validation / the revocation-state lookup it calls) must construct a distinct NotAuthorizedException for the revoked case carrying AWS's fixed sentence `Access Token has been revoked`, rather than falling through to the generic 'Invalid Access Token' construction. It belongs in that validation step's error construction, not in the serializer: the code and status are already correct.

Not covered by any open issue, and this check is not marked OWNED by one: 20261029T184956Z-cognito-idp-revocation-provider-linking-risk-and-mfa.md describes revocation state not being honored at all (GetUser answering 200 with the profile) and carries only wire replay keys (cognito-idp|GetUser|4xx->2xx|...) for fixture cognito-idp-self-service-signup-and-token-session; the emulator now refuses, so what remains is the message, which no open issue's keys cover. 20261030T040953Z-cognito-idp-revoketoken-refusal-for-a-client-without.md is about RevokeToken's own OperationNotEnabledException wording, a different operation and refusal.

Traces: cognito-idp.revoke-token-kills-only-the-origin-jti-family-of-that-refresh-token.

## Evidence
Check cognito-idp.revoke-token-kills-only-the-origin-jti-family-of-that-refresh-token, verdict [mismatch]:
  File "check_cognito_idp_revoke_token_kills_only_the_origin_jti_family_of_that_refresh_token.py", line 165, in run
    assert message == "Access Token has been revoked", (
  AssertionError: expected message 'Access Token has been revoked' for the access token from the revoked refresh token's auth event, got 'Invalid Access Token'

Expected (AWS): GetUser(AccessToken=<token of the revoked origin_jti family>) -> NotAuthorizedException, Error.Message exactly 'Access Token has been revoked' (fixed text; no request- or state-derived values).
Actual (emulator): NotAuthorizedException with Error.Message 'Invalid Access Token'.
Everything the check asserts before that point passed, which bounds the defect to the message: both sessions' access tokens were usable before the revoke, the access tokens carry jti and origin_jti, the two sign-ins opened two distinct origin_jti families, REFRESH_TOKEN_AUTH preserved origin_jti and minted a new jti, and after RevokeToken the untouched twin session's access token still resolved the same username on every poll iteration while the revoked family's token was refused with the correct NotAuthorizedException class.

## Steps to Reproduce
1. Create a user pool and an app client with ExplicitAuthFlows ALLOW_USER_PASSWORD_AUTH/ALLOW_REFRESH_TOKEN_AUTH/ALLOW_ADMIN_USER_PASSWORD_AUTH, GenerateSecret=False, EnableTokenRevocation=True; create a user and set a permanent password.
2. InitiateAuth USER_PASSWORD_AUTH twice to get two independent sessions; run InitiateAuth REFRESH_TOKEN_AUTH on the first refresh token to mint a second access token in the same origin_jti family.
3. RevokeToken(Token=<first refresh token>, ClientId=<client>).
4. GetUser(AccessToken=<either access token of that family>) -> NotAuthorizedException with Message 'Invalid Access Token'; expected Message 'Access Token has been revoked'.
5. Replay the check id in trace_ids to confirm the fix.
