Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.deploy-apigw-cognito-authorizer-wrong-provider-arns-negative apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive apigateway__cognito-identity__cognito-idp.deploy-cognito-userpool-apigw-authorizer-stack-positive
Issue-Key: cognito-identity-provider|AdminCreateUser|InvalidParameterException
Issue-Commit: 36206cbe376b716dfba9f39501a1a5d68dd7648c

# apigateway__cognito-identity__cognito-idp: Ref of a CloudFormation AWS::Cognito::UserPool returns the UserPoolName instead of the minted user-pool id, so every deployed Cognito scenario dies on its first cognito-idp call (1 cause)

> _Note: this issue was identified against the code base at commit `36206cbe376b716dfba9f39501a1a5d68dd7648c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminCreateUser

## Description
One root cause, grounded in three failing behavioral checks that the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 1: the physical resource id the CloudFormation provisioner assigns to an AWS::Cognito::UserPool is the template's UserPoolName, not the pool id the Cognito service mints, so `!Ref <UserPool>` (and any stack Output wired to it) hands the caller a value no cognito-idp operation will accept.

AWS: the physical resource id of AWS::Cognito::UserPool is the user-pool id, and `Ref` returns that id (`GetAtt <pool>.ProviderName`/`.Arn` are derived from it). Smithy (cognito-identity-provider): every operation's `UserPoolId` member — including AdminCreateUser.UserPoolId, AdminSetUserPassword.UserPoolId and AdminInitiateAuth.UserPoolId — targets `UserPoolIdType`, @length 1..55, @pattern `^[\w-]+_[0-9a-zA-Z]+$` (a region-prefixed id such as `<region>_<alnum>`), while the pool's name is `UserPoolNameType`, @length 1..128, @pattern `^[\w\s+=,.@-]+$`. The two shapes are disjoint for any name without an underscore. On AWS, a stack Output declared as `Value: !Ref UserPool` therefore resolves to a pool id, and the subsequent AdminCreateUser / AdminSetUserPassword / AdminInitiateAuth calls succeed.

Emulator: the Output resolves to the UserPoolName the template supplied (the recorded values are the checks' generated pool names, e.g. a `sim2real-xsvc-…pool-<hex>` name — the name is a per-run value, the defect is that a *name* is returned where an *id* is required). The very next call, AdminCreateUser, is then refused with HTTP 400 `InvalidParameterException` and the message `1 validation error detected: Value '<the returned value>' at 'userPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+_[0-9a-zA-Z]+`. In that message the fixed text is `1 validation error detected: Value '…' at 'userPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: …`; the variable parts are the rejected value (from the request) and the member name. The refusal is correct *for the value it was given* — the value is what is wrong, so the fix is not in cognito-idp's validator.

Why: a client that deploys a Cognito user pool with CloudFormation and then drives it through the Cognito API — the documented pattern for every one of these templates (user pool + app client + API Gateway COGNITO_USER_POOLS authorizer, or user pool + identity pool role attachment) — cannot address the pool it just created. Nothing downstream is reachable: no user can be created, no ID token minted, and the authorizer/identity-pool behavior the stack exists to exercise is never observed. The stack reports CREATE_COMPLETE, so the failure surfaces only as a validation refusal on an unrelated-looking operation.

Fix: the AWS::Cognito::UserPool resource provider in the CloudFormation provisioner — create the pool through the owning cognito-idp service and store the id that service mints as the PhysicalResourceId (the value `Ref` and `Fn::GetAtt`-derived attributes resolve from), rather than echoing the UserPoolName property. Related open issue 20261024T114943Z-cognito-idp-cloudformation-provisioned-user-pools-app-clients.md records the same provisioner defect as its Cause 1, but under service `cognito-idp` and with replay keys on DescribeUserPool / DescribeUserPoolClient / GetIdentityProviderByIdentifier only; it carries none of the three check ids below, so a fixer could close it without ever re-running them. This issue exists to carry those keys.

Traces: apigateway__cognito-identity__cognito-idp.deploy-apigw-cognito-authorizer-wrong-provider-arns-negative, apigateway__cognito-identity__cognito-idp.deploy-cognito-identity-pool-roles-stack-positive, apigateway__cognito-identity__cognito-idp.deploy-cognito-userpool-apigw-authorizer-stack-positive.

## Evidence
Identical failure shape in all three checks — the stack reaches CREATE_COMPLETE, the Output declared `Value: !Ref UserPool` is read, and the first cognito-idp call with it is refused:

  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the AdminCreateUser operation: 1 validation error detected: Value 'sim2real-xsvc-realpool-8047fd807b2f' at 'userPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+_[0-9a-zA-Z]+
  … Value 'sim2real-xsvc-pool-ef564334c426' at 'userPoolId' …
  … Value 'sim2real-xsvc-pool-52d6b202b866' at 'userPoolId' …

In each case the rejected value is exactly the `UserPoolName` property the template set for that resource (the checks generate it via ctx.unique_name, so the literal string is a per-run value and not part of the requirement).

Expected: the Output resolves to the user-pool id the service minted — a value matching `UserPoolIdType` @pattern ^[\w-]+_[0-9a-zA-Z]+$ — and AdminCreateUser, AdminSetUserPassword and AdminInitiateAuth all succeed against it.
Actual: the Output resolves to the pool name; AdminCreateUser answers HTTP 400 InvalidParameterException on the userPoolId pattern, and the check aborts before any authorizer or identity-pool behavior is exercised.

## Steps to Reproduce
1. CreateStack with a template containing:
     Resources:
       UserPool:
         Type: AWS::Cognito::UserPool
         Properties: { UserPoolName: <any name without an underscore> }
       UserPoolClient:
         Type: AWS::Cognito::UserPoolClient
         Properties: { UserPoolId: !Ref UserPool, ClientName: <name>, GenerateSecret: false, ExplicitAuthFlows: [ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH] }
     Outputs:
       UserPoolId: { Value: !Ref UserPool }
2. Poll DescribeStacks until CREATE_COMPLETE (it reaches CREATE_COMPLETE).
3. Read the `UserPoolId` output value -> observe it is the UserPoolName from step 1, not an id of the form <region>_<alnum>.
4. Call cognito-idp AdminCreateUser(UserPoolId=<that output>, Username=<name>, MessageAction='SUPPRESS') -> expect success; observe HTTP 400 InvalidParameterException "1 validation error detected: Value '<name>' at 'userPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+_[0-9a-zA-Z]+".
