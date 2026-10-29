Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-login-under-another-providers-key-rejected
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-login-under-another-providers-key-rejected|run#24|AssertionError|assert message == expected, f"expected AWS's exact message {expected!r}, got {message!r}"
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# cognito-identity__cognito-idp__iam: a login token presented under another configured provider's Logins key is refused with the wrong message ('Incorrect token audience.' instead of "Issuer doesn't match providerName")

> _Note: this issue was identified against the code base at commit `c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetId

## Description
Cause 1 of 1 — the issuer-vs-providerName mismatch refusal carries the audience-mismatch message.
AWS: recorded behavior only (measured us-west-2): a user-pool ID token presented under ANOTHER configured OIDC provider's Logins key is refused by both GetId and GetCredentialsForIdentity with NotAuthorizedException (Smithy: NotAuthorizedException is modeled on both operations, @httpError 403) and the fixed message "Invalid login token. Issuer doesn't match providerName" — no variable parts. AWS names the failed check (issuer vs. the Logins key it was presented under), and the refused call mints no identity and adds no login to the existing identity.
Emulator: the code is correct (NotAuthorizedException) but the message is "Invalid login token. Incorrect token audience." — a different failure class (audience) than the one the request actually violates (issuer/providerName).
Why: a client diagnosing a federation misconfiguration is pointed at the app-client/audience configuration instead of at the Logins key / provider mapping that is actually wrong; AWS-worded assertions on the refusal do not match.
Fix: the login-token validation in the GetId / GetCredentialsForIdentity handlers — the issuer-vs-provider-key check must emit its own message rather than falling through to the audience check's message.
Traces: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-login-under-another-providers-key-rejected.

## Evidence
AssertionError: expected AWS's exact message "Invalid login token. Issuer doesn't match providerName", got 'Invalid login token. Incorrect token audience.' — asserted for both ci.get_id(IdentityPoolId=pool, Logins={key_b: token_a}) and ci.get_credentials_for_identity(IdentityId=id_a, Logins={key_b: token_a}).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-login-under-another-providers-key-rejected.
3. By hand: configure one identity pool with two OIDC/user-pool provider keys A and B, obtain a token issued by A, then call GetId with Logins={B: token_A} — expect NotAuthorizedException with the message "Invalid login token. Issuer doesn't match providerName", and the same from GetCredentialsForIdentity.
