Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle
Issue-Key: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2__dynamodb__lambda__logs__s3: the WebSocket upgrade to a deployed prod stage answers 404 UnknownService (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api WebSocket upgrade)

## Description
Cause 1 of 1: the emulator's front door does not recognise a deployed WEBSOCKET stage's URL as any service, so the RFC 6455 upgrade is refused with HTTP 404.
AWS: recorded behavior only (data plane). A WEBSOCKET API with a deployed stage completes the upgrade with HTTP 101 Switching Protocols on its wss:// stage URL; the backend Lambda then receives $connect/$default events and its replies are delivered back over the socket.
Emulator: the upgrade is answered `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1` and `x-amzn-errortype: UnknownService` — an unrouted-request answer from the JSON control-plane router, not an API Gateway data-plane response. The check aborts at its pre_invoke phase (phase 3 of 6), so the backend-response lifecycle it then asserts over DynamoDB, CloudWatch Logs and S3 never starts.
Why: a client's WebSocket application cannot connect at all, and the 404 it gets back is shaped like a control-plane protocol error rather than anything a WebSocket client can interpret.
Fix: the execute-api WebSocket listener and the router entry that maps a deployed WEBSOCKET stage's announced host/path to it (the same root absence reported for service apigatewayv2 in this sweep). Related open issues 20261023T124140Z-apigatewayv2-websocket-duplex-data-plane-is-not-served.md and 20261023T223918Z-apigatewaymanagementapi-is-untestable-end-to-end-the-websocket.md describe the same surface but do not carry this check's replay key.
Traces: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle.

## Evidence
Expected: HTTP 101 Switching Protocols on the deployed prod WEBSOCKET stage. Actual: `AssertionError: [phase 3/6 pre_invoke] the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149'`.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType=WEBSOCKET) with $connect/$disconnect/$default over an AWS_PROXY Lambda integration, CreateDeployment, CreateStage(StageName='prod').
3. Send an HTTP/1.1 GET with `Upgrade: websocket` / `Connection: Upgrade` / `Sec-WebSocket-Key` to the announced stage URL — it answers 404 with x-amzn-errortype: UnknownService instead of 101.
4. Replay apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle.
