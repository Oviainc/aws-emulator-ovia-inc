Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected|_error#39|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 5bdc422e571ff089197010dd8b23a6f4d1e72235

# cognito-identity: GetOpenIdTokenForDeveloperIdentity mints a developer token on a pool configured for RoleMappings instead of refusing it with InvalidParameterException

> _Note: this issue was identified against the code base at commit `5bdc422e571ff089197010dd8b23a6f4d1e72235`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpenIdTokenForDeveloperIdentity

## Description
This issue carries ONE net-new root cause reached through a cognito-identity behavioral check in suite simparity-behavioral-aws-v1. It must be fixed and the trace_ids list re-replayed before closing.

=== Cause 1 of 1: a pool whose identity-pool role attachment set RoleMappings still mints developer-identity tokens ===

AWS: GetOpenIdTokenForDeveloperIdentity is the backend-credentials path that mints an OpenID Connect token for a developer (backend-authenticated) user of an identity pool. When that pool's roles configuration carries RoleMappings (i.e. SetIdentityPoolRoles / an AWS::Cognito::IdentityPoolRoleAttachment put at least one entry into RoleMappings), the role a session gets must be resolved from the token's claims, which a developer-minted token cannot carry — so AWS refuses the operation outright rather than minting a token. Recorded behavior only (the Smithy model does not state this precondition; GetOpenIdTokenForDeveloperIdentity's documentation describes only the Logins/IdentityId linking rules): the refusal is the modelled client error InvalidParameterException (Smithy: GetOpenIdTokenForDeveloperIdentity errors include InvalidParameterException, errorKind client, httpError 400; InvalidParameterException doc "Thrown for missing or bad input parameter(s).") with HTTP 400 and the message, whose text is entirely fixed for this condition (no variable parts taken from the request or state): "Invalid identity pool configuration. When a pool is configured for RoleMappings, GetOpenIdTokenForDeveloperIdentity may not be used."

Emulator: the call SUCCEEDS with HTTP 200. On a pool created by a CloudFormation stack whose AWS::Cognito::IdentityPoolRoleAttachment declared RoleMappings {"userpool": {IdentityProvider: "cognito-idp.<region>.amazonaws.com/<user pool id>:<client id>", Type: "Token", AmbiguousRoleResolution: "AuthenticatedRole"}}, GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<that pool>, Logins={<pool's DeveloperProviderName>: <developer user identifier>}) returns an IdentityId and a freshly minted Token (an unsigned JWT with alg "none", iss https://cognito-identity.amazonaws.com, aud = the pool id, sub = the identity id and amr ["authenticated", "cognito-identity.amazonaws.com", <developer provider name>]) instead of raising InvalidParameterException. No error is produced at all, so the emulator never consults the pool's role-mapping configuration on this path.

Why: a client relying on AWS's behavior gets a token for a flow AWS forbids, and the forbidden flow is the one that bypasses role mapping. The whole point of RoleMappings on a pool is that the role a session receives is decided by the claims of a provider-issued token; a developer-minted token has no such claims, so AWS blocks the mint. Against the emulator the backend can mint the token, exchange it, and obtain the pool's authenticated role unconditionally — a configuration an operator believes is claim-gated is silently open. Symmetrically, negative tests and guard rails that assert this refusal pass against AWS and fail against the emulator, and code written against the emulator breaks the first time it meets real Cognito, where the mint is rejected with a 400.

Fix: the GetOpenIdTokenForDeveloperIdentity handler's precondition check (before any identity is resolved or token minted) — read the pool's stored roles configuration and, when its RoleMappings map is non-empty, return the modelled InvalidParameterException (HTTP 400) carrying AWS's fixed sentence for this condition. It belongs with the handler's other pool-level refusals, not in the token minter.

Related but NOT covering: open issue 20261027T054912Z-cognito-identity-a-cloudformation-aws-cognito-identitypoolroleattachment-is.md is grounded in the SAME check but on an earlier assertion (its replay key is the check's `assert bad_status == "ROLLBACK_COMPLETE"` step) and is scoped to the CloudFormation provisioner arm for AWS::Cognito::IdentityPoolRoleAttachment plus SetIdentityPoolRoles validation of Rules-without-RulesConfiguration. The check now runs past those assertions and fails later, on this GetOpenIdTokenForDeveloperIdentity precondition, which no open issue names.

Traces: failing check cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected [mismatch]. Code the fix belongs in: services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_open_id_token_for_developer_identity.rs (the handle body's pool-level preconditions) against the pool roles state written by services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/set_identity_pool_roles.rs.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (259/334 passed). Failing check cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected, verdict [mismatch], at check line 135:

  code = _error(lambda: cog.get_open_id_token_for_developer_identity(
      IdentityPoolId=ok_pool_id, Logins={ok_dev: "sim2real-user-1"}))
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'IdentityId': 'us-west-2:fd484aef-...', 'Token': 'eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0...', 'ResponseMetadata': {..., 'HTTPStatusCode': 200, ...}}

Expected (what the check asserts): a ClientError whose (Code, Message) is exactly
  ("InvalidParameterException",
   "Invalid identity pool configuration. When a pool is configured for RoleMappings, GetOpenIdTokenForDeveloperIdentity may not be used.")
with the modelled HTTP status 400. Every word of that message is fixed text for this condition; nothing in it comes from the request or from state, so there is no per-run value in it.

Observed: HTTP 200 with IdentityId + Token. The pool under test is the control stack's pool, whose live roles configuration the same check then confirms through GetIdentityPoolRoles: exactly one RoleMappings key, "<provider name>:<client id>" (a Token-type mapping with AmbiguousRoleResolution=AuthenticatedRole). So the pool IS configured for RoleMappings at the moment of the call and the emulator still mints.

Model facts used (smithy_query on services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json):
- GetOpenIdTokenForDeveloperIdentity input: IdentityPoolId @required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$, Logins @required LoginsMap @length{0,10}, IdentityId, PrincipalTags, TokenDuration @range{1,86400}; output IdentityId + Token (sensitive).
- Its modelled errors: DeveloperUserAlreadyRegisteredException 400, InternalErrorException 500, InvalidParameterException 400, NotAuthorizedException 403, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429 — so InvalidParameterException (400) is the modelled channel for this refusal.
- The operation's documentation says nothing about pools configured for RoleMappings, hence "recorded behavior only" for the precondition itself; the status, code and message above are taken from the recorded AWS response the check encodes.

The check's SERVICES list is ["cognito-identity"] and the suite reports it under the heading '### cognito-identity'; that service crate (service-com-amazonaws-cognitoidentity-awscognitoidentityservice) is bootstrapped in this checkout (nothing is pending), so this is a fidelity divergence of a serving service.

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay suite simparity-behavioral-aws-v1; re-replay cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected to confirm a fix.
2. Create an identity pool with a DeveloperProviderName, e.g. cognito-identity CreateIdentityPool(IdentityPoolName=<name>, AllowUnauthenticatedIdentities=false, DeveloperProviderName=<dev provider>, CognitoIdentityProviders=[{ProviderName: "cognito-idp.<region>.amazonaws.com/<region>_AbCdEfGhI", ClientId: "<client id>", ServerSideTokenCheck: false}]). (The check reaches the same state through a CloudFormation stack with AWS::Cognito::IdentityPool + AWS::IAM::Role + AWS::Cognito::IdentityPoolRoleAttachment; either path is fine.)
3. Attach roles WITH a role mapping: SetIdentityPoolRoles(IdentityPoolId=<pool>, Roles={"authenticated": <auth role arn>}, RoleMappings={"<provider name>:<client id>": {Type: "Token", AmbiguousRoleResolution: "AuthenticatedRole"}}).
4. Confirm the mapping is live: GetIdentityPoolRoles(IdentityPoolId=<pool>) must report exactly the one RoleMappings key "<provider name>:<client id>".
5. Call GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<dev provider>: "<developer user identifier>"}). Expect HTTP 400 InvalidParameterException with message "Invalid identity pool configuration. When a pool is configured for RoleMappings, GetOpenIdTokenForDeveloperIdentity may not be used."; observe HTTP 200 with an IdentityId and a minted Token.
6. Control, same pool: call SetIdentityPoolRoles again with Roles only and NO RoleMappings, then repeat step 5 — the mint must now succeed (200 with IdentityId + Token), so the new precondition keys off the presence of RoleMappings and nothing else.
