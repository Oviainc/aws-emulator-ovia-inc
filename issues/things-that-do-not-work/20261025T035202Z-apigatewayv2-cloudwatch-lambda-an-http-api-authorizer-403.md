Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid
Issue-Key: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid|_http#15|Timeout|with urllib.request.urlopen(req, timeout=timeout) as resp:
Issue-Commit: bb4602e7ae983d0b86fd9b19deccaf384a5ade46

# apigatewayv2__cloudwatch__lambda: an HTTP API authorizer 403 produces no traffic and no metric — the authorizer-forbidden 4XX-under-ApiId metric check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `bb4602e7ae983d0b86fd9b19deccaf384a5ade46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: a deployed HTTP API stage with a Lambda authorizer never answers an invoke, so the authorizer's 403 is never produced and CloudWatch never records the lowercase `4xx` datapoint that AWS/ApiGateway publishes under the ApiId dimension for HTTP APIs. The check cannot reach any assertion and is cut off at the harness 60 s cap.

  AWS: recorded behavior only for the data-plane leg (the execute-api invoke path is not in the apigatewayv2 Smithy model; the control plane used here — CreateApi(ProtocolType='HTTP'), CreateAuthorizer, CreateRoute, CreateStage with DetailedMetricsEnabled — is). The recording shows a request that an authorizer denies returning HTTP 403 to the caller, and CloudWatch `GetMetricStatistics` in namespace AWS/ApiGateway then reporting that request under the HTTP-API metric name `4xx` (lowercase, which is the HTTP-API spelling, as distinct from the REST-API `4XXError`) dimensioned by ApiId (and by Stage when detailed metrics are on).
  Emulator: no part of the exchange happens. The replay ends in `Check timed out after 60.0s`: the deployed HTTP API stage never answers the invoke, so there is no 403 to count and the metric poll never finds a datapoint.
  Why: two contracts are unobservable at once — an HTTP API authorizer cannot deny a request (the 403 the caller relies on is never served), and API Gateway's HTTP-API metric vocabulary (lowercase `4xx` keyed on ApiId) is never exercised, so a customer dashboard or alarm built on it stays empty while the control plane reports the stage as deployed with detailed metrics enabled.
  Fix: the apigatewayv2 execute-api HTTP-API data plane (serving the deployed stage and running its authorizer to a 403), and the AWS/ApiGateway metric emission that must follow it into the CloudWatch metric store.
  Traces: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid

Relation to an already-open issue: 20261024T120951Z-apigatewayv2-cloudwatch-lambda-authorizer-403-and-websocket-per.md describes this surface but carries only the sibling check's replay key (the WebSocket per-route metrics check), not this one — so it does not close this check. Re-replay the key in trace_ids before closing either issue.

## Evidence
Behavioral replay:

  [mismatch] apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid
  Error: Check timed out after 60.0s

Expected on AWS: the invoke of the deployed HTTP API stage is denied by the authorizer with HTTP 403, and CloudWatch GetMetricStatistics(Namespace='AWS/ApiGateway', MetricName='4xx', Dimensions=[{ApiId}]) returns a datapoint covering it.
Actual: the stage never answers the invoke, so no 403 is served and no datapoint ever appears; the check is terminated at the cap with no assertion reached.
The 60 s figure is the harness cap, not a requirement; the divergence is that neither the 403 nor the metric is ever produced.

## Steps to Reproduce
1. Boot the emulator.
2. CreateApi(ProtocolType='HTTP'); create the backing Lambda and a Lambda authorizer via CreateAuthorizer; CreateRoute bound to the authorizer; CreateStage with DetailedMetricsEnabled=true and deploy.
3. Issue an HTTPS request to the deployed stage's invoke URL with credentials/headers the authorizer denies.
   Expected: HTTP 403 returned to the caller.
   Observed: no response; the check hangs.
4. Poll CloudWatch GetMetricStatistics(Namespace='AWS/ApiGateway', MetricName='4xx', Dimensions=[{'Name':'ApiId','Value':<api-id>}], Statistics=['Sum']).
   Expected: a datapoint with Sum >= 1.
   Observed: never reached — the check is cut off at the 60 s cap.
