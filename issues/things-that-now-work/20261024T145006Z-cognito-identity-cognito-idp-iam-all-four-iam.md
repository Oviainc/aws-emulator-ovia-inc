Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-client-id-missing-from-iam-provider-rejected cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-client-id-missing-from-iam-provider-rejected|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cognito-identity__cognito-idp__iam: all four IAM-OIDC federation checks hang to the 60 s harness cap — a user-pool ID token is never admitted through an OpenIdConnectProviderARN

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
=== Cause 1 of 1: federation of a user-pool ID token through an IAM OIDC identity provider never becomes admissible, so every check on that path is killed at the harness cap ===
AWS: recorded against real AWS (us-west-2, 2026-10-01) by all four checks: an identity pool with NO CognitoIdentityProviders and only an IAM OIDC provider's ARN in OpenIdConnectProviderARNs admits a Cognito user-pool ID token under the issuer key `cognito-idp.<region>.amazonaws.com/<user-pool-id>` (the `iss` claim without its scheme) and vends STS credentials; the first GetId right after CreateOpenIDConnectProvider may be refused NotAuthorizedException ("Incorrect token audience.") and succeeds a few seconds later as IAM's ClientIDList propagates. On the refusal side: an `aud` not present in the provider's ClientIDList is refused NotAuthorizedException until AddClientIDToOpenIDConnectProvider; deleting the IAM OIDC provider does not cascade into the pool (the dangling ARN is still described) but GetCredentialsForIdentity through it is refused NotAuthorizedException; and after UpdateIdentityPool drops a provider ARN, GetCredentialsForIdentity for that provider's identity is refused NotAuthorizedException while a sibling provider on the same pool keeps vending. Smithy: GetCredentialsForIdentity models NotAuthorizedException (HTTP 403) and is bound with an empty auth list and optionalAuth:true.
Emulator: none of the four checks reaches any of those assertions. Each one is cut off by the suite's 60 s per-check cap while polling for the first admission (the checks' own propagation budget is 180 s), so the positive vend never happens and the three refusal boundaries are never exercised.
Why: a client federating an OIDC/user-pool token through an IAM identity provider gets no answer it can act on — it cannot distinguish "the provider has not propagated yet" from "this token will never be admitted", and none of the security boundaries on this path (audience in ClientIDList, provider deleted, provider removed from the pool) can be validated against the emulator.
Hypothesis: the polled admission call is answered with a persistent refusal that the checks' poll helper counts as "not yet" and therefore loops on; a likely source is that unsigned/optionalAuth cognito-identity requests are answered with error code UnknownService ("no emulated service is registered for 'awscognitoidentityservice'"), the routing defect reported in 20261023T185902Z-cognito-identity-unsigned-anonymous-requests-answer-unknownservice-and.md Cause 1 — related, but it carries none of these four checks.
Fix: login-token admission in the cognito-identity credentials path — match a login's issuer key against the pool's OpenIdConnectProviderARNs, resolve that ARN in IAM OIDC-provider state (url/ClientIDList), and answer GetId/GetCredentialsForIdentity with either a vend or a NotAuthorizedException instead of a condition the client must poll forever.
Traces: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials, cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-client-id-missing-from-iam-provider-rejected, cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected, cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected.

## Evidence
All four checks in the group 'oidc-federation' end identically:
  [mismatch] cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials — Error: Check timed out after 60.0s
  [mismatch] cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-client-id-missing-from-iam-provider-rejected — Error: Check timed out after 60.0s
  [mismatch] cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected — Error: Check timed out after 60.0s
  [mismatch] cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected — Error: Check timed out after 60.0s
No frame is reported: the checks never leave the `_poll_until(..., timeout_s=_PROPAGATION_S=180.0, interval_s=3.0)` admission loop that precedes every assertion, so not one of the recorded responses (a 200 vend, or a 403 NotAuthorizedException) is ever observed.
Recorded AWS reference inlined in each check header (us-west-2, 2026-10-01): admission within ~5 s of CreateOpenIDConnectProvider; refusals NotAuthorizedException with messages such as "Invalid login token. Incorrect token audience." and "Token is not from a supported provider of this identity pool." (messages quoted as examples; the 403 + NotAuthorizedException are the requirement).
Model: smithy_query operation=GetCredentialsForIdentity -> errors include NotAuthorizedException (httpError 403), InvalidIdentityPoolConfigurationException (400); "auth": [], "optionalAuth": true.

## Steps to Reproduce
1. Create a Cognito user pool + app client and sign a user in to obtain an ID token.
2. iam CreateOpenIDConnectProvider with Url=https://cognito-idp.<region>.amazonaws.com/<user-pool-id> and ClientIDList=[<app client id>].
3. cognito-identity CreateIdentityPool with NO CognitoIdentityProviders and OpenIdConnectProviderARNs=[<the provider ARN>], then SetIdentityPoolRoles with an authenticated role trusting the pool.
4. GetId / GetCredentialsForIdentity with Logins={'cognito-idp.<region>.amazonaws.com/<user-pool-id>': <ID token>} -> expect an identity and vended STS credentials within a few seconds (a transient 403 NotAuthorizedException "Incorrect token audience." is allowed only briefly); observe the admission never succeeding, so the call is still being polled when the 60 s budget expires.
5. The three refusal legs are unreachable for the same reason: (a) present a token whose `aud` is an app client NOT in ClientIDList -> expect 403 NotAuthorizedException; (b) DeleteOpenIDConnectProvider and vend again -> expect 403 NotAuthorizedException while a sibling provider still vends; (c) UpdateIdentityPool dropping the provider ARN and vend again -> expect 403 NotAuthorizedException.
