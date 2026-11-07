Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes|_connect#16|AssertionError|raise AssertionError(f"stage {stage!r} never upgraded a WebSocket handshake: {seen!r}")
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2__cloudwatch__lambda: a deployed WEBSOCKET stage answers every handshake 404, so the stage's ConnectCount/MessageCount CloudWatch metrics are never produced (1 cause)

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
ONE net-new root cause on the apigatewayv2__cloudwatch__lambda service token, from the single failure the replay marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: the WEBSOCKET stage never upgrades a handshake (40 consecutive HTTP 404 answers), so no connect, frame, reply or @connections push is ever counted into the stage's CloudWatch metrics ===

AWS: recorded behavior only. A deployed WEBSOCKET stage answers a correct RFC 6455 handshake with `HTTP/1.1 101 Switching Protocols` plus `Upgrade: websocket`, `Connection: Upgrade` and `Sec-WebSocket-Accept: <base64(sha1(Sec-WebSocket-Key + 258EAFA5-E914-47DA-95CA-C5AB0DC85B11))>`, invokes the `$connect` route's integration, and emits the stage's WebSocket metrics into the AWS/ApiGateway namespace dimensioned by ApiId/Stage (connection counts, message counts, integration latency), which CloudWatch GetMetricData/GetMetricStatistics then reports. The check's subject is that each observable event (a connect, a client frame, a route reply, an @connections push) increments the corresponding stage metric.

Emulator: the handshake is refused. The check retries the upgrade 40 times and records `HTTP/1.1 404 Not Found` every single time — the same front-door symptom the apigatewayv2 checks report with `x-amzn-errortype: UnknownService` — so the socket never opens, the `$connect` Lambda never runs, and no stage metric datapoint is ever published for CloudWatch to read. The metric assertions are never reached.

Why: an operator validating a dashboard or an alarm over a WebSocket stage's connect/message counts sees an empty metric on the emulator and a populated one on AWS, so the alarm cannot be exercised; and the 404 tells the client the stage does not exist rather than that the upgrade is unsupported.

Fix: the emulator's front-door HTTP router for the execute-api data plane must complete the WebSocket upgrade and hand the connection to the apigatewayv2 WEBSOCKET data plane (see the apigatewayv2 issue filed in this sweep); that data plane must then publish the stage's WebSocket metrics to the CloudWatch metric store under AWS/ApiGateway with ApiId/Stage dimensions as each connect, frame, reply and @connections push occurs.

Related: the identical missing upgrade is filed in this sweep against the apigatewayv2 token; this issue additionally requires the metric emission that is gated behind it.

Traces: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes.

## Evidence
Suite: simparity-behavioral-aws-v1. Totals: 0/47 passed (0.0%); apigatewayv2__cloudwatch__lambda 0/1 (0%), 1 failed, marked neither ALREADY FILED nor OWNED.

[mismatch] apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes — AssertionError: stage 'live' never upgraded a WebSocket handshake: ['HTTP/1.1 404 Not Found', 'HTTP/1.1 404 Not Found', ... 40 identical entries ...]

Expected (real AWS): the first handshake answers 'HTTP/1.1 101 Switching Protocols' with Upgrade: websocket, Connection: Upgrade and a correct Sec-WebSocket-Accept; the $connect integration runs; and the stage's WebSocket metrics in AWS/ApiGateway (dimensioned ApiId + Stage) gain datapoints for the connect, the client frame, the route reply and the @connections push, readable via CloudWatch.
Actual (emulator): 40 consecutive 'HTTP/1.1 404 Not Found' answers to the upgrade, no 101 ever, hence no $connect invocation and no metric datapoints. The stage name 'live' and the retry count are the check's own values; the requirement is that a deployed WEBSOCKET stage upgrades the handshake and counts the events.

The sibling apigatewayv2 checks in the same run show the full head of that 404: content-type: application/x-amz-json-1.1, x-amzn-errortype: UnknownService — the upgrade is being dispatched as an unknown AWS JSON service call rather than as an execute-api data-plane request.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes from simparity-behavioral-aws-v1.
2. CreateApi ProtocolType=WEBSOCKET with $connect/$default routes backed by a Lambda integration; CreateDeployment; CreateStage (the check uses stage 'live').
3. Send a correct RFC 6455 handshake to the stage's execute-api invoke URL. Observe 'HTTP/1.1 404 Not Found' on every attempt instead of 'HTTP/1.1 101 Switching Protocols'.
4. Query CloudWatch for the stage's WebSocket metrics in AWS/ApiGateway with ApiId/Stage dimensions: no datapoints exist, because no connect, frame, reply or @connections push ever happened.
5. After fixing, re-replay that check id: the handshake must answer 101, the $connect Lambda must run, and the stage's connect/message/reply/push counts must appear as CloudWatch datapoints.
