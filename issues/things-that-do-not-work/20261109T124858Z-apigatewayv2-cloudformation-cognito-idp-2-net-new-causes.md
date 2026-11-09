Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__cognito-idp.deploy-apigatewayv2-cfn-authorizer-identitysource-type-rejected apigatewayv2__cloudformation__cognito-idp.deploy-apigatewayv2-cfn-jwt-authorizer-reads-back-and-removed
Issue-Key: apigatewayv2__cloudformation__cognito-idp.deploy-apigatewayv2-cfn-authorizer-identitysource-type-rejected|run#10|AssertionError|assert rows == [
Issue-Key: apigatewayv2__cloudformation__cognito-idp.deploy-apigatewayv2-cfn-jwt-authorizer-reads-back-and-removed|run#14|AssertionError|assert [p["Id"] for p in pools] == [out["PoolId"]], f"ListUserPools found {pools!r}"
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# apigatewayv2__cloudformation__cognito-idp: 2 net-new causes — a rolled-back stack records no PROPERTY_VALIDATION row for a wrongly-typed property, and a CREATE_COMPLETE stack's AWS::Cognito::UserPool is invisible to ListUserPools

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in CloudFormation deployment of an API Gateway v2 authorizer + Cognito user pool template.

Cause 1 of 2: a template-validation failure leaves no validation row on the stack's events.
AWS: recorded behavior only (stack-event detail is not Smithy-modelled). A stack whose AWS::ApiGatewayV2::Authorizer declares IdentitySource as a String instead of a JSONArray rolls back AND records exactly one resource validation row: logical id 'Authorizer', type PROPERTY_VALIDATION, status FAILED, detail FAIL, message 'Property [IdentitySource] expected type: JSONArray, found: String', path '/Resources/Authorizer/Properties/IdentitySource'. The fixed text is 'Property [<member>] expected type: <expected>, found: <actual>' with the member name, expected type and found type taken from the template; the path is the member's template pointer.
Emulator: the stack does reach ROLLBACK_COMPLETE, but the validation rows are empty ([]) — no PROPERTY_VALIDATION record, so the reason the deployment failed is nowhere in the stack's events.
Why: a client that deploys a template and reads the stack events to learn WHICH property it typed wrongly gets nothing; CI that surfaces the failing property path cannot report it, and the rollback looks unexplained.
Fix: the cloudformation stack-event/detailed-status recording path for template property validation (the resource-validation rows emitted alongside DescribeStackEvents). Related open issues that do NOT cover this check: 20261108T003043Z-cloudformation-iam-lambda-logs-a-stack-whose-resource.md, 20261108T140221Z-cloudformation-iam-lambda-logs-a-failed-createstack-emits.md.
Traces: apigatewayv2__cloudformation__cognito-idp.deploy-apigatewayv2-cfn-authorizer-identitysource-type-rejected.

Cause 2 of 2: a user pool declared by a CREATE_COMPLETE stack does not exist in Cognito.
AWS: recorded behavior only for the CFN resource mapping; Smithy: ListUserPools returns UserPools[].{Id,Name} for every pool in the region. A stack that reaches CREATE_COMPLETE with an AWS::Cognito::UserPool resource has created that pool, so ListUserPools lists exactly one pool whose Name is the template's PoolName and whose Id equals the stack output that Refs the resource.
Emulator: after CREATE_COMPLETE, ListUserPools finds no pool with the template's name at all ([]), while the stack output still carries a pool id — the stack's Ref value names a pool that was never materialised.
Why: anything that reads the provisioned estate back through the Cognito API (describe the pool, create a client/user, point a JWT authorizer at it) fails against a stack the emulator reported as complete; the control plane and CloudFormation disagree about what exists.
Fix: the cloudformation resource provider for AWS::Cognito::UserPool — it must create the pool in cognito-idp state (and make its Ref the real pool id), not synthesize a physical id.
Traces: apigatewayv2__cloudformation__cognito-idp.deploy-apigatewayv2-cfn-jwt-authorizer-reads-back-and-removed.

## Evidence
Cause 1 — AssertionError: unexpected validation rows [] (expected [('Authorizer','PROPERTY_VALIDATION','FAILED','FAIL','Property [IdentitySource] expected type: JSONArray, found: String','/Resources/Authorizer/Properties/IdentitySource')]).
Cause 2 — AssertionError: ListUserPools found [] (expected [the stack output PoolId]).

## Steps to Reproduce
1) Cause 1: CreateStack with an AWS::ApiGatewayV2::Authorizer whose IdentitySource is a plain string; wait for ROLLBACK_COMPLETE and read the stack's resource validation rows — AWS has one PROPERTY_VALIDATION FAILED row naming /Resources/Authorizer/Properties/IdentitySource, the emulator has none.
2) Cause 2: CreateStack with AWS::Cognito::UserPool (+ApiGatewayV2 Api/Authorizer), wait for CREATE_COMPLETE, then ListUserPools and filter by the template's PoolName — the pool is absent even though the stack output Refs an id.
