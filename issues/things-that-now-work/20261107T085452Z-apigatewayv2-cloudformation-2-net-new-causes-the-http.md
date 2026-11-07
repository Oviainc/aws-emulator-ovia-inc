Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes|run#19|AssertionError|assert json.loads(text) == {"routeKey": "GET /alpha", "rawPath": "/alpha"}, (
Issue-Key: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change|_update_cfn#27|AssertionError|raise AssertionError(
Issue-Commit: ad889ea564c74ff66a8d5d7080e6fcd3e62d9684

# apigatewayv2__cloudformation: 2 net-new causes — the HTTP API payload's rawPath carries the $default stage segment, and a stack update fails on its AWS::Lambda::Permission resource

> _Note: this issue was identified against the code base at commit `ad889ea564c74ff66a8d5d7080e6fcd3e62d9684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two distinct root causes. A fixer must close both before submitting.

Cause 1 of 2: the payload-format-2.0 event the HTTP API proxy builds prefixes the stage name to rawPath.
AWS: an HTTP API's $default stage is served on the execute-api endpoint with NO stage segment in the URL, and the payload-2.0 event's `rawPath` is the request path exactly as sent — a GET on <endpoint>/alpha yields rawPath '/alpha' while requestContext.routeKey is 'GET /alpha'. Recorded behavior only: the Lambda event payload is not part of the Smithy model.
Emulator: routeKey is correct but rawPath is '/$default/alpha' — the literal stage name is inserted as a path segment.
Why: any handler that dispatches on rawPath (or rebuilds a URL/signature from it) sees a path no AWS deployment produces, so routing inside the function breaks while the API itself looks healthy.
Fix: the HTTP API data-plane proxy's payload-2.0 event construction (rawPath, and the same stage prefix in requestContext.path) must not carry the stage segment for the $default stage.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes (the Body import, GetRoutes, GetIntegrations and the 200 all pass; only rawPath diverges).

Cause 2 of 2: an UpdateStack that adds a route fails on the stack's AWS::Lambda::Permission resource and rolls the whole stack back.
AWS: the update changes only the route set; the AWS::Lambda::Permission resource's properties (FunctionName from Fn::GetAtt, Action, Principal apigateway.amazonaws.com, SourceArn via Fn::Sub over the unchanged API id) are unchanged, so CloudFormation treats it as a no-op (or replaces it cleanly) and the stack reaches UPDATE_COMPLETE; the AutoDeploy stage then serves the new route with a new DeploymentId.
Emulator: the stack ends UPDATE_ROLLBACK_COMPLETE with StackStatusReason 'The following resource(s) failed to update: [InvokePermission].' — fixed text 'The following resource(s) failed to update: [' + the failed logical ids + '].' Hypothesis: the AWS::Lambda::Permission update handler re-issues AddPermission with the same StatementId rather than no-oping or remove-then-add, so the second apply conflicts.
Why: no stack that grants API Gateway permission to invoke a function can ever be updated — every route or handler change rolls back.
Fix: the AWS::Lambda::Permission update handler in the CloudFormation resource provisioner (no-op on unchanged properties; replace by removing the old statement first).
Traces: apigatewayv2__cloudformation.stage-auto-deploy-republishes-on-a-cfn-route-change (the create leg, the served GET /v1 and the pinned-stage control all pass first).

## Evidence
Cause 1 — expected json body {'routeKey': 'GET /alpha', 'rawPath': '/alpha'}; actual '{"routeKey": "GET /alpha", "rawPath": "/$default/alpha"}'.
Cause 2 — expected UPDATE_COMPLETE after flipping the template's IncludeSecondRoute parameter to 'true'; actual AssertionError: stack <name> update ended in UPDATE_ROLLBACK_COMPLETE: The following resource(s) failed to update: [InvokePermission].

## Steps to Reproduce
Cause 1: CreateStack an AWS::ApiGatewayV2::Api with an inline OpenAPI Body declaring GET /alpha and GET /beta as x-amazon-apigateway-integration AWS_PROXY to a Lambda that echoes event['rawPath'] and requestContext['routeKey']; GET <execute-api endpoint for $default>/alpha and read rawPath. Cause 2: deploy the same kind of stack (HTTP API + AutoDeploy $default stage + AWS::Lambda::Permission for apigateway.amazonaws.com) with one route, then UpdateStack with the identical template plus a second route and poll DescribeStacks/DescribeStackEvents.
