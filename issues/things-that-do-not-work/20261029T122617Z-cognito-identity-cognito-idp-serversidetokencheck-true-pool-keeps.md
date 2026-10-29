Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable
Issue-Key: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable|_poll_for_refusal#16|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 83aee2831731f4d9adb4a4e165486d477c375438

# cognito-identity__cognito-idp: ServerSideTokenCheck=true pool keeps vending for an ID token whose user was disabled — the revocation refusal never arrives, so the check exhausts its propagation budget

> _Note: this issue was identified against the code base at commit `83aee2831731f4d9adb4a4e165486d477c375438`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
Cause 1 of 1: an identity pool whose CognitoIdentityProviders[] entry carries ServerSideTokenCheck=true never revalidates the presented user-pool ID token against the user pool, so AdminDisableUser on the token's user does not revoke the token and GetCredentialsForIdentity (and GetId) keep succeeding.

  AWS: with ServerSideTokenCheck=true, Cognito Identity re-checks the token's user in the named user pool on every GetId/GetCredentialsForIdentity call. Once the user is disabled with cognito-idp AdminDisableUser, a pre-existing ID token is refused with HTTP 403 `NotAuthorizedException`; the message is the fixed sentence "Token is inactive" (no request- or state-derived parts in it). The refusal persists after AdminEnableUser for that same outstanding token, while a freshly minted token vends again. A twin identity pool differing ONLY in ServerSideTokenCheck=false keeps vending for the very same token, which is what proves the flag is the single variable. Smithy (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice): GetCredentialsForIdentity errors include NotAuthorizedException @httpError 403 (also ResourceNotFoundException 404, InvalidParameterException 400); GetId likewise models NotAuthorizedException @httpError 403. The SSTC revalidation semantics and the exact message text are recorded behavior only — the recording shows code `NotAuthorizedException` with message "Token is inactive".
  Emulator: after AdminDisableUser (AdminGetUser correctly reports Enabled=false), GetCredentialsForIdentity on the SSTC=true pool with the unchanged token keeps returning 200 with Credentials. The check's `_poll_for_refusal` retries the call FOR a refusal with a 180 s propagation budget and never sees any ClientError, so it consumes the whole run budget and ends with `TimeoutError: the SSTC=True refusal for a disabled user's token: the run budget ended first; not graded`. The emulator therefore behaves identically to the SSTC=false twin: the flag is stored (DescribeIdentityPool echoes it) but never consulted.
  Why: a client cannot revoke access by disabling a user. Any token minted before the disable keeps exchanging for authenticated-role credentials indefinitely, so offboarding and incident-response flows — the entire reason to enable ServerSideTokenCheck — validate as working against the emulator and fail in production. Because the divergence is an unexpected SUCCESS rather than a wrong error, callers that poll for the revocation never terminate.
  Fix: the login/token validation path shared by GetId and GetCredentialsForIdentity in the cognito-identity service: when the matched CognitoIdentityProviders entry has ServerSideTokenCheck=true, resolve the token's subject in the named user pool and refuse an inactive (disabled) or absent user with the modeled NotAuthorizedException above, instead of accepting any token whose provider NAME is listed on the pool. The refusal must also survive a subsequent AdminEnableUser for that already-outstanding token.
  Traces: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable. Login validation lives in services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs, which only matches the provider name against the pool's CognitoIdentityProviders list.

Related open issue, which the replay did NOT mark as owning this check (its stored replay key is a different frame of `_poll_for_refusal`, so this failure came back unowned): 20261026T054328Z-cognito-identity-cognito-idp-serversidetokencheck-is-not-enforced.md. A fixer closing that one should close this with it; it is named here so the two are not fixed twice.

## Evidence
cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable [error]
  expected: after AdminDisableUser, the SSTC=true identity pool refuses the pre-existing ID token on GetCredentialsForIdentity with HTTP 403, Error.Code `NotAuthorizedException`, Error.Message "Token is inactive" (fixed text, no variable parts); GetId with the same Logins refuses identically; the SSTC=false twin pool still vends; after AdminEnableUser a fresh token vends on the checked pool while the outstanding token stays refused with the same 403/NotAuthorizedException/"Token is inactive".
  actual:   no refusal ever arrives. GetCredentialsForIdentity keeps answering 200 with Credentials for the disabled user's token; `_poll_for_refusal` (180 s budget, 1 s interval) observes zero ClientErrors and the run ends with `TimeoutError: the SSTC=True refusal for a disabled user's token: the run budget ended first; not graded` (check file line 235 -> _poll_for_refusal line 73). The positive controls before the disable pass, and DescribeIdentityPool echoes ServerSideTokenCheck=true, so the flag is persisted but ignored at vend time.

## Steps to Reproduce
1. Build and boot the emulator, then replay cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable.
2. Manually: create a cognito-idp user pool, an app client with ALLOW_ADMIN_USER_PASSWORD_AUTH/ALLOW_REFRESH_TOKEN_AUTH, and one confirmed user with a permanent password; mint an ID token via AdminInitiateAuth.
3. Create two identity pools fronted by provider cognito-idp.<region>.amazonaws.com/<userPoolId> with the same ClientId, one with ServerSideTokenCheck=true and one with false, each with an authenticated role whose trust policy admits cognito-identity.amazonaws.com for its own pool; confirm both vend credentials for the token (GetId + GetCredentialsForIdentity).
4. Call AdminDisableUser for the user and confirm AdminGetUser reports Enabled=false.
5. Call GetCredentialsForIdentity on the SSTC=true pool with the SAME unchanged token. The emulator returns 200 with Credentials; AWS returns HTTP 403 NotAuthorizedException "Token is inactive". GetId on that pool with the same Logins also succeeds instead of refusing.
6. Call AdminEnableUser and repeat step 5 with the outstanding token: AWS still refuses it with the same 403; the emulator keeps vending.
