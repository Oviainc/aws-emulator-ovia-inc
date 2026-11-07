Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error|_connect#14|AssertionError|raise AssertionError(f"stage {stage!r} never upgraded a WebSocket handshake: {seen!r}")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route|_connect#20|AssertionError|raise AssertionError(f"stage {stage!r} never upgraded a WebSocket handshake: {seen!r}")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes|_connect#16|AssertionError|raise AssertionError(f"stage {stage!r} never upgraded a WebSocket handshake: {seen!r}")
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2__cloudwatch__lambda: no WebSocket stage metric can be produced — every stage refuses the RFC 6455 upgrade with 404

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api WebSocket upgrade + GetMetricStatistics (ConnectCount/MessageCount/ClientError/ExecutionError/IntegrationLatency)

## Description
Cause 1 of 1: a deployed WEBSOCKET stage never upgrades a handshake, so the whole WebSocket metric vocabulary is unobservable.
AWS: an upgrade request to a deployed WEBSOCKET stage answers 101 Switching Protocols; the stage then publishes ConnectCount, MessageCount, IntegrationError, ClientError, ExecutionError and IntegrationLatency under {ApiId, Stage} (and per {ApiId, Stage, Route} when DetailedMetricsEnabled), and a $connect integration answering 403 raises ClientError without an ExecutionError. Recorded behavior only for the handshake; Smithy: RouteSettings.DetailedMetricsEnabled ('Specifies whether detailed metrics are enabled').
Emulator: each of the three checks drove 40 handshake attempts against its stages ('live', 'guard', 'detailed', 'duplex') and every single one answered 'HTTP/1.1 404 Not Found' — the execute-api WebSocket path is not served, so no connection, no frame and therefore no metric datum exists. Fixed part of the divergence: the 404 status in place of 101; the stage names and api id are request values.
Why: a client cannot observe any WebSocket traffic metric or alarm on it, and the client-error/execution-error classification (a $connect 403 is a ClientError, not an ExecutionError) can never be verified.
Fix: the execute-api front door's WebSocket upgrade seam plus the data plane's metric emission into cloudwatch for {ApiId, Stage} and per-route series. Related open issues that do not carry these checks' replay keys: 20261029T182452Z-apigatewayv2-cloudwatch-lambda-websocket-api-data-plane-never-settles-connect-disconnect.md, 20261024T120951Z-apigatewayv2-cloudwatch-lambda-authorizer-403-and-websocket-per.md.
Traces: the three checks in trace_ids.

## Evidence
"stage 'live' never upgraded a WebSocket handshake: ['HTTP/1.1 404 Not Found', ... x40]"; "stage 'detailed' never upgraded a WebSocket handshake: ['HTTP/1.1 404 Not Found', ... x40]". Expected the first line of the handshake response to be 'HTTP/1.1 101 Switching Protocols'.

## Steps to Reproduce
Replay the three checks in trace_ids: create a WEBSOCKET API with $connect/$default/$disconnect (plus reply/boom) Lambda integrations, deploy stages live/guard/detailed/duplex (DetailedMetricsEnabled on 'detailed'), then open an RFC 6455 upgrade against each stage and read the metric series from cloudwatch.
