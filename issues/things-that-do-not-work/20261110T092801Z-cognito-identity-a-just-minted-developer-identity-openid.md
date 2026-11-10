Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetCredentialsForIdentity|NotAuthorizedException
Issue-Commit: 27e85037a3bb1cfc28a1944211d6fd0ab6477136

# cognito-identity: a just-minted developer-identity OpenID token with TokenDuration=1 is refused as expired by GetCredentialsForIdentity — no clock-skew grace period on the token validity window

> _Note: this issue was identified against the code base at commit `27e85037a3bb1cfc28a1944211d6fd0ab6477136`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One net-new root cause on the cognito-identity OpenID-token validity window. Open cognito-identity issues that are related but do NOT carry this replay key: 20261029T173827Z-cognito-identity-10-net-new-token-credential-refusal.md (token integrity, forged logins, unsigned calls, pool-vs-identity not-found wording) and 20261029T173827Z-cognito-identity-10-net-new-developer-identity-root.md — neither covers the expiry/grace-period behaviour of a token this service itself minted.

Cause 1 of 1 — the validity window of a service-minted OpenID token is evaluated as a strict `expiration > now` with NO clock-skew grace, so a token issued at the low end of the TokenDuration range is already unusable by the time the caller presents it.

AWS: GetOpenIdTokenForDeveloperIdentity models TokenDuration as a long with @range min 1, max 86400, and the model documents the window's tolerance explicitly: "The expiration time of the token, in seconds. You can specify a custom expiration time for the token so that you can cache it. If you don't provide an expiration time, the token is valid for 15 minutes." plus the note "Please provide for a small grace period, usually no more than 5 minutes, to account for clock skew." (Smithy: GetOpenIdTokenForDeveloperIdentityInput.TokenDuration doc.) Consequently a token minted with the minimum TokenDuration=1 is STILL ACCEPTED when it is exchanged immediately after issuance — GetCredentialsForIdentity answers HTTP 200 with a complete Credentials set (AccessKeyId, SecretKey, SessionToken, Expiration) — and it is refused only once the nominal duration plus that skew grace has elapsed, at which point the refusal is the modelled NotAuthorizedException (Smithy: NotAuthorizedException @httpError 403) with the fixed message text "Invalid login token. Token is expired." (no request- or state-derived part).

Emulator: the exchange of the freshly minted TokenDuration=1 token is refused at once. GetCredentialsForIdentity(IdentityId=<the id just returned>, Logins={"cognito-identity.amazonaws.com": <the token just returned>}) answers NotAuthorizedException / "Invalid login token. Token is expired." with no credentials, on the very next call after GetOpenIdTokenForDeveloperIdentity returned the token. The code and the message text are the right ones for an expired token — the defect is WHEN they are served: the window has no grace at all, so it closes a second after issuance.

Why: a client relying on AWS's behaviour cannot use the documented low end of the TokenDuration range at all. Any caller that requests a short, cacheable developer token — and any caller whose clock runs even slightly ahead of the emulator's, which is exactly what the documented grace period exists to absorb — receives a token that it is then told is expired, so a legitimate developer-authenticated federation happy path turns into a 403 instead of credentials. Code that treats NotAuthorizedException as "re-authenticate the user" will loop: every re-issued short token is refused the same way.

Fix: the OpenID-token validity-window evaluation shared by the credential-exchange path — the token-issuance lookup in the cognito-identity service crate's state (the strict `expiration > now` filter used when a presented token is resolved, and the matching purge of expired issuances) — and the login-token validation step in the GetCredentialsForIdentity handler that maps that lookup to the Expired refusal. The window a presented token is read against must extend past the recorded expiration by the documented clock-skew grace, while issuance itself keeps honouring the requested TokenDuration.

Traces: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry. Code sites for the fixer: services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/state.rs lines 2144-2191 (`purge_expired_open_id_tokens`, `open_id_token_record`, both filtering `issued.expiration > now`), the `LoginTokenRefusal::Expired` -> `EXPIRED_LOGIN_TOKEN_MESSAGE` mapping at state.rs lines 1769 and 1803-1813, and services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs (the `let now = SystemTime::now()` at line 263 and the login-token resolution that precedes role resolution).

## Evidence
Check cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry errored, not merely mismatched — the refusal escaped as an unhandled client error from the UNGUARDED GetCredentialsForIdentity call that immediately follows the TokenDuration=1 issuance:

  botocore.errorfactory.NotAuthorizedException: An error occurred (NotAuthorizedException) when calling the GetCredentialsForIdentity operation: Invalid login token. Token is expired.

Sequence replayed (all against the same pool and identity):
  1. CreateIdentityPool(AllowUnauthenticatedIdentities=false, DeveloperProviderName=<dev provider>) -> pool id
  2. CreateRole with a trust policy Federated=cognito-identity.amazonaws.com, StringEquals cognito-identity.amazonaws.com:aud = <pool id>, ForAnyValue:StringLike cognito-identity.amazonaws.com:amr = authenticated; SetIdentityPoolRoles(authenticated=<role arn>)
  3. GetOpenIdTokenForDeveloperIdentity(Logins={<dev provider>: <user id>}, TokenDuration=86400) -> IdentityId + Token; GetCredentialsForIdentity with that token succeeds (the max-duration half of the window passes)
  4. GetOpenIdTokenForDeveloperIdentity(Logins={<dev provider>: <same user id>}, TokenDuration=1) -> the SAME IdentityId + a new Token
  5. GetCredentialsForIdentity(IdentityId=<that id>, Logins={"cognito-identity.amazonaws.com": <that token>}) — expected HTTP 200 with a complete Credentials set, because the token was minted moments earlier and AWS layers a documented clock-skew grace of "usually no more than 5 minutes" on the nominal 1-second duration. The emulator answered NotAuthorizedException (HTTP 403) / "Invalid login token. Token is expired." instead.

The check's later poll (which waits out nominal duration + grace and then requires exactly NotAuthorizedException / "Invalid login token. Token is expired.") was never reached, so the emulator's code and message for a genuinely expired token are not in dispute — only the instant at which they begin to be served. Note the message text is correct and carries no variable parts; nothing in this finding depends on a generated pool id, identity id, role name or timestamp.

## Steps to Reproduce
1. Boot the emulator and replay cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry.
2. By hand: CreateIdentityPool(IdentityPoolName=<any>, AllowUnauthenticatedIdentities=false, DeveloperProviderName=<any dev provider name>).
3. CreateRole with a trust policy allowing sts:AssumeRoleWithWebIdentity to Federated principal cognito-identity.amazonaws.com, conditioned on cognito-identity.amazonaws.com:aud = <pool id> and ForAnyValue:StringLike cognito-identity.amazonaws.com:amr = authenticated; then SetIdentityPoolRoles(IdentityPoolId=<pool id>, Roles={"authenticated": <role arn>}).
4. GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool id>, Logins={<dev provider>: <user id>}, TokenDuration=1) and note IdentityId and Token.
5. Immediately (within a second or two) call GetCredentialsForIdentity(IdentityId=<that IdentityId>, Logins={"cognito-identity.amazonaws.com": <that Token>}). Expected: HTTP 200 with Credentials.AccessKeyId / SecretKey / SessionToken / Expiration populated. Observed: HTTP 403 NotAuthorizedException "Invalid login token. Token is expired.".
6. Repeat step 5 after waiting longer than the nominal duration plus the documented clock-skew grace (~5 minutes): THEN the NotAuthorizedException "Invalid login token. Token is expired." refusal is the correct answer and must still be served.
