Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured|run#35|AssertionError|assert f":assumed-role/{mapped_name}/" in enhanced_arn, (
Issue-Commit: 32af0d92e59e46f2fdf169ea1f28f8cf7b2e0f6a

# cognito-identity: GetCredentialsForIdentity ignores a configured RoleMappings entry and vends the pool's default authenticated role, so server-side role selection is bypassed even though the basic flow is correctly refused for it

> _Note: this issue was identified against the code base at commit `32af0d92e59e46f2fdf169ea1f28f8cf7b2e0f6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
ONE root cause (Cause 1 of 1).

AWS: when an identity pool carries a `RoleMappings` entry for the provider key the identity authenticated with, GetCredentialsForIdentity resolves the role through that mapping — for `Type: Rules`, the first matching `MappingRule`'s `RoleARN` — and the vended credentials are an sts:AssumeRoleWithWebIdentity session of THAT role, observable through sts:GetCallerIdentity answering `arn:<partition>:sts::<account-id>:assumed-role/<mapped-role-name>/CognitoIdentityCredentials`. The pool's `Roles{authenticated}` entry is only the fallback used when no mapping resolves. Smithy (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): SetIdentityPoolRoles/GetIdentityPoolRoles carry `RoleMappings` (map of provider key -> RoleMapping{Type, AmbiguousRoleResolution, RulesConfiguration.Rules}), and `RulesConfigurationType.Rules` is documented "Rules are evaluated in order. The first one to match specifies the role." The assumed-role ARN shape is recorded behavior; the variable parts are the account id and the mapped role's NAME, both taken from the request/emulator state — the session name `CognitoIdentityCredentials` is fixed text.
Emulator: the mapping is stored and read back correctly (GetIdentityPoolRoles reports the entry just set, and the pool-wide refusal that depends on it works — GetOpenIdToken on the same pool IS refused with 400 InvalidParameterException "Basic (classic) flow is not supported with RoleMappings, please use enhanced flow.", so RoleMappings is not merely dropped on write), but the credential path never consults it: the enhanced-flow vend for the very same identity resolves to the pool's default authenticated role, `arn:aws:sts::<account>:assumed-role/<pool authenticated role>/CognitoIdentityCredentials`, instead of the mapped role.
Why: this is the security hole the basic-flow refusal exists to close, reopened from the other side. The pool is explicitly configured so that the SERVER chooses the role from the token's claims; the emulator instead hands every authenticated identity the single pool-wide role, so claim-based privilege separation (admin vs member, tenant vs tenant) silently does not happen. A client whose authorization model depends on the mapped role gets a session for a different role, and the operator's RBAC configuration is observable only as inert storage — the escalation passes against the emulator and only diverges in production.
Fix: role resolution in the GetCredentialsForIdentity handler (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs) — look up the pool's stored `RoleMappings` for the identity's provider key and apply the resolved `RoleARN` (first matching rule in declared order for Type=Rules, honoring AmbiguousRoleResolution) before falling back to `Roles{authenticated|unauthenticated}`.
Traces: cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured (beats 1-3 of that check pass: the basic-flow refusal and its message are correct; beat 4, the enhanced-flow vend under the same mapping, is what diverges).
Related open issue, NAMED because a fixer should close them together: 20261026T054328Z-cognito-identity-4-net-new-root-causes-a.md describes the same resolution gap as its Cause 2 from a different check (get-identity-pool-roles-rules-evaluated-in-declared-order, rule ORDER). This check is not marked OWNED by it and carries a different replay key, so it is filed here; if that issue's Cause 2 is fixed properly this check should pass with it and can be closed as the same fix. Also related but not covering: 20261025T005354Z-cognito-identity-getcredentialsforidentity-never-performs-a-real-assumerolewithwebidentity.md (its Cause 1 was that NO role was assumed at all; the emulator now does mint an assumed-role session, so what is left is which role it picks).

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1; quotation from the failing-check list.

cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured [mismatch]:
  expected: with a RoleMappings entry in place for the user-pool provider key, the enhanced flow vends a session of the MAPPED role — sts:GetCallerIdentity Arn contains ":assumed-role/<mapped-role-name>/".
  actual:   "AssertionError: with the mapping in place the enhanced flow must vend the MAPPED role sim2real-behav-classic-mapped-<id> (that is what the basic flow was refused for bypassing), got arn:aws:sts::000000000000:assumed-role/sim2real-behav-classic-auth-<id>/CognitoIdentityCredentials" — i.e. the pool's default authenticated role, not the mapped one. (The role names and account id are per-run values; the requirement is that the role in the ARN is the one the mapping resolves to.)

Earlier beats of the same check PASSED against the emulator, which is what localizes the defect to role resolution rather than to RoleMappings storage:
  - SetIdentityPoolRoles with the RoleMappings entry succeeded and GetIdentityPoolRoles reported the entry under the provider:client mapping key.
  - GetOpenIdToken against that pool was refused with the expected 400 InvalidParameterException and the exact message "Basic (classic) flow is not supported with RoleMappings, please use enhanced flow."
  - GetCredentialsForIdentity returned Credentials with a SessionToken, and sts:GetCallerIdentity under them returned a genuine assumed-role session ARN (only the ROLE is wrong).

Model (smithy_query, cognito-identity): GetCredentialsForIdentity errors ExternalServiceException 400, InvalidIdentityPoolConfigurationException 400, InvalidParameterException 400, NotAuthorizedException 403, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429; SetIdentityPoolRoles/GetIdentityPoolRoles carry RoleMappings -> RoleMapping{Type, AmbiguousRoleResolution, RulesConfiguration{Rules: list<MappingRule>}}.

Note on provenance: the check file is marked DRAFT/unblessed for its refusal hypotheses, but the refusal legs it hypothesized PASS here; the assertion that fails is the documented RoleMappings role-selection contract above, which the Smithy documentation of RulesConfigurationType.Rules states directly.

## Steps to Reproduce
1. Build and boot the emulator, then replay cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured.
2. CreateIdentityPool(AllowUnauthenticatedIdentities=False, AllowClassicFlow=True, CognitoIdentityProviders=[{ProviderName: cognito-idp.<region>.amazonaws.com/<user-pool-id>, ClientId: <client-id>}]).
3. Create two IAM roles, AUTH and MAPPED, both trusting cognito-identity.amazonaws.com for sts:AssumeRoleWithWebIdentity with the pool as aud.
4. SetIdentityPoolRoles(IdentityPoolId=<pool>, Roles={"authenticated": <AUTH arn>}, RoleMappings={}) and confirm the basic/enhanced flows work.
5. SetIdentityPoolRoles(IdentityPoolId=<pool>, Roles={"authenticated": <AUTH arn>}, RoleMappings={"cognito-idp.<region>.amazonaws.com/<user-pool-id>:<client-id>": {Type: "Rules", AmbiguousRoleResolution: "Deny", RulesConfiguration: {Rules: [{Claim: ..., MatchType: "Equals", Value: <a value the ID token carries>, RoleARN: <MAPPED arn>}]}}}) and confirm GetIdentityPoolRoles reports the entry.
6. GetId then GetCredentialsForIdentity with Logins={<provider>: <ID token>}, and call sts:GetCallerIdentity with the vended credentials.
   Expect: Arn == arn:aws:sts::<account>:assumed-role/<MAPPED role name>/CognitoIdentityCredentials.
   Observe: Arn == arn:aws:sts::<account>:assumed-role/<AUTH role name>/CognitoIdentityCredentials.
