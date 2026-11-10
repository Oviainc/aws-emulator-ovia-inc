Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends
Issue-Key: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable|_poll_for_refusal#16|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends|_poll_for_refusal#14|Timeout|time.sleep(interval_s)
Issue-Commit: 3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e

# cognito-identity__cognito-idp: ServerSideTokenCheck is not enforced — a disabled/deleted user's ID token keeps vending, so both SSTC scenarios hang to the 60 s cap waiting for a refusal that never arrives

> _Note: this issue was identified against the code base at commit `3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
Cause 1 of 1: an identity pool created with CognitoIdentityProviders[].ServerSideTokenCheck=true never revalidates the presented user-pool ID token against the user pool, so disabling or deleting the user does not revoke the token.
  AWS: with ServerSideTokenCheck=true, Cognito Identity checks the token's user against the user pool on every GetId/GetCredentialsForIdentity. Once the user is disabled (AdminDisableUser) — and still after AdminDeleteUser — a pre-existing ID token is refused with HTTP 403 `NotAuthorizedException` whose message states the token is inactive (recorded wording: "Token is inactive"), and the refusal persists even after the user is re-enabled; AdminGetUser for the deleted user answers `UserNotFoundException`. The twin pool configured with ServerSideTokenCheck=false keeps vending for the very same token, which is what proves the flag is the only variable. Smithy: `GetCredentialsForIdentity` models `NotAuthorizedException` (HTTP 403) and `ResourceNotFoundException` (HTTP 404); the SSTC semantics themselves are recorded behavior only. Fixed text of the refusal is the message sentence; nothing in it is request- or state-derived.
  Emulator: both checks are killed at the 60 s harness cap. Their only unbounded waits are propagation polls with a 180 s budget: `_poll_for_refusal`, which retries the call FOR a refusal and only returns when one arrives. No refusal ever arrives, so the loop runs until the cap. Hypothesis, from the error shape: the SSTC=true pool answers the disabled (then deleted) user's token with a successful vend exactly like the SSTC=false twin, i.e. ServerSideTokenCheck is stored but never consulted.
  Why: a client cannot revoke access by disabling or deleting a user. A token minted before the user was removed keeps exchanging for credentials forever against the emulator, so incident-response and offboarding flows (the whole point of enabling ServerSideTokenCheck) validate as working and fail in production. It also makes any check that waits for the revocation unable to terminate rather than reporting a clean mismatch.
  Fix: the GetId/GetCredentialsForIdentity login-validation path of the cognito-identity service — when the identity pool's provider entry has ServerSideTokenCheck=true, resolve the token's user in the named user pool and refuse an inactive/absent user with the modeled NotAuthorizedException above, instead of accepting any token the pool's provider list names.
  Traces: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable, cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends. Login validation lives in services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:269-295, which only checks that the provider NAME is one of the pool's CognitoIdentityProviders.

Related open issue that does not cover this (neither check is marked OWNED by it, and it describes the positive-control leg rather than the revocation leg): 20261025T005354Z-cognito-identity-cognito-idp-both-server-side-token.md.

## Evidence
cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable
  expected: after AdminDisableUser, the SSTC=true pool refuses the pre-existing ID token on GetCredentialsForIdentity with HTTP 403 NotAuthorizedException ("Token is inactive"), and still refuses it after AdminEnableUser
  actual:   Check timed out after 60.0s — `_poll_for_refusal` (180 s budget, 3 s interval) never observes any ClientError; every attempt succeeds

cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends
  expected: after AdminDisableUser + AdminDeleteUser, the SSTC=true pool refuses the token (403 NotAuthorizedException, "Token is inactive") while the SSTC=false twin pool keeps vending
  actual:   Check timed out after 60.0s in the same refusal poll — the SSTC=true pool behaves identically to the SSTC=false twin

## Steps to Reproduce
1. Build and boot the emulator and replay cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable and cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends.
2. Manually: create a cognito-idp user pool, an app client with ALLOW_ADMIN_USER_PASSWORD_AUTH, and one confirmed user; mint an ID token.
3. Create two identity pools fronted by that provider — one with ServerSideTokenCheck=true, one with false — each mapped to a role whose trust policy admits cognito-identity.amazonaws.com for its own pool; confirm both vend credentials for the token.
4. AdminDisableUser (then AdminDeleteUser) the user, and call GetCredentialsForIdentity on the SSTC=true pool with the SAME pre-existing token. It still returns 200 Credentials; AWS refuses it with 403 NotAuthorizedException. The SSTC=false twin correctly keeps vending, so the flag makes no difference.
