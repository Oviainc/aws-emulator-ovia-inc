Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error|_connect#14|Timeout|time.sleep(3)
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes|_connect#16|Timeout|time.sleep(3)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# WebSocket API data plane never settles: $connect/$disconnect handshakes and stage metric series never materialise, so both stage checks exhaust their budget

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
This issue bundles 2 net-new root causes for the WebSocket (PROTOCOL_TYPE=WEBSOCKET) API data plane. Both must be closed before submitting.

Cause 1 of 2: a WebSocket `$connect` integration that returns statusCode 403 does not produce the refused handshake, and the rejection is not classified into the stage's error series.
AWS: recorded behavior only (the apigatewayv2 Smithy model describes the control plane; the handshake and the metric vocabulary are not modelled). When a `$connect` integration returns statusCode 403, the handshake is refused with `HTTP/1.1 403` carrying the integration's body and NO `Sec-WebSocket-Accept` header, and the refusal counts as one `ClientError` under {ApiId, Stage} in the AWS/ApiGateway namespace, while still counting in `ConnectCount` and `MessageCount`; `IntegrationError` stays an explicit 0. A frame routed to a route whose Lambda raises produces an `Internal server error` frame and counts as one `ExecutionError`.
Emulator: the check never reaches any of those assertions — it exhausts its 60s budget while waiting on the handshake/metric outcome, so neither the 403 handshake nor the ClientError/ExecutionError series is ever observed.
Why: a client validating a WebSocket authorizer/`$connect` gate, or an alarm built on ClientError vs ExecutionError vs IntegrationError, cannot be exercised at all, and the 4XX/5XX classification a customer's alarms depend on is unverifiable.
Fix: the apigatewayv2 WebSocket data-plane endpoint (handshake completion and refusal path) and the AWS/ApiGateway metric publisher for WebSocket stages.
Traces: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error.

Cause 2 of 2: a WebSocket stage publishes no counter series for admitted connections, frames, replies and server pushes.
AWS: recorded behavior only. An admitted connection and a frame on a WebSocket stage increment `ConnectCount` and `MessageCount` (Sum) under {ApiId, Stage}, and the reply/push path is likewise counted; `IntegrationLatency` carries a SampleCount.
Emulator: the check exhausts its 60s budget waiting for the stage's counter series; the awaited datapoints never appear.
Why: WebSocket traffic is invisible to CloudWatch, so stage dashboards and connection-volume alarms stay empty.
Fix: the AWS/ApiGateway metric publisher on the WebSocket data path (ConnectCount, MessageCount, IntegrationLatency under {ApiId, Stage}).
Traces: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes.

The open issue 20261024T120951Z-apigatewayv2-cloudwatch-lambda-authorizer-403-and-websocket-per.md carries the sibling detailed-metrics check's replay key; neither of the two keys above is on it.

## Evidence
Both checks fail with `Error: Check timed out after 60.0s` — no assertion was reached, so no handshake response and no stage datapoint was ever observed.

apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error — the check deploys one function serving $connect/$disconnect/boom and two stages (`guard` refusing a tokenless handshake with statusCode 403, `live` admitting). Expected: HTTP/1.1 403 with the integration body and no Sec-WebSocket-Accept, ClientError=1, ExecutionError=1 for the raising route, IntegrationError=0 explicitly, and clean zeroes on the `live` control. Actual: timeout.

apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes — expected the {ApiId, Stage} series (ConnectCount Sum, MessageCount Sum, IntegrationLatency SampleCount). Actual: timeout.

## Steps to Reproduce
Replay the two check ids in trace_ids. Each creates a WEBSOCKET apigatewayv2 API with a Lambda integration on $connect/$disconnect plus a custom route, opens a raw WebSocket handshake against the stage endpoint, then polls CloudWatch GetMetricStatistics/GetMetricData for the AWS/ApiGateway series under {ApiId, Stage}.
