Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error|_recv_exact#17|AssertionError|raise AssertionError(f"WebSocket closed after {len(buf)} of {n} expected bytes")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route|_recv_exact#21|AssertionError|raise AssertionError(f"WebSocket closed after {len(buf)} of {n} expected bytes")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes|_recv_exact#19|AssertionError|raise AssertionError(f"WebSocket closed after {len(buf)} of {n} expected bytes")
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__cloudwatch__lambda: an upgraded WebSocket connection is closed before the first frame header, so no connect/message/reply stage metric is ever produced

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
Cause 1 of 1: the emulator closes an upgraded WEBSOCKET connection before any frame is exchanged, so the AWS/ApiGateway WebSocket stage series can never be driven.
  AWS: recorded behavior only — the execute-api WebSocket data plane is not modeled in Smithy. AWS holds the connection open after 101 Switching Protocols, dispatches $connect/$default/named routes, and publishes ConnectCount, MessageCount, IntegrationError/ClientError and (with detailed metrics enabled) per-route series under the ApiId/Stage dimensions; a $connect integration answering 403 is counted as a client error with a route-level execution error.
  Emulator: the handshake completes, then the socket is closed with zero bytes written — every check fails reading the 2-byte frame header with 'WebSocket closed after 0 of 2 expected bytes'. No frame is dispatched, so no metric datum exists to assert on.
  Why: a client that monitors a WebSocket API sees an empty metric namespace and cannot distinguish a healthy stage from a broken one; alarms on ConnectCount/MessageCount/4xx never fire, and the 403-on-$connect classification (client error + route execution error) is unobservable.
  Fix: the apigatewayv2 WEBSOCKET execute-api data plane (the post-upgrade frame loop that must own the socket and dispatch routes) and the stage metric emission that hangs off it. Related open issues 20261107T193731Z-apigatewayv2-cloudwatch-lambda-no-websocket-stage-metric-can.md and 20261109T154749Z-apigatewayv2-cloudwatch-lambda-2-net-new-causes-the.md describe the earlier 404-on-handshake shape and do not carry these checks' replay keys.
  Traces: the three checks in trace_ids.

## Evidence
expected: after 101 the connection carries $connect/frame/reply traffic and AWS/ApiGateway publishes ConnectCount/MessageCount (and per-route series with detailed metrics) under ApiId+Stage.
actual, all three checks: AssertionError: WebSocket closed after 0 of 2 expected bytes (_recv_exact while reading the frame header) — the socket is closed immediately after the upgrade, with no CLOSE frame.

## Steps to Reproduce
1. Create a WEBSOCKET API with $connect/$default Lambda integrations, a stage and a deployment; enable detailed metrics for the per-route case.
2. Complete an RFC 6455 upgrade and attempt to read any frame — the peer closes after 0 bytes.
3. Call cloudwatch GetMetricStatistics/GetMetricData for AWS/ApiGateway ConnectCount/MessageCount with ApiId+Stage dimensions: no datapoints.
4. Replay the checks in trace_ids.
