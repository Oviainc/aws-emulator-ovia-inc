Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected|_poll_for_refusal#20|Timeout|time.sleep(interval_s)
Issue-Commit: 5f5c8c74019b1fc5b04177af48d49ea878911ccb

# cognito-identity__cognito-idp__iam: removing an OIDC provider with UpdateIdentityPool never makes that provider's token stop vending — the removal-refusal check reaches no verdict in 180 s (1 cause)

> _Note: this issue was identified against the code base at commit `5f5c8c74019b1fc5b04177af48d49ea878911ccb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One net-new behavioral root cause, grounded in 1 failing check of the simparity-behavioral-aws-v1 replay (cognito-identity__cognito-idp__iam 3/5; the other failure in this service is marked OWNED and is not part of this issue). Related open issue that does NOT carry this replay key, named for the fixer: 20261028T040721Z-cognito-identity-cognito-identity-cognito-idp-iam-getcredentialsforidentity-vends-an.md — filed as 20261028T040721Z-cognito-identity-cognito-idp-iam-getcredentialsforidentity-vends-an.md — which records GetCredentialsForIdentity minting an 'EMUL'-prefixed access key id for an IAM-OIDC-federated login in the SAME service group; it does not own this check and does not cover the provider-set gate described here.

Cause 1 of 1: the identity pool's current OpenIdConnectProviderARNs set does not gate credential vending, so a token from an OIDC provider that UpdateIdentityPool removed is never refused, and the check that asserts the refusal burns the whole 180 s propagation window and is killed at the harness cap without reaching a verdict.
AWS: Smithy com.amazonaws.cognitoidentity#UpdateIdentityPool takes OpenIdConnectProviderARNs (target OIDCProviderList) and REPLACES the whole provider set, echoing the new set in its output (output shape IdentityPool), which DescribeIdentityPool then reports. com.amazonaws.cognitoidentity#GetCredentialsForIdentity models NotAuthorizedException with errorKind client and httpError 403 among its errors. Once provider A's ARN is no longer in the pool's set, GetCredentialsForIdentity for an identity whose Logins key is A's issuer is refused NotAuthorizedException (HTTP 403) with the message whose fixed text is "Token is not from a supported provider of this identity pool." (recorded behavior — the model carries no message text; the sentence has no variable parts). The refusal is a refusal to VEND only: DescribeIdentity still lists the identity's login for A's issuer key, a sibling provider B in the same pool keeps vending the pool's authenticated role at the same instant, and restoring A's ARN with a second UpdateIdentityPool makes the identical GetCredentialsForIdentity call vend again.
Emulator: the check never reaches any of those assertions — the replay reports 'Error: Check timed out after 180.0s', i.e. no verdict within the harness cap. The setup legs the check performs before the refusal (two cognito-idp user pools with confirmed users, two IAM CreateOpenIDConnectProvider registrations, CreateIdentityPool with both provider ARNs, SetIdentityPoolRoles, GetId and GetCredentialsForIdentity for both providers) are the same legs that pass in the sibling check of this service, so the stall is in the post-removal phase. Hypothesis: the check's refusal poll retries GetCredentialsForIdentity for 180 s and never receives a ClientError, because the emulator keeps vending through the removed provider — i.e. the vend path does not consult the pool's current OpenIdConnectProviderARNs, and the provider set is write-only state. The alternative reading of the same timeout is that one of the 180 s success-polls (the post-removal vend for provider B, or the vend after A is restored) never satisfies its predicate; either way the pool-provider lifecycle contract is unreachable in the emulator.
Why: this is an over-permissive authorization outcome. Removing an OIDC provider from an identity pool is the documented way to cut off a federation source; a client that performs that removal believes already-issued, still-valid tokens from that provider can no longer obtain AWS credentials for the pool's authenticated role. Against this emulator the removal has no effect that any caller can observe, so revocation cannot be tested and a security control that AWS enforces silently does nothing. A caller also gets no distinguishable error: instead of 403 NotAuthorizedException with the documented sentence, it either gets credentials it should not have or hangs.
Fix: the cognito-identity GetCredentialsForIdentity handler and the login/provider resolution it shares with GetId — each Logins key must be matched against the identity pool's CURRENT provider configuration (OpenIdConnectProviderARNs, resolved through the registered IAM OIDC provider's issuer URL, plus SupportedLoginProviders / CognitoIdentityProviders / SamlProviderARNs) at vend time, and a key whose provider is not in that set must be refused NotAuthorizedException (403) with the sentence above, while leaving the identity and its stored logins intact; UpdateIdentityPool's replacement of the set must be the state that gate reads.
Traces: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[mismatch] cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-oidc-provider-removed-by-update-identity-pool-rejected
  Error: Check timed out after 180.0s

The check's sequence and the assertions it never reached:
  1. two cognito-idp user pools (A, B), one app client and one confirmed user each; an ID token minted from each via AdminInitiateAuth.
  2. iam.CreateOpenIDConnectProvider for each token's 'iss' (issuer without scheme is the Logins key), ClientIDList=[that app client].
  3. cognito-identity.CreateIdentityPool(AllowUnauthenticatedIdentities=False, OpenIdConnectProviderARNs=[arn_a, arn_b]) + SetIdentityPoolRoles with an authenticated role trusting cognito-identity.amazonaws.com.
  4. positive controls: GetId + GetCredentialsForIdentity for A and for B each vend a session of that authenticated role (sts:GetCallerIdentity ARN contains ':assumed-role/<role>/').
  5. UpdateIdentityPool(..., OpenIdConnectProviderARNs=[arn_b]) must echo exactly [arn_b]; DescribeIdentityPool must then list exactly [arn_b].
  6. GetCredentialsForIdentity(IdentityId=<id for A>, Logins={<A issuer key>: <A token>}) must be refused:
       assert code == "NotAuthorizedException"
       assert message == "Token is not from a supported provider of this identity pool."
     (polled for up to 180 s FOR the refusal; a continued success is what keeps that poll alive.)
  7. provider B must still vend the authenticated role at the same instant.
  8. DescribeIdentity(IdentityId=<id for A>) must still report Logins == [<A issuer key>] (a refusal to vend, not an unlink).
  9. restoring OpenIdConnectProviderARNs=[arn_a, arn_b] must make the identical call in step 6 vend again.

Expected (AWS): step 6 answers HTTP 403, Error.Code 'NotAuthorizedException', Error.Message with the fixed text 'Token is not from a supported provider of this identity pool.' (no variable parts), while steps 7-9 all hold.
Actual (emulator): no verdict at all — the check is killed at the 180.0 s harness cap, so the refusal (its 403, its code and its sentence) is never produced within the window the check allows. The sibling check in this same service group gets as far as a successful vend on this wiring, so steps 1-4 are served; the stall is in the post-removal phase of the lifecycle.

Smithy citations: com.amazonaws.cognitoidentity#UpdateIdentityPool input/output OpenIdConnectProviderARNs -> OIDCProviderList (whole-set replacement, echoed in the response); com.amazonaws.cognitoidentity#GetCredentialsForIdentity errors include NotAuthorizedException (client, httpError 403).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. cognito-idp: create two user pools, each with one app client (ALLOW_ADMIN_USER_PASSWORD_AUTH) and one confirmed user with a permanent password; AdminInitiateAuth each to get an ID token. The Logins key for each is the token's 'iss' claim without its https:// scheme.
3. iam create-open-id-connect-provider --url <iss of token A> --client-id-list <client A>, and the same for B.
4. cognito-identity create-identity-pool --no-allow-unauthenticated-identities --open-id-connect-provider-arns <arnA> <arnB>; set-identity-pool-roles with an authenticated role trusting cognito-identity.amazonaws.com for that pool.
5. get-id + get-credentials-for-identity with Logins={<A key>: <A token>} and separately for B — both must vend a session of the authenticated role (control).
6. cognito-identity update-identity-pool --identity-pool-id <pool> --identity-pool-name <same name> --no-allow-unauthenticated-identities --open-id-connect-provider-arns <arnB>   (A removed); confirm the response and describe-identity-pool both list only <arnB>.
7. cognito-identity get-credentials-for-identity --identity-id <A's identity> --logins <A key>=<A token>
   AWS: HTTP 403, NotAuthorizedException, Message 'Token is not from a supported provider of this identity pool.'
   Emulator: no refusal arrives; the call is retried for 180 s and the scenario never reaches a verdict.
8. Also verify, as AWS does: provider B still vends; describe-identity for A still lists <A key>; restoring --open-id-connect-provider-arns <arnA> <arnB> makes step 7's call vend again.
9. Re-replay the check id in trace_ids.
