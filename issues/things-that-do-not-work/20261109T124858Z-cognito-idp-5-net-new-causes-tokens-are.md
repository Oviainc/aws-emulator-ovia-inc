Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.id-token-verifies-rs256-against-the-issuer-jwks cognito-idp.id-token-from-another-pool-rejected-by-this-pools-jwks cognito-idp.openid-discovery-jwks-uri-serves-unique-unpadded-rs256-signing-keys cognito-idp.refreshed-id-token-verifies-under-the-same-jwks-and-keeps-the-session-claims cognito-idp.admin-password-sign-in-is-refused-for-a-disabled-user-until-re-enabled cognito-idp.refresh-token-auth-for-a-disabled-user-rejected cognito-idp.refresh-token-auth-under-another-client-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.id-token-verifies-rs256-against-the-issuer-jwks|run#13|AssertionError|assert header.get("alg") == "RS256", f"expected ID token alg RS256, got {header!r}"
Issue-Key: cognito-idp.id-token-from-another-pool-rejected-by-this-pools-jwks|_assert_issuer#4|AssertionError|assert (parts is not None and parts.scheme == "https"
Issue-Key: cognito-idp.openid-discovery-jwks-uri-serves-unique-unpadded-rs256-signing-keys|_assert_issuer#13|AssertionError|assert (parts is not None and parts.scheme == "https"
Issue-Key: cognito-idp.refreshed-id-token-verifies-under-the-same-jwks-and-keeps-the-session-claims|_assert_issuer#15|AssertionError|assert (parts is not None and parts.scheme == "https"
Issue-Key: cognito-idp.admin-password-sign-in-is-refused-for-a-disabled-user-until-re-enabled|_refusal#18|AssertionError|assert isinstance(status, int) and 400 <= status < 500, (
Issue-Key: cognito-idp.refresh-token-auth-for-a-disabled-user-rejected|run#17|AssertionError|assert refresh == ("NotAuthorizedException", "User is disabled."), (
Issue-Key: cognito-idp.refresh-token-auth-under-another-client-rejected|run#16|AssertionError|assert (code, message) == ("NotAuthorizedException", "Refresh Token has different Client"), (
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# cognito-idp: 5 net-new causes — tokens are signed ES256, the iss claim is the bare emulator endpoint, REFRESH_TOKEN_AUTH answers 501 for a disabled user, and two refresh-token refusal messages are wrong

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Five net-new root causes in token issuance and the refresh-token auth path.

Cause 1 of 5: issued tokens are signed with ES256 instead of RS256.
AWS: recorded behavior only for the JOSE header (the token body is not Smithy-modelled). Every Cognito ID/access token is a JWT whose header is {"alg":"RS256","kid":<a key id from the pool's jwks_uri>,"typ":"JWT"} and whose signature verifies as RSASSA-PKCS1-v1_5/SHA-256 against the RSA key the pool's JWKS publishes.
Emulator: the header is {"alg":"ES256","kid":...,"typ":"JWT"}, i.e. an EC key and algorithm.
Why: every library that verifies Cognito tokens pins RS256 (and fetches an RSA JWK); an ES256 token is rejected outright or, worse, accepted only by code that does not pin the algorithm — so no real verifier can validate a token the emulator issues.
Fix: the cognito-idp token-signing key material and JWT header/JWKS serialization (the pool's signing-key state and the jwks_uri document). Related open issue that does NOT cover this check: 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md.
Traces: cognito-idp.id-token-verifies-rs256-against-the-issuer-jwks.

Cause 2 of 5: the iss claim (and the OpenID discovery issuer) is the emulator's bare endpoint, with no cognito-idp.<region> host.
AWS: recorded behavior only. The iss claim of an ID token, and the `issuer` of the pool's /.well-known/openid-configuration, are https://cognito-idp.<region>.<domain>/<user pool id> — the fixed shape is the scheme https, a host whose first labels are 'cognito-idp' and the pool's region, and a single path segment equal to the pool id taken from the request/state. The <domain> and port are environment values; the cognito-idp.<region> host prefix and the pool-id path are not.
Emulator: iss is https://<emulator host>:<port>/<pool id> (e.g. https://127.0.0.1:37571/us-west-2_00754d2b7) — the issuer identifies no Cognito endpoint, only the listener it was reached on.
Why: token validators and OIDC clients derive the JWKS URI and the audience/issuer check from iss; an issuer with no cognito-idp.<region> host fails issuer validation, cannot be distinguished from another provider's issuer, and makes cross-pool rejection untestable.
Fix: the cognito-idp issuer computation shared by the token minting path and the OpenID discovery/JWKS documents (the per-pool issuer URL it stores/serves).
Traces: cognito-idp.id-token-from-another-pool-rejected-by-this-pools-jwks, cognito-idp.openid-discovery-jwks-uri-serves-unique-unpadded-rs256-signing-keys, cognito-idp.refreshed-id-token-verifies-under-the-same-jwks-and-keeps-the-session-claims.

Cause 3 of 5: the admin refresh leg answers HTTP 501 for a disabled user.
AWS: Smithy: AdminInitiateAuth (and InitiateAuth) declare NotAuthorizedException with httpError 403; there is no 501 in the operation's error set, and the server-side InternalErrorException is 500. Refreshing with AuthFlow=REFRESH_TOKEN_AUTH for a user that has been disabled is refused 403 NotAuthorizedException with message 'User is disabled.' (fixed text, no variable part).
Emulator: answers HTTP 501 — an unimplemented-style status outside the operation's modelled error set, so the call is not even a refusal a client can classify.
Why: an SDK maps 501 to a non-retryable protocol/implementation failure rather than NotAuthorized; a client cannot tell 'this user is disabled, stop refreshing' from 'the service is broken', and the disabled-user state is unenforced on that path.
Fix: the AdminInitiateAuth/InitiateAuth handler's REFRESH_TOKEN_AUTH branch in the cognito-idp service crate (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/) — it must consult the user's Enabled state and refuse with the modelled 403 error.
Traces: cognito-idp.admin-password-sign-in-is-refused-for-a-disabled-user-until-re-enabled.

Cause 4 of 5: refreshing for a disabled user is refused with the revoked-token message.
AWS: Smithy: InitiateAuth declares NotAuthorizedException (httpError 403). REFRESH_TOKEN_AUTH for a disabled user is refused NotAuthorizedException with message 'User is disabled.' — the disabled-user sentence, not a token-state sentence.
Emulator: refuses NotAuthorizedException with message 'Refresh Token has been revoked'.
Why: the message is the only thing distinguishing 'the account was disabled' from 'this refresh token was revoked/rotated'; a client that reacts to a revoked token by re-authenticating will loop, because re-authentication is also refused while the user is disabled.
Fix: the InitiateAuth REFRESH_TOKEN_AUTH validation order in cognito-idp — the user-state check must precede/override the token-state message.
Traces: cognito-idp.refresh-token-auth-for-a-disabled-user-rejected.

Cause 5 of 5: a refresh token presented under a different app client is refused with the generic invalid-token message.
AWS: Smithy: InitiateAuth declares NotAuthorizedException (httpError 403). A refresh token minted for client A and presented with ClientId=B is refused NotAuthorizedException with message 'Refresh Token has different Client' (fixed text, no variable part).
Emulator: refuses NotAuthorizedException with message 'Invalid Refresh Token.'
Why: a multi-client application cannot tell a client-id mix-up from a malformed/expired token, so the actual misconfiguration (wrong ClientId) is undiagnosable from the API response.
Fix: the InitiateAuth REFRESH_TOKEN_AUTH branch's client-binding check and the message it emits.
Traces: cognito-idp.refresh-token-auth-under-another-client-rejected.

## Evidence
Cause 1 — AssertionError: expected ID token alg RS256, got {'alg': 'ES256', 'kid': 'tngVQzGGwDG-adWsYHOkA4zRwPZweFEayWNpdBdIKVw', 'typ': 'JWT'}.
Cause 2 — AssertionError: the ID token must be issued by https://cognito-idp.us-west-2.<domain>/<pool id>, got iss='https://127.0.0.1:37571/us-west-2_00754d2b7' (same shape in the discovery-document and refreshed-token checks).
Cause 3 — AssertionError: refresh for a disabled user: answered HTTP 501, expected 4xx.
Cause 4 — AssertionError: expected NotAuthorizedException 'User is disabled.' refreshing a disabled user, got ('NotAuthorizedException', 'Refresh Token has been revoked').
Cause 5 — AssertionError: expected NotAuthorizedException 'Refresh Token has different Client', got ('NotAuthorizedException', 'Invalid Refresh Token.').

## Steps to Reproduce
1) Create a user pool + app client and a confirmed user; sign in with ADMIN_USER_PASSWORD_AUTH.
2) Cause 1: base64-decode the ID token header — alg is ES256, not RS256.
3) Cause 2: read the iss claim and GET the pool's /.well-known/openid-configuration — both name https://<host>:<port>/<poolId> instead of https://cognito-idp.<region>.<domain>/<poolId>.
4) Cause 3/4: AdminDisableUser, then AdminInitiateAuth/InitiateAuth with AuthFlow=REFRESH_TOKEN_AUTH and the user's refresh token — the admin call answers HTTP 501, the non-admin call answers NotAuthorizedException 'Refresh Token has been revoked' instead of 403 'User is disabled.'.
5) Cause 5: create a second app client in the same pool and present client A's refresh token with ClientId=B — the refusal says 'Invalid Refresh Token.' instead of 'Refresh Token has different Client'.
