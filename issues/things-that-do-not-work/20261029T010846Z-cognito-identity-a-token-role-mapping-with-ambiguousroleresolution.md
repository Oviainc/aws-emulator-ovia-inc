Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.role-mapping-token-type-uses-preferred-role
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.role-mapping-token-type-uses-preferred-role|run#33|AssertionError|assert denied.get("Message") == (
Issue-Commit: d05383a7a9fbb10dc2241138b0670096478c83a6

# cognito-identity: a Token role mapping with AmbiguousRoleResolution=Deny refuses with the message 'Role resolution failed.' instead of AWS's ambiguous-role-mapping message (1 cause)

> _Note: this issue was identified against the code base at commit `d05383a7a9fbb10dc2241138b0670096478c83a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One net-new root cause in the cognito-identity GetCredentialsForIdentity role-mapping refusal path, grounded in a failing behavioral check of this suite that the replay report marked neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: the NotAuthorizedException raised when a Token-type role mapping resolves ambiguously under AmbiguousRoleResolution=Deny carries the wrong message text ===
AWS: Smithy (com.amazonaws.cognitoidentity) — SetIdentityPoolRoles accepts RoleMappings of RoleMapping{Type, AmbiguousRoleResolution, RulesConfiguration}; RoleMapping.Type documents "Token will use cognito:roles and cognito:preferred_role claims from the Cognito identity provider token to map groups to roles", and AmbiguousRoleResolution "specifies the action to be taken if either no rules match the claim value for the Rules type, or there is no cognito:preferred_role claim and there are multiple cognito:roles matches for the Token type" (enum AmbiguousRoleResolutionType: AuthenticatedRole | Deny). GetCredentialsForIdentity models NotAuthorizedException (httpError 403, "Thrown when a user is not authorized to access the requested resource"). The model does not fix the message string, so the message itself is recorded behavior only: real Cognito Identity refuses a login whose token cannot be resolved to a role under a Deny mapping with code NotAuthorizedException and message `The ambiguous role mapping rules for: <provider-name> denied this request.` The FIXED text of that message is `The ambiguous role mapping rules for: ` … ` denied this request.`; the single VARIABLE part is the role-mapping provider name taken from the request/pool configuration (for a user-pool provider, `cognito-idp.<region>.amazonaws.com/<user-pool-id>`) — never a literal captured from one run.
Emulator: the refusal is classified correctly (Error.Code = NotAuthorizedException) but the message is the fixed, provider-less string `Role resolution failed.` — it contains none of AWS's fixed wording and names no provider, so the whole message diverges, not just a per-run value.
Why: the message is the only part of this refusal that tells an operator WHICH mapped provider denied the request; AWS callers log, assert on, and surface it to end users. A client (or a conformance/assertion harness) that matches AWS's documented refusal wording treats the emulator's refusal as an unknown failure, and an operator debugging a Deny mapping across several configured providers loses the provider attribution entirely. Code-level handling still works, message-level handling does not.
Fix: the cognito-identity GetCredentialsForIdentity handler's role-resolution component — specifically the error construction on the AmbiguousRoleResolution=Deny branch (for both Token and Rules mappings), which must build the NotAuthorizedException message from AWS's fixed template with the role-mapping provider name of the login being resolved interpolated, rather than emitting a constant string.
Traces: cognito-identity.role-mapping-token-type-uses-preferred-role.
Related but NOT covering: open issue 20261028T221339Z-cognito-identity-3-net-new-getcredentialsforidentity-root-causes.md (Cause 2) reports the RULES-type Deny path answering the wrong error CODE (InvalidIdentityPoolConfigurationException/400 instead of NotAuthorizedException/403) and never vending for a matching rule; here the Token-type path answers the right code with the wrong message, and that issue does not carry this check's replay key — the report marked this check neither OWNED nor ALREADY FILED.

## Evidence
cognito-identity.role-mapping-token-type-uses-preferred-role (check_cognito_identity_role_mapping_token_type_uses_preferred_role.py:267):
  assert denied.get("Message") == (f"The ambiguous role mapping rules for: {provider} denied this request.")
  AssertionError: expected the exact refusal message AWS returns, got 'Role resolution failed.'

Expected (AWS): HTTP 403, Error.Code = 'NotAuthorizedException', Error.Message = 'The ambiguous role mapping rules for: cognito-idp.<region>.amazonaws.com/<user-pool-id> denied this request.'
  fixed text: 'The ambiguous role mapping rules for: ' + ' denied this request.'
  variable part: the role-mapping provider name from the request / pool configuration
Actual (emulator): Error.Code = 'NotAuthorizedException' (correct), Error.Message = 'Role resolution failed.' (no fixed AWS wording, no provider name)

Context from the same check that did NOT diverge: the Token mapping is stored and echoed verbatim by GetIdentityPoolRoles (Type=Token, AmbiguousRoleResolution=Deny, no RulesConfiguration), and the grouped login is vended the cognito:preferred_role group role — the earlier assertions passed, so the divergence is isolated to the refusal message for the login that carries no cognito:preferred_role.

## Steps to Reproduce
1. cognito-idp: CreateUserPool + CreateUserPoolClient (GenerateSecret=false, ExplicitAuthFlows ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH). Create one group with a RoleArn and Precedence=0.
2. iam: create three roles with the same trust policy allowing sts:AssumeRoleWithWebIdentity for Principal Federated cognito-identity.amazonaws.com with cognito-identity.amazonaws.com:aud = <identity pool id> (pool default authenticated, unauthenticated, group role).
3. cognito-identity: CreateIdentityPool with CognitoIdentityProviders=[{ProviderName: 'cognito-idp.<region>.amazonaws.com/<user-pool-id>', ClientId: <client-id>, ServerSideTokenCheck: false}].
4. SetIdentityPoolRoles(Roles={authenticated, unauthenticated}, RoleMappings={'<provider>:<client-id>': {Type: 'Token', AmbiguousRoleResolution: 'Deny'}}).
5. Mint an ID token for a user in NO group (AdminCreateUser + AdminSetUserPassword + AdminInitiateAuth ADMIN_USER_PASSWORD_AUTH) — it carries no cognito:preferred_role.
6. GetId(Logins={provider: token}) then GetCredentialsForIdentity(IdentityId, Logins={provider: token}).
7. Expect HTTP 403 NotAuthorizedException with Message 'The ambiguous role mapping rules for: <provider> denied this request.'; the emulator returns Message 'Role resolution failed.'
