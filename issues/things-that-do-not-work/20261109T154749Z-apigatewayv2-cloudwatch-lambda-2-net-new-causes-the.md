Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
Issue-Key: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid|run#15|PublishedUrlRejected|base = ctx.stage_url(http_api["ApiEndpoint"], "live")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error|_connect#15|AssertionError|raise AssertionError(f"stage {stage!r} never upgraded a WebSocket handshake: {seen!r}")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route|_connect#21|AssertionError|raise AssertionError(f"stage {stage!r} never upgraded a WebSocket handshake: {seen!r}")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes|_connect#17|AssertionError|raise AssertionError(f"stage {stage!r} never upgraded a WebSocket handshake: {seen!r}")
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__cloudwatch__lambda: 2 net-new causes — the HTTP API's published ApiEndpoint is an AWS execute-api host, and every WebSocket stage answers the RFC 6455 handshake 500, so no API Gateway metric series is ever produced

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Two net-new root causes; both block all metric observation for this service pair and both must be closed.

Cause 1 of 2: the HTTP API's published ApiEndpoint is an AWS public execute-api hostname the emulator does not serve, so the authorizer-403 request is never made and no 4XX datapoint is recorded (1 check).
AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateApi/GetApi output member ApiEndpoint, documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com" — the address the API is served on. A request refused by a Lambda authorizer is answered 403 and counted in the AWS/ApiGateway 4XXError series under the ApiId (and Stage) dimension.
Emulator: the value published is "https://<api-id>.execute-api.<region>.amazonaws.com"; replay refuses it with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>"). No request, so no metric.
Why: a client given only an unreachable address cannot exercise the API, and its CloudWatch series stays empty — monitoring built against the emulator never sees traffic.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi handlers plus the execute-api front-door listener that must serve it.
Traces: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid.

Cause 2 of 2: a deployed WEBSOCKET stage answers every correct RFC 6455 handshake with HTTP/1.1 500 Internal Server Error, so no connection is ever established and ConnectCount/MessageCount/route-split series cannot exist (3 checks).
AWS: recorded behavior only. A correct handshake against a deployed WEBSOCKET stage is answered 101 Switching Protocols (or 403/401 when $connect refuses); AWS never answers a well-formed handshake 500. Connect/message/reply/push counts are then published per stage, and with detailed metrics enabled split per route.
Emulator: every attempt returns 'HTTP/1.1 500 Internal Server Error' (content-type application/json, body {"message":"Internal server error"}), repeated until the check's deadline — e.g. "stage 'live' never upgraded a WebSocket handshake: ['HTTP/1.1 500 Internal Server Error', ...]". No connection, no frames, no metrics.
Why: WebSocket clients cannot connect at all, and a 500 invites retries for a request AWS would have upgraded; the stage's entire metric surface is unobservable.
Fix: the WEBSOCKET execute-api front door (the upgrade seam that must hand the connection to the api's socket after running $connect) and the $connect integration invocation in the apigatewayv2 data-plane handler; the stage metric emission path is only reachable once a connection upgrades.
Related but not covering: 20261107T193414Z-apigatewayv2-cloudwatch-lambda-a-deployed-websocket-stage-answers.md and 20261107T193731Z-apigatewayv2-cloudwatch-lambda-no-websocket-stage-metric-can.md describe handshakes answered 404 for this service pair and own other replay keys; the status observed now is 500.
Traces: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error, apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route, apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes.

## Evidence
Cause 1 — expected: a 403 from the Lambda authorizer is counted in AWS/ApiGateway 4XXError under the ApiId dimension. Actual: simparity.published.PublishedUrlRejected at context.py:572 — "endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com ... its host is not one of the emulator's own hosts"; no request made.
Cause 2 — expected: 'HTTP/1.1 101 Switching Protocols' for a correct handshake on the deployed stage, then ConnectCount/MessageCount datapoints. Actual: AssertionError "stage 'live' never upgraded a WebSocket handshake: ['HTTP/1.1 500 Internal Server Error', 'HTTP/1.1 500 Internal Server Error', ...]" (same for stage 'detailed').

## Steps to Reproduce
1. CreateApi(ProtocolType='HTTP') with a Lambda authorizer that denies, read ApiEndpoint and request the route — the published host is not served. Replay apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid.
2. CreateApi(ProtocolType='WEBSOCKET') with $connect/$default Lambda integrations and an AutoDeploy stage; open a WebSocket handshake to the stage — it is answered 500. Replay the three obs-apigatewayv2-1-websocket checks.
