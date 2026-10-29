Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url
Issue-Key: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url|_ws_connect#4|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Commit: c96a98ba4351b01bcc3b10297b18cb90664e8dd6

# apigatewayv2: the RFC 6455 upgrade to a deployed WEBSOCKET prod stage answers 404 UnknownService instead of 101 Switching Protocols (1 cause)

> _Note: this issue was identified against the code base at commit `c96a98ba4351b01bcc3b10297b18cb90664e8dd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api WebSocket upgrade on a deployed WEBSOCKET stage)

## Description
Cause 1 of 1: the emulator's front door does not route a deployed API Gateway v2 WEBSOCKET stage's announced URL to any execute-api data plane, so the WebSocket handshake is refused by the generic JSON control-plane router.

AWS: recorded behavior only. The execute-api WebSocket data plane is not a Smithy-modeled operation — the handshake is a plain HTTP/1.1 GET carrying `Upgrade: websocket`, `Connection: Upgrade`, `Sec-WebSocket-Key` and `Sec-WebSocket-Version: 13` against the deployed stage URL. On real AWS, once CreateApi(ProtocolType=WEBSOCKET) + CreateRoute($connect/$disconnect/$default over an AWS_PROXY Lambda integration) + CreateDeployment + CreateStage(StageName=<stage>) have been made, that GET is answered `HTTP/1.1 101 Switching Protocols` with a `Sec-WebSocket-Accept` header equal to base64(sha1(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")), and the connection then carries $connect/$default route events to the backing Lambda; the handler's reply travels back over the same socket via the modeled apigatewaymanagementapi PostToConnection callback. Transient refusals during stage/permission propagation are 5xx, never 404.

Emulator: the handshake is answered `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1` and `x-amzn-errortype: UnknownService` (content-length 149). That is the unrouted-request answer of the AWS-JSON control-plane router: the request never reached an execute-api listener at all. The fixed text of the divergence is the status `404 Not Found` and the error type `UnknownService`; nothing in the response is request- or state-derived, which is itself the tell — AWS's answer here is a 101 handshake, not a JSON protocol error. Because the socket never opens, everything the check then grades (the registration frame, the `registration_success` reply, and the row the handler writes to the connections table) is unreachable.

Why: a client's WebSocket application cannot connect to the emulated stage at all. Worse, the refusal is a non-5xx, so a client (and this check's bounded retry loop) correctly treats it as permanent rather than as propagation lag, and the 404/`UnknownService` body is not a shape any WebSocket or API Gateway client can interpret — it looks like the service name was wrong rather than like the stage was unreachable.

Fix: the execute-api WebSocket listener for apigatewayv2 and the router entry that maps a deployed WEBSOCKET stage's announced host/path (the same host/path the emulator hands back as the stage's wss:// and https:// callback URL) onto it, plus the handshake response itself (101 + Sec-WebSocket-Accept) and the $connect/$default route dispatch into the AWS_PROXY integration.

Already-open issues describing this same surface do NOT carry this check's replay key, so this check is not covered by them: 20261024T120951Z-apigatewayv2-dynamodb-lambda-logs-s3-the-websocket-upgrade.md (same 404 UnknownService, grounded in apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle), 20261023T124140Z-apigatewayv2-websocket-duplex-data-plane-is-not-served.md and 20261023T223918Z-apigatewaymanagementapi-is-untestable-end-to-end-the-websocket.md.

Traces: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url.

## Evidence
Expected: `HTTP/1.1 101 Switching Protocols` with a matching `Sec-WebSocket-Accept` on the deployed WEBSOCKET stage URL (any transient pre-propagation refusal being 5xx).
Actual: `AssertionError: the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\ndate: ...'` raised from `_ws_connect` (line 509) via `run` (line 34) of check_apigatewayv2__lambda__logs_registration_reply_is_delivered_via_the_https_callback_url.py. The assertion is `status >= 500 and time.monotonic() < deadline`, i.e. the 404 is rejected as a non-retryable, non-propagation answer. The `date` header value is per-run and not part of the requirement.
The check had already confirmed the control plane agreed: get_function_configuration reported WEBSOCKET_ENDPOINT equal to the stage's https:// callback URL the emulator itself announced, so the URL being dialled is the emulator's own.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create three PAY_PER_REQUEST DynamoDB tables (connections/nodes/deployments), an IAM execution role, and a nodejs22.x Lambda function whose handler replies over the WebSocket callback; set its WEBSOCKET_ENDPOINT env var to the stage's https:// callback URL.
3. apigatewayv2 CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<lambda invocations arn>); CreateRoute for $connect, $default and $disconnect targeting that integration; CreateDeployment; CreateStage(StageName='prod'); lambda AddPermission for apigateway.amazonaws.com.
4. Send an HTTP/1.1 GET with `Upgrade: websocket`, `Connection: Upgrade`, `Sec-WebSocket-Key: <b64 16 bytes>`, `Sec-WebSocket-Version: 13` to the announced wss:// stage URL. It answers `404 Not Found` with `x-amzn-errortype: UnknownService` instead of `101 Switching Protocols`.
5. Replay apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url.
