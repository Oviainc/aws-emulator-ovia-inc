Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route
Issue-Key: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid|_ready_then_align#15|Timeout|time.sleep(0.5)
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route|_connect#20|Timeout|time.sleep(3)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2__cloudwatch__lambda: authorizer-403 and WebSocket per-route metric checks cannot run — the execute-api data plane never serves (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane) / GetMetricStatistics

## Description
Cause 1 of 1: neither an HTTP API invoke nor a WEBSOCKET stage ever serves, so the metric series that only request traffic can create are never produced.
AWS: recorded behavior only (data plane + metric publication). A request an HTTP API authorizer refuses is answered 403 and counted in the lowercase `4xx` series of AWS/ApiGatewayV2 under the ApiId dimension; a WEBSOCKET stage with DetailedMetricsEnabled splits its stage totals into per-route series.
Emulator: both checks poll their deployed endpoint and never get a servable answer, so each runs past the suite's 60 s cap and is reported as `Check timed out after 60.0s`. With no served request there is no 403 to count and no per-route WebSocket datum to split.
Why: a client validating authorizer-rejection alarms or per-route WebSocket dashboards sees nothing at all, and cannot tell a missing metric pipeline from a quiet API.
Fix: the execute-api data-plane listeners for HTTP and WEBSOCKET APIs (the same root absence reported for service apigatewayv2 in this sweep) and the AWS/ApiGatewayV2 metric publication behind them. Related open issue 20261023T151412Z-apigatewayv2-cloudwatch-lambda-the-authorizer-403-4xx-metric.md attributes an authorizer-403 metric failure to a CreateAuthorizer NotImplemented stub; these two checks did not fail that way and are not carried by it.
Traces: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid, apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route.

## Evidence
Expected: a 403 from an authorizer-refused HTTP API request appears in the AWS/ApiGatewayV2 `4xx` series under ApiId, and a WEBSOCKET stage with detailed metrics yields per-route series. Actual: `Error: Check timed out after 60.0s` for both checks — the endpoint never served the traffic the metric assertions depend on.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Build an HTTP API with a Lambda authorizer that denies, deploy a stage, and GET the announced invoke URL — no 403 is ever served.
3. Build a WEBSOCKET API with DetailedMetricsEnabled route settings and deploy a stage — the upgrade never completes.
4. Query AWS/ApiGatewayV2 for the ApiId dimension — no datapoints.
5. Replay apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid and apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route.
