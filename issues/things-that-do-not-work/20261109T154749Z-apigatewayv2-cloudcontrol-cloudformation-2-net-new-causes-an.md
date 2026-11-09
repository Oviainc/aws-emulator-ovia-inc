Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-api-unknown-property-rejected-with-positive-control apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol
Issue-Key: apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-api-unknown-property-rejected-with-positive-control|run#9|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {bad_status!r}"
Issue-Key: apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol|run#11|AssertionError|assert status == "CREATE_COMPLETE", f"expected CREATE_COMPLETE, got {status!r}"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__cloudcontrol__cloudformation: 2 net-new causes — an unknown property on AWS::ApiGatewayV2::Api reaches CREATE_COMPLETE instead of failing schema validation, and an Api declared by Body with a read-only ApiEndpoint override rolls the stack back

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the CloudFormation/Cloud Control view of AWS::ApiGatewayV2::Api. Both must be closed.

Cause 1 of 2: a template property that the AWS::ApiGatewayV2::Api resource schema does not define is accepted, and the stack reaches CREATE_COMPLETE.
AWS: recorded behavior only (resource-schema validation is not in the Smithy model). CloudFormation validates the resource against its registered schema before provisioning: the stack ends ROLLBACK_COMPLETE, the stack's own CREATE_FAILED event carries ResourceStatusReason "Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation." (fixed text; the error count is the number of schema violations), and DescribeEvents reports one VALIDATION_ERROR row (LogicalResourceId 'Api', ValidationName 'PROPERTY_VALIDATION', ValidationStatus 'FAILED', ValidationFailureMode 'FAIL', ValidationStatusReason "Unsupported property [<PropertyName>]", ValidationPath "/Resources/<LogicalId>/Properties/<PropertyName>" — the property name and path come from the template). No api is created.
Emulator: the stack with the extra property ends 'CREATE_COMPLETE'; no validation row and no failure reason are produced, and the api is created.
Why: a template AWS refuses deploys cleanly here, so typo'd or version-skewed templates pass in the emulator and fail in production, and the deploy-time negative contract (rollback + property path) cannot be relied on.
Fix: resource-schema property validation in the cloudformation crate's AWS::ApiGatewayV2::Api resource handler/registry (reject unsupported properties, emit the PROPERTY_VALIDATION event and the stack-level failure reason, then roll back).
Traces: apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-api-unknown-property-rejected-with-positive-control.

Cause 2 of 2: an AWS::ApiGatewayV2::Api declared only by its inline OpenAPI Body, with the read-only ApiEndpoint also present in Properties, rolls the stack back instead of creating the api.
AWS: recorded behavior only. The stack reaches CREATE_COMPLETE: the write-only Body creates the api (Name/ProtocolType taken from the OpenAPI document, here info.title and HTTP) and its routes (GET /probe from the document's path + x-amazon-apigateway-integration), the read-only ApiEndpoint in Properties is ignored so Fn::GetAtt ApiEndpoint returns API Gateway's own computed value (host labels <api-id>.execute-api.<region>), Fn::GetAtt ExecuteApiArn is arn:aws:execute-api:<region>:<account>:<api-id>, and Cloud Control GetResource's model exposes ApiId/Name/ApiEndpoint but omits the write-only Body.
Emulator: the stack ends 'ROLLBACK_COMPLETE', so none of the Ref/GetAtt/Cloud-Control representation behaviour is reached. Hypothesis: the resource handler refuses either the read-only ApiEndpoint property or the Body-only declaration instead of ignoring the former and importing the latter.
Why: a CFN or CDK estate that declares its API from an OpenAPI document cannot be deployed at all against the emulator, and the read-only/write-only property contract Cloud Control clients depend on is unobservable.
Fix: the AWS::ApiGatewayV2::Api resource handler in the cloudformation crate (ignore read-only properties, import Body via the apigatewayv2 ImportApi path) and the Cloud Control resource model projection for that type.
Related but not covering: 20261026T235900Z-apigatewayv2-cloudformation-2-net-new-root-causes-aws.md and 20261027T192238Z-apigatewayv2-cloudformation-a-stack-whose-aws-apigatewayv2-api.md describe Body-declared Api failures under the apigatewayv2__cloudformation service and own only those keys.
Traces: apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol.

## Evidence
Cause 1 — expected: CreateStack with Resources.Api.Properties.ThisPropertyDoesNotExist -> ROLLBACK_COMPLETE, stack CREATE_FAILED reason "Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.", one DescribeEvents VALIDATION_ERROR row ('Api','PROPERTY_VALIDATION','FAILED','FAIL','Unsupported property [ThisPropertyDoesNotExist]','/Resources/Api/Properties/ThisPropertyDoesNotExist'), and GetApis showing no api of that name. Actual: AssertionError "expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'".
Cause 2 — expected: CreateStack with Resources.Api.Properties = {Body: <OpenAPI>, ApiEndpoint: '<bogus>'} -> CREATE_COMPLETE, then Ref = ApiId, Fn::GetAtt ApiEndpoint == GetApi's ApiEndpoint (and != the template override), Fn::GetAtt ExecuteApiArn == arn:aws:execute-api:<region>:<account>:<api-id>, GetRoutes == ['GET /probe'], Cloud Control GetResource properties carry ApiId/Name/ApiEndpoint and no Body. Actual: AssertionError "expected CREATE_COMPLETE, got 'ROLLBACK_COMPLETE'".

## Steps to Reproduce
1. CreateStack with a single AWS::ApiGatewayV2::Api whose Properties are {Name, ProtocolType: HTTP, RouteSelectionExpression, ThisPropertyDoesNotExist: 1}; poll DescribeStacks — it reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE. Replay apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-api-unknown-property-rejected-with-positive-control.
2. CreateStack with a single AWS::ApiGatewayV2::Api whose Properties are {Body: <inline OpenAPI 3.0.1 with one MOCK-integrated GET /probe>, ApiEndpoint: 'https://sim2real-bogus.example.com'} and Outputs for Ref/Fn::GetAtt ApiEndpoint/ExecuteApiArn; poll DescribeStacks — it reaches ROLLBACK_COMPLETE. Replay apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol.
