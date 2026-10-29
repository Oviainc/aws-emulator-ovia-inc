Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.role-mapping-token-type-uses-preferred-role
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.role-mapping-token-type-uses-preferred-role|run#35|AssertionError|assert recheck["Credentials"]["AccessKeyId"].startswith("ASIA"), (
Issue-Commit: 37c02434d582c8370b608107fbfd29504c12ec2b

# cognito-identity: GetCredentialsForIdentity under a Token role mapping vends an access key id prefixed 'EMULATOR' instead of an STS temporary-credential 'ASIA' key id (1 cause)

> _Note: this issue was identified against the code base at commit `37c02434d582c8370b608107fbfd29504c12ec2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One net-new root cause, grounded in a failing behavioral check of this suite that the replay report marked neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: the credential vended for a Token-role-mapping exchange does not carry the STS temporary-credential access-key-id prefix ===
AWS: Smithy (com.amazonaws.cognitoidentity) — GetCredentialsForIdentity returns `Credentials` (AccessKeyId -> AccessKeyString "The Access Key portion of the credentials", SecretKey, SessionToken, Expiration); modeled errors include NotAuthorizedException (httpError 403) and InvalidIdentityPoolConfigurationException (httpError 400). AccessKeyString carries no pattern, so the key-id SHAPE is recorded behavior only: the credential a real identity pool vends is the credential of an `sts:AssumeRoleWithWebIdentity` session, and the access key id of ANY temporary/session credential carries the STS temporary-credential key-type prefix `ASIA` — never a long-term `AKIA` and never a vendor literal. The recorded run asserts exactly that prefix on the key id re-vended for the group member after the claim-less login was denied; the rest of the key id is per-run random and is NOT a requirement.
Emulator: the vended `Credentials.AccessKeyId` begins with the literal string `EMULATOR` (observed `EMULATORASGBJLLK5GBC`; HTTP 200, no error). Everything else this check asserts on the path already matches: the Token mapping does assume the `cognito:preferred_role` group role (GetCallerIdentity shows `:assumed-role/<group-role>/`), and the claim-less login IS refused with `NotAuthorizedException` / `The ambiguous role mapping rules for: <provider-name> denied this request.` — so the ONLY divergence is the key-id prefix on the re-vend.
Why: a client that classifies a credential by its access-key-id key-type prefix — SDK and CLI credential-type detection, audit/compliance tooling, harnesses that must prove a caller is a role session rather than a long-term key, policy/condition logic keyed on the credential type — reads a Cognito-vended session as a non-session credential. Code that fails closed on "this is not a temporary credential" breaks against the emulator, and code that asserts the session nature of the vend passes only by accident.
Fix: the credential-minting component the cognito-identity GetCredentialsForIdentity handler calls when it assumes the resolved role (the shared vended-role-session minting used at services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:500-552, whose `vended.access_key_id` is returned verbatim as `Credentials.AccessKeyId`). It must generate the key id with the STS temporary-credential key-type prefix instead of an emulator literal; no other member of the response is at fault here.
Traces: cognito-identity.role-mapping-token-type-uses-preferred-role (assertion at check line 282, on the second, post-denial vend for the grouped identity).
Related open issues that do NOT cover this check (none carries its replay key, and the report marked it neither OWNED nor ALREADY FILED): 20261028T221339Z-cognito-identity-3-net-new-getcredentialsforidentity-root-causes.md (same symptom on the developer-identity and Rules-mapping paths), 20261026T234020Z-cognito-identity-4-net-new-root-causes-web.md (the web-identity path) and 20261028T040721Z-cognito-identity-cognito-idp-iam-getcredentialsforidentity-vends-an.md (the IAM-OIDC-registered-issuer path). A single fix in the shared minting component would close all of them, which is why this is filed as one cause rather than a new behavior.

## Evidence
cognito-identity.role-mapping-token-type-uses-preferred-role (verdict: mismatch)
  check line 282: assert recheck["Credentials"]["AccessKeyId"].startswith("ASIA")
  expected: the Token-mapping exchange re-vends credentials for the grouped identity whose Credentials.AccessKeyId carries the STS temporary-credential key-type prefix 'ASIA' (body after the prefix is per-run random)
  actual:   HTTP 200 with Credentials.AccessKeyId = 'EMULATORASGBJLLK5GBC' — literal prefix 'EMULATOR', i.e. not a temporary-credential key id
  AssertionError: the group member stopped working after the ungrouped login's denial: {'IdentityId': 'us-west-2:...', 'Credentials': {'AccessKeyId': 'EMULATORASGBJLLK5GBC', ...}}
  (the identity id, key-id body, secret, session token, expiration and request id in that response are per-run values and are not part of the requirement)

Controls on the same check that already PASS, which isolate the cause: GetIdentityPoolRoles echoes the Token mapping verbatim; the grouped token's vend assumes the cognito:preferred_role group role (GetCallerIdentity Arn contains ':assumed-role/<group-role>/'); the ungrouped token is refused with NotAuthorizedException and the message 'The ambiguous role mapping rules for: <provider-name> denied this request.' (fixed text 'The ambiguous role mapping rules for: ' + ' denied this request.', variable part = the role-mapping provider name from the request/pool configuration).

## Steps to Reproduce
1. Build and boot the emulator and replay cognito-identity.role-mapping-token-type-uses-preferred-role.
2. Manually: create a cognito-idp user pool and app client (ALLOW_ADMIN_USER_PASSWORD_AUTH), a group whose RoleArn is a role trusting cognito-identity.amazonaws.com for sts:AssumeRoleWithWebIdentity, and a confirmed user in that group; mint an ID token (it carries cognito:preferred_role = the group role and cognito:roles listing it).
3. CreateIdentityPool with CognitoIdentityProviders=[{ProviderName: cognito-idp.<region>.amazonaws.com/<user-pool-id>, ClientId: <client-id>, ServerSideTokenCheck: false}] and AllowUnauthenticatedIdentities=true.
4. SetIdentityPoolRoles with Roles{authenticated, unauthenticated} and RoleMappings{'<provider>:<client-id>': {Type: 'Token', AmbiguousRoleResolution: 'Deny'}}.
5. GetId + GetCredentialsForIdentity with the grouped user's ID token, then inspect Credentials.AccessKeyId: AWS returns a key id starting with 'ASIA'; the emulator returns one starting with 'EMULATOR'. The same holds on a second vend after an unrelated claim-less login has been denied.
