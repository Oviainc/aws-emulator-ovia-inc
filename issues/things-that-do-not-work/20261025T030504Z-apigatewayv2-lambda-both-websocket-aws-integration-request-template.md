Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# apigatewayv2__lambda: both WebSocket AWS-integration request-template checks hang to the 60 s cap — the deployed WEBSOCKET stage never serves (1 cause)

> _Note: this issue was identified against the code base at commit `5d61882ef55489b75c810fe19d4c281a0249999c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
WebSocket upgrade (execute-api data plane)

## Description
One cause, grounded in the two failing apigatewayv2__lambda checks the replay report marked neither ALREADY FILED nor OWNED. Both services involved (apigatewayv2 and lambda) are bootstrapped.

=== Cause 1 of 1: a WEBSOCKET AWS (non-proxy) Lambda integration's request template never reaches the function, because the deployed stage serves no WebSocket traffic ===
AWS: recorded behavior only (execute-api data plane; not in the Smithy model). On a deployed WEBSOCKET stage whose route targets an AWS (non-proxy) Lambda integration, API Gateway renders the integration's RequestTemplates against the client's frame and the connection context and invokes the function with the RENDERED document — so an echo function returns the template's output ($input.json('$.payload'), $context.routeKey, $context.connectionId, $context.stage), not the raw frame. With PassthroughBehavior=NEVER and a TemplateSelectionExpression that selects a key with no template, the message is refused on the socket with an internal-server-error frame carrying the connection's own connectionId and nothing the function would have returned; switching only PassthroughBehavior to WHEN_NO_MATCH makes the identical frame reach the function verbatim.
Emulator: neither check gets that far. Both open a WebSocket upgrade against the stage's announced wss:// URL and poll for a live socket that never comes up, so each runs past the suite's 60 s cap and is reported as `Check timed out after 60.0s`. The same emulator answers a WEBSOCKET upgrade with `HTTP/1.1 404 Not Found` and `x-amzn-errortype: UnknownService` (observed directly by apigatewayv2.websocket-connect-mock-integration-rejected in this same replay), which is the behaviour these two checks are polling against.
Why: AWS (non-proxy) integrations are the only way to reshape a WebSocket frame before Lambda sees it, and PassthroughBehavior is the only way to refuse an unmatched content type. With no data plane, a client cannot tell a correct template from a broken one, and a mapping-template regression is invisible until it reaches AWS.
Fix: the execute-api WebSocket listener and the router entry for a WEBSOCKET stage's announced host/path, then AWS (non-proxy) integration handling on that path: TemplateSelectionExpression resolution, RequestTemplates rendering with the $context/$input bindings, PassthroughBehavior=NEVER refusal as an error frame on the socket, and the rendered payload being what Lambda is invoked with. Related open issue 20261023T223918Z-apigatewayv2-lambda-all-three-websocket-authorizer-routing-checks.md covers a different set of apigatewayv2__lambda WebSocket checks and does not carry these two checks' replay keys.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda, apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against this checkout: 18/194 passed; apigatewayv2__lambda 0/6.

apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda [mismatch]: Error: Check timed out after 60.0s.
apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-never-passthrough-unmatched-template-rejected [mismatch]: Error: Check timed out after 60.0s.

Expected: HTTP 101 Switching Protocols on the deployed WEBSOCKET stage's wss:// URL, then (a) the echo function returning the RENDERED template document, and (b) with PassthroughBehavior=NEVER and an unmatched TemplateSelectionExpression, an internal-server-error frame naming this connection's connectionId instead of any function output.
Actual: no handshake ever completes, so both checks' poll loops run until the suite's 60 s cap kills them. The directly-observed answer to a WEBSOCKET upgrade on this emulator in the same replay is HTTP 404 with x-amzn-errortype: UnknownService.

Neither check carried an ALREADY FILED or OWNED-by marker in the replay report.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a Lambda function that echoes its event ({'received': event}).
3. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); CreateIntegration(IntegrationType='AWS', IntegrationUri=<function arn>, TemplateSelectionExpression='\\$default', RequestTemplates={'\\$default': '{"marker":"...","echo":$input.json(\'$.payload\'),"routeKey":"$context.routeKey","connectionId":"$context.connectionId","stage":"$context.stage"}'}); CreateRoute + CreateRouteResponse + CreateIntegrationResponse; CreateStage; CreateDeployment.
4. Open an RFC 6455 upgrade to the announced wss:// URL and send a frame. Expect HTTP 101 then the function echoing the RENDERED template; observe no handshake (HTTP 404, x-amzn-errortype: UnknownService) and the check hanging to the cap.
5. Set PassthroughBehavior='NEVER' with TemplateSelectionExpression='${request.body.kind}' selecting a key with no template and send the frame: expect an internal-server-error frame carrying this connectionId; then switch to WHEN_NO_MATCH and expect the raw frame to reach the function.
6. Re-replay the two check ids in trace_ids.
