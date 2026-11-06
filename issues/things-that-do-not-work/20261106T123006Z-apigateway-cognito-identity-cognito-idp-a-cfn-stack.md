Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 0d41ee472df788e58645875c19630e6a52d7394d

# apigateway__cognito-identity__cognito-idp: a CFN stack of Cognito user pool + app client + identity pool rolls back because the UserPoolClient Ref is a hyphenated CFN physical name, not a Cognito client id (1 cause)

> _Note: this issue was identified against the code base at commit `0d41ee472df788e58645875c19630e6a52d7394d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause, grounded in the single failing check under the suite heading `apigateway__cognito-identity__cognito-idp` that the replay report marks neither ALREADY FILED nor OWNED.

Related open issues, named as required: 20261106T100902Z-apigateway-cognito-identity-cognito-idp-a-cloudformation-stack.md records the same check id and the same terminal ROLLBACK_COMPLETE, but it only hypothesises the failing resource (it guesses the UserPool/IdentityPool physical id is the pool NAME) and does not name the resource that actually fails or its refusal; 20261106T081405Z-cloudformation-cognito-identity-sts-aws-cognito-userpoolclient-s.md and 20261106T090935Z-cloudformation-cognito-identity-sts-an-aws-cognito-userpoolclient.md record the client-id shape defect under a DIFFERENT suite heading (cloudformation__cognito-identity__sts) and do not carry this check's replay key. This filing re-grounds the cause in this heading's failing check id and names the concrete failing resource and reason seen in this run's sibling checks.

Cause 1 of 1: the physical resource id the CloudFormation provisioner assigns to AWS::Cognito::UserPoolClient — the value {"Ref": "UserPoolClient"} resolves to — is the generic synthesized CFN physical name of the form <stack-name>-<logical-id>-<hex suffix>, not the app-client id minted by cognito-idp. The template under test feeds that Ref into AWS::Cognito::IdentityPool's CognitoIdentityProviders[].ClientId, whose value then fails cognito-identity's clientId pattern, the IdentityPool resource ends CREATE_FAILED, and the whole stack settles ROLLBACK_COMPLETE, so no Cognito estate exists and the check aborts at deploy time. Failing check id for this cause: apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive.

AWS: a Cognito app client id is an opaque service-minted token — Smithy com.amazonaws.cognitoidentityprovider#ClientIdType: @length 1..128, @pattern ^[\w+]+$, @sensitive, returned as UserPoolClientType.ClientId — and `Ref` on AWS::Cognito::UserPoolClient returns that client id. Handing it to an identity pool is the modeled wiring: Smithy com.amazonaws.cognitoidentity#CognitoIdentityProvider.ClientId targets CognitoIdentityProviderClientId, @length 1..128, @pattern ^[\w_]+$ (doc: "The client ID for the Amazon Cognito user pool."). Because a real client id contains no hyphen it satisfies that constraint by construction, so a well-formed template of UserPool + UserPoolClient + IdentityPool + IAM::Role + IdentityPoolRoleAttachment reaches CREATE_COMPLETE (Smithy cloudformation: CreateStack's OnFailure defaults to ROLLBACK, so ROLLBACK_COMPLETE is reported only when a resource create really failed).

Emulator: DescribeStacks settles on ROLLBACK_COMPLETE instead of CREATE_COMPLETE and no Output is readable. In the sibling cognito-identity CFN checks of this same run the stack events for the identical wiring name the cause exactly: the IdentityPool resource is CREATE_FAILED with ResourceStatusReason `Resource handler returned message: "1 validation error detected: Value '<stack>-UserPoolClient-<hex>' at 'cognitoIdentityProviders.1.member.clientId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w_]+" (RequestToken: <per-run id>, HandlerErrorCode: InvalidRequest)`. In that message the fixed text (`N validation error(s) detected: Value ... at ... failed to satisfy constraint: Member must satisfy regular expression pattern: ...`), the member path and the pattern are AWS's; the quoted value, the stack name, the hex suffix and the RequestToken are per-run values, not requirements. The CreateIdentityPool refusal itself is correct — the value it is handed is wrong — so the defect is upstream, in the UserPoolClient provider's physical id.

Why: no template that creates a user pool client and hands its id to anything can deploy. The client gets no infrastructure at all (not a degraded reference it could work around): the user pool, app client, identity pool and role attachment are rolled back, so every downstream call in the documented Cognito federation topology (AdminCreateUser, AdminInitiateAuth, GetId, GetCredentialsForIdentity, the API Gateway Cognito authorizer built on it) is unreachable. The failure also surfaces inside an unrelated resource, pointing a user at their IdentityPool properties rather than at the client id.

Fix: the CloudFormation resource provider for AWS::Cognito::UserPoolClient — its PhysicalResourceId (and therefore Ref) must be the ClientId returned by the cognito-idp CreateUserPoolClient call it performs, and that id generator must emit a value matching ^[\w+]+$, instead of the generic stack+logical-id+suffix physical-name generator used for resources whose physical id IS a name. No change belongs in cognito-identity's CreateIdentityPool validation or in its constraint-violation message.

Traces: apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive.

## Evidence
[mismatch] apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive

Expected (AWS): CreateStack with the template below reaches CREATE_COMPLETE and its Outputs (UserPoolId, UserPoolClientId, IdentityPoolId, RoleName) resolve, after which the user-pool/identity-pool flow runs against them.

Actual (emulator):
  File "check_apigateway__cognito_identity__cognito_idp_deploy_cognito_identity_pool_roles_stack_positive.py", line 93, in run
    stack = _deploy_cfn(ctx, template)
  File "...", line 217, in _deploy_cfn
    raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
  AssertionError: Stack <generated-name> ended in ROLLBACK_COMPLETE

The harness polls DescribeStacks until the status is terminal, so this is a settled status and not a timeout. The stack name is a per-run value; the requirement is the terminal status.

Corroborating stack events from the identical wiring in the same run (cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access):
  ('<stack>', 'The following resource(s) failed to create: [IdentityPool]. ')
  ('IdentityPool', 'Resource handler returned message: "1 validation error detected: Value \'<stack>-UserPoolClient-<hex>\' at \'cognitoIdentityProviders.1.member.clientId\' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w_]+" (RequestToken: <per-run id>, HandlerErrorCode: InvalidRequest)')

The divergence is the SHAPE of the Ref/physical id (it contains '-').

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive.
2. Manual repro: cloudformation:CreateStack (Capabilities CAPABILITY_IAM, CAPABILITY_NAMED_IAM) with AWS::Cognito::UserPool; AWS::Cognito::UserPoolClient (UserPoolId: !Ref UserPool, GenerateSecret: false, ExplicitAuthFlows [ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH]); AWS::Cognito::IdentityPool (AllowUnauthenticatedIdentities: false, CognitoIdentityProviders: [{ClientId: !Ref UserPoolClient, ProviderName: !GetAtt UserPool.ProviderName, ServerSideTokenCheck: false}]); AWS::IAM::Role trusted by cognito-identity.amazonaws.com; AWS::Cognito::IdentityPoolRoleAttachment (IdentityPoolId: !Ref IdentityPool, Roles.authenticated: !GetAtt AuthenticatedRole.Arn, RoleMappings cognitoProvider Type: Token).
3. Poll DescribeStacks: the emulator settles ROLLBACK_COMPLETE (expected CREATE_COMPLETE) and no Outputs exist.
4. DescribeStackEvents: IdentityPool is CREATE_FAILED with the cognitoIdentityProviders.1.member.clientId pattern violation quoting <stack>-UserPoolClient-<hex>.
5. DescribeStackResource(LogicalResourceId='UserPoolClient'): PhysicalResourceId contains '-'; it must equal the ClientId returned by cognito-idp CreateUserPoolClient and match ^[\w+]+$.
6. After the fix the stack reaches CREATE_COMPLETE and Output UserPoolClientId equals that ClientId.
