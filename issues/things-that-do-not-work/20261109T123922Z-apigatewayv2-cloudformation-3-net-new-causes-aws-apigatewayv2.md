Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-managed-overrides-persists-after-removal apigatewayv2__cloudformation.deploy-apigatewayv2-portalproduct-displayname-over-maximum-rejected apigatewayv2__cloudformation.deploy-apigatewayv2-portalproduct-round-trip-then-delete apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-model-schema-type-rejected apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-response-and-model-removal
Issue-Key: apigatewayv2__cloudformation|CreateStack|ValidationError
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-model-schema-type-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-response-and-model-removal|run#25|AssertionError|assert got == [(outs["RouteResponseId"], "$default")], f"GetRouteResponses: {got!r}"
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# apigatewayv2__cloudformation: 3 net-new causes — AWS::ApiGatewayV2::{PortalProduct,ApiGatewayManagedOverrides} are unregistered resource types, an invalid Model schema type is accepted, and GetRouteResponses returns nothing for a created route response

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three independent causes behind 5 net-new failing apigatewayv2__cloudformation checks.

Cause 1 of 3: AWS::ApiGatewayV2::PortalProduct and AWS::ApiGatewayV2::ApiGatewayManagedOverrides are missing from the CloudFormation template resource-type registry.
AWS: recorded behavior only — CreateStack accepts both types; a PortalProduct whose DisplayName exceeds its documented maximum instead rolls the stack back with API Gateway V2's own length refusal, and ApiGatewayManagedOverrides is applied to the managed stage/route and its effect persists after the override resource is removed.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::ApiGatewayV2::PortalProduct]` / `[AWS::ApiGatewayV2::ApiGatewayManagedOverrides]`, so neither the round-trip, nor the length refusal, nor the override-persistence contract is reached.
Why: API Gateway V2 portal products and managed overrides cannot be provisioned from a template; the user is told these AWS resource types do not exist.
Fix: register both types in the CloudFormation template resource-type registry and add a resource handler per type over the apigatewayv2 crate (including the DisplayName length check and the managed-override merge semantics).
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-managed-overrides-persists-after-removal, apigatewayv2__cloudformation.deploy-apigatewayv2-portalproduct-displayname-over-maximum-rejected, apigatewayv2__cloudformation.deploy-apigatewayv2-portalproduct-round-trip-then-delete

Cause 2 of 3: an AWS::ApiGatewayV2::Model with an invalid schema type is provisioned instead of rolling the stack back.
AWS: recorded behavior only — a Model whose Schema declares a type outside the JSON-schema types API Gateway V2 accepts fails to create, and the stack settles ROLLBACK_COMPLETE.
Emulator: the stack settles 'CREATE_COMPLETE'.
Why: a model that cannot validate anything is installed on the API; the authoring error is never surfaced and request validation built on that model behaves differently from AWS.
Fix: the AWS::ApiGatewayV2::Model resource handler's Schema validation on create, and the rollback it must drive.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-model-schema-type-rejected

Cause 3 of 3: GetRouteResponses returns an empty list for a route response the stack created.
AWS: recorded behavior only — after a stack declaring AWS::ApiGatewayV2::RouteResponse reaches CREATE_COMPLETE, GetRouteResponses for the owning route returns that response; removing it on a later update makes the list empty again.
Emulator: GetRouteResponses answers `[]` while the stack reports the resource as created, so the removal half of the contract is unobservable.
Why: a client cannot read back or operate on a WebSocket route response created by a template, and cannot tell a created response from a removed one.
Fix: the AWS::ApiGatewayV2::RouteResponse resource handler's create path — it must write the route response into apigatewayv2 state so GetRouteResponses returns it.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-response-and-model-removal

## Evidence
Cause 1 — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ApiGatewayV2::ApiGatewayManagedOverrides] / [AWS::ApiGatewayV2::PortalProduct] (the PortalProduct message appears for both the round-trip and the over-maximum DisplayName check).

Cause 2 — AssertionError: rejected stack ended 'CREATE_COMPLETE'  (expected ROLLBACK_COMPLETE).

Cause 3 — AssertionError: GetRouteResponses: []  (expected the route response the stack created).

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring AWS::ApiGatewayV2::PortalProduct or AWS::ApiGatewayV2::ApiGatewayManagedOverrides; observe the 400 ValidationError "Unrecognized resource types".
2. Cause 2: CreateStack with an AWS::ApiGatewayV2::Model whose Schema uses an unsupported type; expect ROLLBACK_COMPLETE.
3. Cause 3: deploy a WebSocket API with AWS::ApiGatewayV2::Route + ::RouteResponse, then call GetRouteResponses for that route.
4. Re-replay the 5 checks in trace_ids.
