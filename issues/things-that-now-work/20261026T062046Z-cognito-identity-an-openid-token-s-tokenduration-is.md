Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry|run#29|Timeout|time.sleep(15)
Issue-Commit: 0e9a1926b382543791c56cc4902ed328671b8b8b

# cognito-identity: an OpenID token's TokenDuration is never enforced at credential exchange — an expired developer-identity token keeps vending credentials instead of NotAuthorizedException "Invalid login token. Token is expired."

> _Note: this issue was identified against the code base at commit `0e9a1926b382543791c56cc4902ed328671b8b8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
This issue carries ONE net-new root cause reached through a cognito-identity check. It must be fixed and re-replayed before closing.

=== Cause 1 of 1: the validity window of an OpenID token minted by GetOpenIdTokenForDeveloperIdentity is never checked when the token is exchanged by GetCredentialsForIdentity ===

AWS: GetOpenIdTokenForDeveloperIdentity.TokenDuration is modeled `long @range{min:1,max:86400}` (Smithy: com.amazonaws.cognitoidentity#GetOpenIdTokenForDeveloperIdentityInput$TokenDuration), documented as "The expiration time of the token, in seconds. You can specify a custom expiration time for the token so that you can cache it. If you don't provide an expiration time, the token is valid for 15 minutes. [...] Please provide for a small grace period, usually no more than 5 minutes, to account for clock skew." The token is therefore a time-bounded credential: once its duration (plus that documented skew grace period) has elapsed, presenting it to GetCredentialsForIdentity as Logins={"cognito-identity.amazonaws.com": <token>} is REFUSED. GetCredentialsForIdentity models NotAuthorizedException at HTTP 403 (Smithy: errors ExternalServiceException 400, InvalidIdentityPoolConfigurationException 400, InvalidParameterException 400, NotAuthorizedException 403, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429). The recorded refusal for this error class is HTTP 403, code NotAuthorizedException, message "Invalid login token. Token is expired." — that message is entirely FIXED text for this condition: it has no request- or state-derived variable part (no identity id, no pool id, no token, no timestamp), so it must be emitted verbatim. The symmetric positive half of the contract also holds: a token minted with the maximum TokenDuration=86400 is usable immediately, and re-minting for the same developer user identifier returns the SAME IdentityId.

Emulator: the expiry half is never enforced. A token minted with TokenDuration=1 (the modeled minimum) is still accepted by GetCredentialsForIdentity indefinitely: every exchange answers HTTP 200 with a full credential set, and no refusal is ever produced. The check polls the exchange for 450 s (15 s apart) waiting for the documented refusal to appear; because it never appears, the check is killed at the harness ceiling and the suite records `Error: Check timed out after 60.0s` with no error response ever captured.

Why: a client relying on AWS's behavior gets a login token that never dies. The entire negative branch a client has for an expired federated login token — catch NotAuthorizedException, re-authenticate against the backend, re-mint an OpenID token — is unreachable, so token-refresh logic is never exercised and a bug in it ships undetected. Token caching keyed on TokenDuration is validated as safe when it is not, and the security property the member's own documentation turns on ("an attacker could use a leaked token to access your Amazon Web Services resources for the token's duration") is not represented: here the duration is unbounded. It is also a hang rather than a wrong answer — the call neither succeeds-as-refused nor errors, so caller retry budgets are burned with no diagnosis.

Fix: the GetCredentialsForIdentity handler's vend path. It validates only the Logins provider NAMES — `pool.accepts_login_provider(provider)` over `input.logins.keys()` — and by its own comment never reads the token values out of the map, so the issuance registry that the minting side already maintains (the `open_id_tokens` store written via `record_open_id_token` with `issued_at`/`expiration`, populated from the request's TokenDuration or the documented 900 s default) is never consulted on the consuming side. The handler must resolve a token presented under the reserved provider name "cognito-identity.amazonaws.com" against that registry and refuse with NotAuthorizedException (403, message "Invalid login token. Token is expired.") when the recorded expiration has passed, before any role is resolved or any session is minted; an unknown/unissued token belongs in the same refusal channel rather than being vended for.

Traces: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry.

Not covered by the open issues on this service: 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md faults GetId for not validating login tokens at identity-mint time and lists a different set of checks; 20261025T005354Z-cognito-identity-getcredentialsforidentity-never-performs-a-real-assumerolewithwebidentity.md faults WHICH principal the vend returns, not WHETHER an expired login token is admitted; neither carries this check's replay key.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry (verdict: mismatch):

  Error: Check timed out after 60.0s

Scenario the check drives (SERVICES=["cognito-identity"]): CreateIdentityPool(AllowUnauthenticatedIdentities=False, DeveloperProviderName=<dev provider>), CreateRole trusting cognito-identity.amazonaws.com for sts:AssumeRoleWithWebIdentity with cognito-identity.amazonaws.com:aud=<pool> and amr=authenticated, SetIdentityPoolRoles(Roles={"authenticated": <role>}).

1. GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<dev provider>: <user id>}, TokenDuration=86400) -> 200; the token exchanges successfully through GetCredentialsForIdentity and sts:GetCallerIdentity resolves to an assumed-role ARN of the mapped role. This positive half is reached, so the check's failure is NOT a setup failure.
2. GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<dev provider>: <same user id>}, TokenDuration=1) -> 200, same IdentityId re-issued.
3. GetCredentialsForIdentity(IdentityId=<identity>, Logins={"cognito-identity.amazonaws.com": <the 1-second token>}) immediately -> 200 with a complete credential set (correct).
4. The same call repeated every 15 s for up to 450 s, long past the 1 s duration plus AWS's documented ~5 minute skew grace period:
     expected: HTTP 403, Error.Code "NotAuthorizedException", Error.Message "Invalid login token. Token is expired."
     actual:   HTTP 200 with credentials on every attempt; no ClientError is ever raised, so the poll never breaks and the check is killed at the 60 s harness cap.

Emulator code corroborating the cause (citations only):
- services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_open_id_token_for_developer_identity.rs:480-511 — computes `validity` from `input.token_duration` (defaulting to DEFAULT_TOKEN_DURATION = 900 s at line 127), derives `expiration`, and records it via `store.record_open_id_token(... issued_at, expiration ...)`. The issuance side DOES carry the window.
- services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:221-229 — `login_providers()`, commented "The tokens themselves are the caller's secrets and are never read out of the map here."
- services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:269-296 — the only Logins validation performed is `pool.accepts_login_provider(provider)` over the provider NAMES; the token value is never resolved against `open_id_tokens`, and no NotAuthorizedException for an expired token exists on this path. Lines 321-342 purge expired VENDED SESSIONS only, which is a different clock from the login token's.

Model (smithy_query, services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json):
- GetOpenIdTokenForDeveloperIdentity input: IdentityPoolId required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$, Logins required map @length{0,10}, TokenDuration long @range{1,86400}; output IdentityId, Token (OIDCToken, @sensitive).
- GetCredentialsForIdentity input: IdentityId required, Logins map @length{0,10}, CustomRoleArn; errors include NotAuthorizedException (client, HTTP 403); auth [] / optionalAuth true.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check id in trace_ids.
2. CreateIdentityPool(IdentityPoolName=<n>, AllowUnauthenticatedIdentities=False, DeveloperProviderName=<dev-provider>).
3. CreateRole with a trust policy allowing sts:AssumeRoleWithWebIdentity to Federated principal cognito-identity.amazonaws.com, conditioned on StringEquals cognito-identity.amazonaws.com:aud=<pool id> and ForAnyValue:StringLike cognito-identity.amazonaws.com:amr=authenticated; then SetIdentityPoolRoles(IdentityPoolId=<pool id>, Roles={"authenticated": <role arn>}).
4. tok = GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool id>, Logins={<dev-provider>: <dev user id>}, TokenDuration=1)["Token"].
5. Immediately call GetCredentialsForIdentity(IdentityId=<the returned IdentityId>, Logins={"cognito-identity.amazonaws.com": tok}) -> expect 200 with credentials (this must keep working).
6. Sleep well past the 1 s duration plus AWS's documented ~5 minute clock-skew grace period (the check polls 15 s apart for up to 450 s), then repeat the call in step 5.
   Expected: HTTP 403, Error.Code NotAuthorizedException, Error.Message exactly "Invalid login token. Token is expired."
   Observed: HTTP 200 with a fresh credential set on every attempt; the token never expires, so the polling check is killed at the 60 s harness cap with "Check timed out after 60.0s".
7. Repeat with TokenDuration omitted and assert the same refusal appears after the documented 15-minute default window.
