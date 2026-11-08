Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.concurrent-get-id-creates-distinct-identities cognito-identity.e2e-unauthenticated-identity-access-disabled-rejected cognito-identity.get-id-invalid-parameter-rejected cognito-identity.get-id-malformed-login-token-format-rejected cognito-identity.get-id-malformed-login-token-value-rejected cognito-identity.get-id-rejects-unauthenticated-when-disallowed cognito-identity.server-side-unauthenticated-role-blocked-without-unauth-access-enabled cognito-identity.unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity cognito-identity.deploy-identity-pool-cfn-get-id-succeeds cognito-identity.deploy-identity-pool-cfn-stack-behaves cognito-identity.deploy-identity-pool-cfn-deletion-removes-pool cognito-identity.deploy-identity-pool-cfn-role-mappings-enforced cognito-identity.deploy-identity-pool-cfn-update-role-mapping-takes-effect cognito-identity.deploy-cfn-stack-update-changes-provider-config-live cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected cognito-identity.role-mapping-token-type-uses-preferred-role
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.concurrent-get-id-creates-distinct-identities|run#7|AssertionError|assert n_success == n, f"expected all {n} concurrent GetId calls to succeed, got {n_success}"
Issue-Key: cognito-identity.e2e-unauthenticated-identity-access-disabled-rejected|run#7|AssertionError|assert err == ("NotAuthorizedException", "Unauthenticated access is not supported for this identity pool."), f"expected NotAuthorizedException, got {err!r}"
Issue-Key: cognito-identity.get-id-invalid-parameter-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}: {message!r}"
Issue-Key: cognito-identity.get-id-malformed-login-token-format-rejected|run#10|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.get-id-malformed-login-token-value-rejected|run#11|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.get-id-rejects-unauthenticated-when-disallowed|run#7|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.server-side-unauthenticated-role-blocked-without-unauth-access-enabled|run#9|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity|run#9|AssertionError|assert code == EXPECTED_CODE, (
Issue-Key: cognito-identity|DescribeIdentityPool|InvalidParameterException
Issue-Key: cognito-identity|GetIdentityPoolRoles|InvalidParameterException
Issue-Key: cognito-identity|CreateStack|ValidationError
Issue-Key: cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected|run#17|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: cognito-identity.role-mapping-token-type-uses-preferred-role|run#17|AssertionError|assert grouped_claims.get("cognito:preferred_role") == group_role_arn, (
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# cognito-identity: unsigned (anonymous) requests answer UnknownService and CloudFormation-deployed identity pools are unusable (5 causes)

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetId

## Description
This issue bundles FIVE distinct root causes reached through cognito-identity checks. All five must be fixed and re-replayed before closing.

=== Cause 1 of 5: unsigned (anonymous) cognito-identity requests are not routed at all — the emulator answers UnknownService ===
AWS: GetId is an anonymous-capable operation — the Smithy model binds it with an empty auth list and optionalAuth (smithy_query operation=GetId: "auth": [], "optionalAuth": true), which is exactly how unauthenticated mobile/web clients obtain an IdentityId. AWS dispatches such a request and answers with the operation's own semantics: HTTP 400 InvalidParameterException for a malformed IdentityPoolId/Logins value, HTTP 403 NotAuthorizedException when the pool has AllowUnauthenticatedIdentities=false, and HTTP 200 with a fresh IdentityId otherwise.
Emulator: any unsigned call is refused with error code UnknownService and the message "no emulated service is registered for 'awscognitoidentityservice'" — the Smithy service id, i.e. the request never reaches the cognito-identity service even though it IS bootstrapped and answers signed calls in this same run.
Why: the entire unauthenticated Cognito flow (GetId -> GetOpenIdToken -> GetCredentialsForIdentity) is unreachable, and every negative case on that path returns a code no SDK models, so clients cannot distinguish "pool forbids guests" from "service missing".
Fix: the request router / auth layer — resolve the target service for anonymous requests (operations with optionalAuth or an empty auth list) instead of requiring a sigv4 credential scope to select the service.
Related but NOT covering: 20261023T125454Z-cognito-idp-cognito-every-unsigned-anonymous-operation-answers-unknownservice-admininitiateauth.md and 20261022T220446Z-cognito-idp-initiateauth-is-refused-with-unknownservice-no-emulated-service.md own the cognito-idp side; the checks listed here were not marked as owned by either.
Traces: cognito-identity.get-id-invalid-parameter-rejected, cognito-identity.get-id-malformed-login-token-format-rejected, cognito-identity.get-id-malformed-login-token-value-rejected, cognito-identity.get-id-rejects-unauthenticated-when-disallowed, cognito-identity.unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity, cognito-identity.server-side-unauthenticated-role-blocked-without-unauth-access-enabled, cognito-identity.e2e-unauthenticated-identity-access-disabled-rejected, cognito-identity.concurrent-get-id-creates-distinct-identities.

=== Cause 2 of 5: Ref of a CloudFormation AWS::Cognito::IdentityPool resolves to the pool NAME, not the identity-pool id ===
AWS: Ref on AWS::Cognito::IdentityPool returns the identity pool's id, which the model constrains to @pattern ^[\w-]+:[0-9a-f-]+$ (IdentityPoolId, length 1-55) — i.e. '<region>:<guid>'. A stack Output built from !Ref can therefore be passed straight to DescribeIdentityPool/GetIdentityPoolRoles/GetId.
Emulator: the stack reaches CREATE_COMPLETE and the Output carries the template's IdentityPoolName value, so every follow-up cognito-identity call on it is refused with InvalidParameterException for failing the IdentityPoolId pattern.
Why: no CloudFormation-deployed identity pool can be addressed through the Cognito API; infrastructure-as-code flows are dead end-to-end even though the stack reports success.
Fix: the CloudFormation provider for AWS::Cognito::IdentityPool — create the pool in cognito-identity state and record the real pool id as the resource's physical id (what Ref/GetAtt and Outputs resolve to).
Related but NOT covering: 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md (missing OutputValue / synthesized ids in other services).
Traces: cognito-identity.deploy-identity-pool-cfn-get-id-succeeds, cognito-identity.deploy-identity-pool-cfn-stack-behaves, cognito-identity.deploy-identity-pool-cfn-deletion-removes-pool, cognito-identity.deploy-identity-pool-cfn-role-mappings-enforced, cognito-identity.deploy-identity-pool-cfn-update-role-mapping-takes-effect, cognito-identity.deploy-cfn-stack-update-changes-provider-config-live.

=== Cause 3 of 5: CloudFormation rejects AWS::Cognito::IdentityPoolPrincipalTag as an unrecognized resource type ===
AWS: AWS::Cognito::IdentityPoolPrincipalTag is a supported resource type; CreateStack accepts the template and provisions the principal-tag mapping (equivalent to SetPrincipalTagAttributeMap, which the cognito-identity model does define).
Emulator: CreateStack fails outright with HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Cognito::IdentityPoolPrincipalTag]".
Why: principal-tag based multi-tenant authorization cannot be deployed at all, and the refusal happens at CreateStack so nothing downstream can be exercised.
Fix: register the AWS::Cognito::IdentityPoolPrincipalTag resource type in the CloudFormation template/type registry and back it with the cognito-identity principal-tag state.
Traces: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access, cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected.

=== Cause 4 of 5: an invalid AWS::Cognito::IdentityPoolRoleAttachment does not fail its resource, so the stack never rolls back ===
AWS: a RoleMapping with Type=Rules and no RulesConfiguration is invalid (SetIdentityPoolRoles refuses it with InvalidParameterException), so the CloudFormation resource fails and the stack ends ROLLBACK_COMPLETE.
Emulator: the stack reports CREATE_COMPLETE and its only events are CREATE_IN_PROGRESS/CREATE_COMPLETE for the stack itself — the resource is never validated against cognito-identity.
Why: a template that cannot work in production deploys green here, and rollback handling in deployment pipelines is never exercised.
Fix: the AWS::Cognito::IdentityPoolRoleAttachment provider must apply the resource through the SetIdentityPoolRoles validation path and propagate the refusal into resource/stack failure and rollback.
Traces: cognito-identity.deploy-cognito-identity-2-cfn-rollback-identity-pool-rejected.

=== Cause 5 of 5: Cognito ID tokens carry no cognito:preferred_role claim for a group member, so Token-type role mappings cannot select a role ===
AWS: recorded behavior only — for a user in a user-pool group that has a RoleArn, the ID token carries cognito:roles and, when one group wins on precedence, cognito:preferred_role=<that group's RoleArn>; a cognito-identity RoleMapping of Type=Token then vends exactly that role.
Emulator: the decoded ID token has cognito:preferred_role = None (absent) for a group member whose group carries a RoleArn, so the Token-type mapping has nothing to resolve.
Why: group-driven role selection — the standard way to vend different IAM roles per user group — cannot work against the emulator.
Fix: ID-token claim construction in the Cognito token issuer (populate cognito:roles/cognito:preferred_role from group membership and precedence), and the Token-type branch of role resolution in the cognito-identity credentials path.
Traces: cognito-identity.role-mapping-token-type-uses-preferred-role.

## Evidence
Cause 1 — eight checks fail with the identical refusal on an unsigned client:
  botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the GetId operation: no emulated service is registered for 'awscognitoidentityservice'
  get-id-invalid-parameter-rejected: "expected ValidationException, got 'UnknownService'"; get-id-malformed-login-token-format-rejected / -value-rejected: "expected NotAuthorizedException, got 'UnknownService'"; get-id-rejects-unauthenticated-when-disallowed: "pool disallows unauthenticated identities; expected NotAuthorizedException for anonymous GetId, got 'UnknownService'"; unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity: "expected 'NotAuthorizedException' with unauthenticated identities disabled, got 'UnknownService'"; server-side-unauthenticated-role-blocked-without-unauth-access-enabled: same; e2e-unauthenticated-identity-access-disabled-rejected: "expected NotAuthorizedException, got ('UnknownService', ...)"; concurrent-get-id-creates-distinct-identities: "expected all 8 concurrent GetId calls to succeed, got 0".
  Model: smithy_query operation=GetId -> "auth": [], "optionalAuth": true (anonymous is a modeled access mode).

Cause 2 — template 'Resources: IdentityPool: Type: AWS::Cognito::IdentityPool ... Outputs: PoolId: Value: !Ref IdentityPool' reaches CREATE_COMPLETE, and the Output value is the pool NAME:
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the DescribeIdentityPool operation: Value 'sim2real-behav-pool-39e35db90f53' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w-]+:[0-9a-f-]+$
  Same against GetIdentityPoolRoles ('sim2real-behav-pool-da0a47018ed0', 'sim2real_cog_pool_0e4613e5ed10') and DescribeIdentityPool ('sim2real_cog_pool_6facc7539dd9', 'sim2real_cog_pool_87a0ddff9beb', 'sim2real-cip-96c562503c93') across the six deploy checks.

Cause 3 — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Cognito::IdentityPoolPrincipalTag]

Cause 4 — "stack whose role attachment has Type=Rules and no RulesConfiguration must end ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'; events: [(<stack>, 'CREATE_COMPLETE', ''), (<stack>, 'CREATE_IN_PROGRESS', 'User Initiated')]".

Cause 5 — "ID token for a group member must carry cognito:preferred_role=arn:aws:iam::<account>:role/sim2real-behav-grouprole-..., got None".

## Steps to Reproduce
1. Cause 1: build a cognito-identity client with UNSIGNED credentials and call GetId(IdentityPoolId=<a pool created with signed credentials>) -> expect 200 with an IdentityId (or 403 NotAuthorizedException when the pool has AllowUnauthenticatedIdentities=false); observe error code UnknownService, "no emulated service is registered for 'awscognitoidentityservice'".
2. Cause 2: CreateStack with a template declaring AWS::Cognito::IdentityPool and Outputs PoolId: !Ref IdentityPool; wait for CREATE_COMPLETE; pass the Output value to DescribeIdentityPool -> expect the pool; observe InvalidParameterException because the value is the pool name, not '<region>:<guid>'.
3. Cause 3: CreateStack with a template declaring AWS::Cognito::IdentityPoolPrincipalTag -> expect acceptance; observe 400 ValidationError "Unrecognized resource types".
4. Cause 4: CreateStack with an AWS::Cognito::IdentityPoolRoleAttachment whose RoleMapping has Type=Rules and no RulesConfiguration -> expect ROLLBACK_COMPLETE; observe CREATE_COMPLETE.
5. Cause 5: create a user pool group with a RoleArn, add a user, sign the user in, decode the ID token -> expect cognito:preferred_role=<group RoleArn>; observe the claim absent.
