Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle|_poll#6|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2__dynamodb__lambda: WebSocket route-selection and serverless-lifecycle checks hang — the deployed stage never serves (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane), WebSocket route selection

## Description
Cause 1 of 1: a deployed apigatewayv2 stage never serves traffic, so neither WebSocket route dispatch nor the HTTP serverless lifecycle can be exercised.
AWS: recorded behavior only (data plane). On a WEBSOCKET API whose RouteSelectionExpression is $request.body.action, a frame whose body names a declared action is dispatched to that route's integration and anything else falls to $default; changing the selection expression redirects subsequent dispatch. On an HTTP API, a request served by an AWS_PROXY Lambda integration runs the function, which persists the caller's IP to DynamoDB and returns it.
Emulator: each of these checks drives the deployed endpoint and never gets a servable answer, so all three run past the suite's 60 s cap and are reported as `Check timed out after 60.0s`. No frame is dispatched and no end-to-end write is observable.
Why: the dispatch rule a client encodes in RouteSelectionExpression, and any serverless flow that depends on a served request, are silently untested: the emulator accepts the configuration and then delivers nothing.
Fix: the execute-api data-plane listeners for WEBSOCKET and HTTP APIs — handshake, frame routing by selection expression, and integration dispatch (the same root absence reported for service apigatewayv2 in this sweep). Related open issue 20261023T151412Z-apigatewayv2-dynamodb-lambda-websocket-request-authorizer-and-serverless.md attributes a similar family of hangs to CreateAuthorizer/CreateStage NotImplemented stubs; these three checks are not carried by it.
Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default, apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch, apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle.

## Evidence
Expected: a WebSocket frame `{"action":"<declared>"}` reaches that action's route integration and an undeclared action reaches $default; after UpdateApi changes the selection expression, dispatch follows the new expression; the HTTP visitor-IP flow answers with the IP it just wrote to DynamoDB. Actual: `Error: Check timed out after 60.0s` for all three checks — the deployed endpoint never served.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression='$request.body.action'), declare $connect/$default/<action> routes over an AWS_PROXY Lambda integration, CreateDeployment + CreateStage.
3. Open a WebSocket to the announced stage URL and send a frame naming the action — the connection never serves the dispatch.
4. Separately build the HTTP visitor-IP API (Lambda + DynamoDB) and GET its invoke URL — it never answers.
5. Replay the three check ids in trace_ids.
