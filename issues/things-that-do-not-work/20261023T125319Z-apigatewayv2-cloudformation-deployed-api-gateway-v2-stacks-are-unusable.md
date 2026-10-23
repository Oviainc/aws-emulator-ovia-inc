Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deploy-apigatewayv2-1-dangling-route-target-rejected-rolls-back-api apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected
Issue-Key: apigatewayv2|GetApi|NotFoundException
Issue-Key: apigatewayv2__cloudformation|GetApi|NotFoundException
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys|_wait_stack#37|AssertionError|raise AssertionError(f"stack {stack!r} reached {status!r}; failures {reasons!r}")
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected|_wait_stack#18|AssertionError|raise AssertionError(f"stack {stack!r} reached {status!r}; failures {reasons!r}")
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# CloudFormation-deployed API Gateway v2 stacks are unusable: Ref of AWS::ApiGatewayV2::Api yields the API name, and generated IAM role names exceed 64 chars

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two root causes in the CloudFormation provisioning path that apigatewayv2 deploy checks exercise. Both must be closed before submitting.

Cause 1 of 2: `Ref`/PhysicalResourceId of an `AWS::ApiGatewayV2::Api` resource resolves to the API NAME instead of the ApiId.
AWS: `Ref` on AWS::ApiGatewayV2::Api returns the API id (the same opaque 10-character `ApiId` CreateApi returns), and that id is the resource's PhysicalResourceId in DescribeStackResource / stack events. Downstream apigatewayv2 calls take that value as `ApiId` — Smithy types GetApi's ApiId as an httpLabel string and models NotFoundException 404 with the request-derived message `Invalid API identifier specified <account>:<api-id>`.
Emulator: the stack output `!Ref HttpApi` (and the HttpApi event's PhysicalResourceId) comes back as the API's Name, so the immediately-following `GetApi(ApiId=<that value>)` fails with HTTP 404 `NotFoundException`, message example `Invalid API identifier specified 000000000000:sim2real-apigwv2-cfn-ok-4fe7e25fa325` — i.e. the stack name/API name was passed where an ApiId was expected. The same shape appears for a CFN-generated name (`…-Api-0B53596C2B90`).
Why: every template that wires an Integration/Route/Stage/Lambda permission by `!Ref` to the Api, or exports the ApiId as an output, hands its consumers an unusable identifier; no CFN-deployed API Gateway v2 stack can be read back or invoked, and a rollback cannot be verified.
Fix: the CloudFormation resource provider for AWS::ApiGatewayV2::Api in the cloudformation service crate — set PhysicalResourceId (and therefore Ref) to the ApiId returned by CreateApi, not the resource's Name property.
Traces: apigatewayv2.deploy-apigatewayv2-1-dangling-route-target-rejected-rolls-back-api, apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced, apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes

Cause 2 of 2: CloudFormation-generated physical names for `AWS::IAM::Role` exceed IAM's 64-character RoleName limit, so CDK-synthesised stacks fail to create.
AWS: when a template omits RoleName, CloudFormation generates the physical name itself and guarantees it satisfies the resource's own constraint — IAM `CreateRole.RoleName` is length 1–64 (IAM rejects longer names with ValidationException). CDK-synthesised stacks routinely omit RoleName, and these stacks create cleanly on AWS.
Emulator: the generated name is `<stack-name>-<LogicalId>-<suffix>` with no length bound, e.g. a 68-character `…-FnServiceRoleB9001A96-C3BADC320479`, and IAM then refuses it, so the stack reaches CREATE_FAILED with `The following resource(s) failed to create: [FnServiceRoleB9001A96]. ValidationError: 1 validation error detected: Value '<name>' at 'roleName' failed to satisfy constraint: Member must have length less than or equal to 64`.
Why: any CDK/CFN stack whose stack name plus logical id exceeds the budget cannot be deployed at all — the emulator fails a template that AWS accepts, and the failure is attributed to the customer's template rather than the name generator.
Fix: the physical-name generator in the CloudFormation resource-provisioning layer — truncate/hash the generated name to the target resource's documented maximum (64 for IAM RoleName) the way CloudFormation does.
Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys, apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected

## Evidence
Cause 1 — expected: `describe_stacks(...)['Outputs']['ApiIdOut']` (= `!Ref HttpApi`) is an ApiId that GetApi resolves. Actual: `botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetApi operation: Invalid API identifier specified 000000000000:sim2real-apigwv2-cfn-ok-4fe7e25fa325` — the value returned by Ref is the API/stack name. Same for `…:sim2real-apigwv2-cfn-14fe0b6903a9` and `…:sim2real-xsvc-bodyimport-stk-2503793d97b4-Api-0B53596C2B90`.

Cause 2 — expected: CREATE_COMPLETE. Actual: `AssertionError: stack 'sim2real-apigwv2-cdk3-ce506d99b3af' reached 'CREATE_FAILED'; failures ["… The following resource(s) failed to create: [FnServiceRoleB9001A96]. ValidationError: 1 validation error detected: Value 'sim2real-apigwv2-cdk3-ce506d99b3af-FnServiceRoleB9001A96-C3BADC320479' at 'roleName' failed to satisfy constraint: Member must have length less than or equal to 64"]` (68 characters; the concrete names/suffixes vary per run).

## Steps to Reproduce
Cause 1: create a stack containing an `AWS::ApiGatewayV2::Api` with an output `Value: !Ref TheApi`; read the output (or DescribeStackResource's PhysicalResourceId) and call apigatewayv2:GetApi(ApiId=<it>). Expect the API; observe NotFoundException because a name was returned.
Cause 2: deploy a CDK-synthesised stack whose AWS::IAM::Role omits RoleName and whose stack name + logical id would generate a >64-character name. Expect CREATE_COMPLETE; observe CREATE_FAILED on the role.
Re-replay the five check ids in trace_ids to confirm.
