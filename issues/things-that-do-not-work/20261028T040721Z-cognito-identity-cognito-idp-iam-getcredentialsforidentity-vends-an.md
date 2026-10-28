Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials|run#31|AssertionError|assert creds["AccessKeyId"].startswith("ASIA"), (
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# cognito-identity__cognito-idp__iam: GetCredentialsForIdentity vends an access key id prefixed 'EMUL' instead of a temporary STS 'ASIA' key id for an IAM-OIDC-federated login

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One root cause, grounded in 1 failing behavioral check whose check-id leading token is the composite 'cognito-identity__cognito-idp__iam'. The failure is NOT marked ALREADY FILED or OWNED in the replay report.

Related open issues that do NOT carry this replay key: 20261026T234020Z-cognito-identity-4-net-new-root-causes-web.md (its Cause 1 is the same prefix defect, but observed on the CLASSIC flow — GetOpenIdToken + sts:AssumeRoleWithWebIdentity — for check cognito-identity.classic-flow-open-id-token-assume-role-with-web-identity, under service token 'cognito-identity') and 20261025T005354Z-cognito-identity-getcredentialsforidentity-never-performs-a-real-assumerolewithwebidentity.md (GetCredentialsForIdentity vending static emulator IAM-user credentials, grounded in other checks). Neither owns this check, so this entry carries its replay key; a fixer closing the key-id minting for the classic flow should re-replay this check too.

Cause 1 of 1: the ENHANCED (simplified) identity-pool flow vends a credential whose AccessKeyId is not a temporary-credential key id. For an identity federated through an IAM OIDC provider registered for a Cognito user pool issuer, GetCredentialsForIdentity returns Credentials.AccessKeyId beginning with the literal 'EMUL' instead of AWS's documented temporary-credential prefix 'ASIA'.

AWS: Smithy com.amazonaws.cognitoidentity#GetCredentialsForIdentity (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json) — input IdentityId (required, @pattern ^[\w-]+:[0-9a-f-]+$), Logins, CustomRoleArn; output IdentityId + Credentials, where Credentials.AccessKeyId targets AccessKeyString ('The Access Key portion of the credentials'). The model constrains the prefix nowhere, so this is documented behavior plus recorded behavior: the credentials GetCredentialsForIdentity returns are a temporary STS session of the identity pool's authenticated role, and AWS documents the unique-identifier prefix for temporary access key ids as `ASIA` (IAM User Guide, 'IAM identifiers' -> unique ID prefixes: ASIA = temporary access key id, AKIA = long-term IAM user key). So the key id's FIXED text is 'ASIA'; the characters after it are per-run random values and are not part of any requirement. The same vended keys must be a session of the mapped authenticated role — GetCallerIdentity on them reports an ARN containing ':assumed-role/<role>/'.

Emulator: GetCredentialsForIdentity returns Credentials with a SessionToken (so the call succeeds, HTTP 200) but Credentials.AccessKeyId begins with the fixed literal 'EMUL' (the check asserts on the first four characters and saw 'EMUL'). The diverging part is the fixed prefix, not the per-run tail.

Why: a client relying on AWS's behavior mis-classifies the credential. SDKs, credential providers and tooling decide from the key-id prefix whether a credential is temporary (must be refreshed before Expiration, must be signed with a session token), credential scrubbers and validators key off it, and policy/condition logic and audit rules written against key-id shape see a long-term IAM user key where AWS returns a session key. Any client or test asserting the documented prefix fails, and the whole IAM-OIDC federation path (vend credentials -> sts:GetCallerIdentity -> assert an assumed-role session) is unusable against this emulator.

Fix: the credential-minting component behind GetCredentialsForIdentity — the cognito-identity GetCredentialsForIdentity handler and the STS session minting it delegates to (AssumeRoleWithWebIdentity credential generation) — must emit an 'ASIA'-prefixed access key id of legal length for every temporary session, instead of an emulator-branded literal, and that session must be a session of the pool's mapped authenticated role.

Traces: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials (line 238: AssertionError: vended credentials must be temporary STS keys (ASIA…), got 'EMUL').

## Evidence
Replay report, service cognito-identity__cognito-idp__iam:

  [mismatch] cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials
  File "check_...iam_oidc_registered_user_pool_issuer_vends_sts_credentials.py", line 238, in run
    assert creds["AccessKeyId"].startswith("ASIA")
  AssertionError: vended credentials must be temporary STS keys (ASIA…), got 'EMUL'

Everything before this point passed: CreateOpenIDConnectProvider for the user pool issuer, GetOpenIDConnectProvider echoing Url/ClientIDList, CreateIdentityPool with OpenIdConnectProviderARNs, DescribeIdentityPool, idempotent GetId, ListIdentities, and GetCredentialsForIdentity itself returning a Credentials structure with a SessionToken for the right IdentityId.

Expected (AWS): Credentials.AccessKeyId begins with the fixed text 'ASIA' (documented prefix for a temporary/STS access key id), followed by per-run random characters; signing with those keys and calling sts:GetCallerIdentity returns an ARN containing ':assumed-role/<authenticated-role>/'.

Actual (emulator): Credentials.AccessKeyId begins with the fixed text 'EMUL'. Only the prefix is at issue — the trailing characters are a per-run value and are not a requirement. The assertion on the assumed-role ARN at the next line was never reached.

Smithy citation: GetCredentialsForIdentity output Credentials -> com.amazonaws.cognitoidentity#Credentials with member AccessKeyId -> AccessKeyString; the model places no prefix constraint, so the 'ASIA' prefix is AWS-documented behavior (IAM identifiers / unique ID prefixes) rather than a model constraint.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite against the emulator.
2. cognito-idp: CreateUserPool + CreateUserPoolClient; iam: CreateOpenIDConnectProvider with Url=https://cognito-idp.<region>.amazonaws.com/<poolId> and ClientIDList=[<clientId>].
3. cognito-identity: CreateIdentityPool with AllowUnauthenticatedIdentities=False and OpenIdConnectProviderARNs=[<provider arn>]; SetIdentityPoolRoles with an authenticated role trusting cognito-identity.amazonaws.com.
4. Obtain an ID token for a user in that pool and call GetId with Logins={'cognito-idp.<region>.amazonaws.com/<poolId>': <token>}, then GetCredentialsForIdentity with the same Logins.
5. Inspect Credentials.AccessKeyId: it must begin with 'ASIA'; the emulator returns an 'EMUL'-prefixed id.
6. Sign a request with the vended keys and call sts:GetCallerIdentity — the ARN must contain ':assumed-role/<authenticated role>/'.
7. Re-replay cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-registered-user-pool-issuer-vends-sts-credentials.
