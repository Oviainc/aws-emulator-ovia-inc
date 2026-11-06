Issue-Severity: HIGH
Issue-Commit: 008e4d05d49cb2d9e1d34bb138fa3d8aa2ffd12d

# apigateway__cognito-identity__cognito-idp: a CloudFormation stack of Cognito user pool + app client + identity pool + IdentityPoolRoleAttachment now ends in ROLLBACK_COMPLETE instead of CREATE_COMPLETE (1 cause)

> _Note: this issue was identified against the code base at commit `008e4d05d49cb2d9e1d34bb138fa3d8aa2ffd12d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause, grounded in the single failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 1: CreateStack on a valid template whose resources are AWS::Cognito::UserPool, AWS::Cognito::UserPoolClient (UserPoolId: !Ref UserPool), AWS::Cognito::IdentityPool (CognitoIdentityProviders with ClientId: !Ref UserPoolClient and ProviderName: !GetAtt UserPool.ProviderName), AWS::IAM::Role and AWS::Cognito::IdentityPoolRoleAttachment (IdentityPoolId: !Ref IdentityPool, Roles.authenticated: !GetAtt AuthenticatedRole.Arn, RoleMappings with Type: Token) is accepted and then settles in ROLLBACK_COMPLETE: at least one resource in the chain fails to create, so the whole stack is rolled back and no Cognito resource survives. Failing check ids for this cause: apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive.

AWS: a stack whose resources are all well-formed and whose intrinsic references resolve reaches CREATE_COMPLETE, and its Outputs (here Value: !Ref UserPool, !Ref UserPoolClient, !Ref IdentityPool, !Ref AuthenticatedRole) are then readable. Smithy (cloudformation): DescribeStacks' StackStatus enum contains CREATE_COMPLETE, CREATE_FAILED and ROLLBACK_COMPLETE; CreateStack's OnFailure defaults to ROLLBACK, so ROLLBACK_COMPLETE is reported only when a resource create actually failed. The resource chain each provider must satisfy is modeled: cognito-identity-provider's UserPoolId member targets UserPoolIdType (@length 1..55, @pattern ^[\w-]+_[0-9a-zA-Z]+$) while a pool name is UserPoolNameType (@length 1..128, @pattern ^[\w\s+=,.@-]+$); cognito-identity's SetIdentityPoolRoles.IdentityPoolId targets IdentityPoolId (@length 1..55, @pattern ^[\w-]+:[0-9a-f-]+$, documented as "An identity pool ID in the format REGION:GUID") with required Roles and optional RoleMappings (map, 0..10). Those shapes are disjoint from the corresponding *name* shapes, so every provider in the chain must hand the next one the id the owning service minted, not a template name.

Emulator: CreateStack is accepted (the stack id is returned), DescribeStacks progresses, and the terminal status polled is ROLLBACK_COMPLETE — never CREATE_COMPLETE — so the check aborts at deploy time with "Stack <name> ended in ROLLBACK_COMPLETE" and no Output is ever read. The stack name in that message is a per-run generated value and is not part of the requirement. Hypothesis (neither the model nor the replay output states which resource failed, because the check does not read stack events): the physical resource id this provisioner assigns to AWS::Cognito::UserPool / AWS::Cognito::IdentityPool is the template's *name* property rather than the minted id (that defect is recorded, with CREATE_COMPLETE still being reached, by open issues 20261105T021658Z-apigateway-cognito-identity-cognito-idp-ref-of-a.md, 20261028T195228Z-cognito-identity-2-net-new-cloudformation-root-causes.md and 20261027T054912Z-cognito-identity-a-cloudformation-aws-cognito-identitypoolroleattachment-is.md), so the dependent provider now passes that name into CreateUserPoolClient/SetIdentityPoolRoles, whose UserPoolId/IdentityPoolId pattern no longer accepts it, the resource create fails and the stack rolls back. Those three issues all describe a stack that reaches CREATE_COMPLETE and only yields an unusable reference afterwards; none of them describes the stack failing to create at all, and none of them carries this check id as a replay key, so a fixer could close every one of them and this check would still fail.

Why: a client that provisions the documented Cognito federation topology with CloudFormation — user pool, app client, identity pool with the pool as its provider, and an IdentityPoolRoleAttachment mapping authenticated users onto an IAM role — gets no infrastructure at all. The failure is not a degraded reference it could work around: the stack is rolled back, so the user pool, app client, identity pool and role attachment do not exist, every downstream call (AdminCreateUser, GetId, GetCredentialsForIdentity, sts:GetCallerIdentity on the vended session) is unreachable, and the template is unusable rather than merely imprecise.

Fix: the CloudFormation provisioner's resource providers for the AWS::Cognito::* types (AWS::Cognito::UserPool, AWS::Cognito::UserPoolClient, AWS::Cognito::IdentityPool, AWS::Cognito::IdentityPoolRoleAttachment) and the physical-resource-id / Fn::GetAtt resolution they feed each other: each provider must create its resource through the owning service, record the id that service mints as the PhysicalResourceId, and resolve dependent properties (UserPoolId, ClientId, ProviderName, IdentityPoolId) from it so the chain creates successfully and the stack converges to CREATE_COMPLETE.

Traces: apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive.

## Evidence
Expected: CreateStack with the template below reaches CREATE_COMPLETE and its Outputs (UserPoolId, UserPoolClientId, IdentityPoolId, RoleName) resolve, after which AdminCreateUser / AdminInitiateAuth / GetId / GetCredentialsForIdentity succeed against them.

Actual (replay of apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive):

  File "check_apigateway__cognito_identity__cognito_idp_deploy_cognito_identity_pool_roles_stack_positive.py", line 93, in run
    stack = _deploy_cfn(ctx, template)
  File "check_apigateway__cognito_identity__cognito_idp_deploy_cognito_identity_pool_roles_stack_positive.py", line 217, in _deploy_cfn
    raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
  AssertionError: Stack sim2real-xsvc-stk-300c2c2ffa07 ended in ROLLBACK_COMPLETE

The harness polls DescribeStacks until the status is one of {CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED} and only then asserts CREATE_COMPLETE, so this is a settled terminal status and not a timeout. The stack name is a per-run generated value; the requirement is the terminal status.

Note on scope: open issue 20261105T021658Z-apigateway-cognito-identity-cognito-idp-ref-of-a.md lists this same check id under Issue-Trace-Ids, but it documents a stack that reaches CREATE_COMPLETE and then hands AdminCreateUser the UserPoolName (HTTP 400 InvalidParameterException on the userPoolId pattern). The replay report marked this check neither ALREADY FILED nor OWNED, and the observed behavior is now a different and strictly worse one — the stack never creates — so it is filed here as its own root cause.

## Steps to Reproduce
1. CreateStack (Capabilities: CAPABILITY_IAM, CAPABILITY_NAMED_IAM) with this template:

   AWSTemplateFormatVersion: "2010-09-09"
   Resources:
     UserPool:
       Type: AWS::Cognito::UserPool
       Properties: { UserPoolName: <name> }
     UserPoolClient:
       Type: AWS::Cognito::UserPoolClient
       Properties:
         UserPoolId: !Ref UserPool
         ClientName: <name>
         GenerateSecret: false
         ExplicitAuthFlows: [ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH]
     IdentityPool:
       Type: AWS::Cognito::IdentityPool
       Properties:
         IdentityPoolName: <name>
         AllowUnauthenticatedIdentities: false
         CognitoIdentityProviders:
           - ClientId: !Ref UserPoolClient
             ProviderName: !GetAtt UserPool.ProviderName
             ServerSideTokenCheck: false
     AuthenticatedRole:
       Type: AWS::IAM::Role
       Properties:
         AssumeRolePolicyDocument:
           Version: "2012-10-17"
           Statement:
             - Effect: Allow
               Principal: { Federated: cognito-identity.amazonaws.com }
               Action: sts:AssumeRoleWithWebIdentity
               Condition:
                 StringEquals: { "cognito-identity.amazonaws.com:aud": !Ref IdentityPool }
                 "ForAnyValue:StringLike": { "cognito-identity.amazonaws.com:amr": authenticated }
     IdentityPoolRoleAttachment:
       Type: AWS::Cognito::IdentityPoolRoleAttachment
       Properties:
         IdentityPoolId: !Ref IdentityPool
         Roles: { authenticated: !GetAtt AuthenticatedRole.Arn }
         RoleMappings:
           cognitoProvider:
             Type: Token
             AmbiguousRoleResolution: AuthenticatedRole
             IdentityProvider: !Sub "cognito-idp.${AWS::Region}.amazonaws.com/${UserPool}:${UserPoolClient}"
   Outputs:
     UserPoolId:       { Value: !Ref UserPool }
     UserPoolClientId: { Value: !Ref UserPoolClient }
     IdentityPoolId:   { Value: !Ref IdentityPool }
     RoleName:         { Value: !Ref AuthenticatedRole }

2. Poll DescribeStacks(StackName=<stack>) every 5 s until StackStatus is terminal.
3. Observe StackStatus == ROLLBACK_COMPLETE (expected CREATE_COMPLETE) and no Outputs.
4. (Diagnosis) DescribeStackEvents(StackName=<stack>) to see which AWS::Cognito::* resource reports CREATE_FAILED and with which refusal; check that each provider passed the dependent resource the id its service minted rather than the template's name property.
