Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx
Issue-Key: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx|_http#11|Timeout|with urllib.request.urlopen(req, timeout=timeout) as resp:
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__cloudwatch__lambda: the authorizer-denied 403 4xx-metric check never completes — it hangs to the 60 s harness cap

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the metric flow that drives a request at a deployed REST API whose Lambda authorizer denies, then reads the stage's CloudWatch metrics, never returns — the check is killed at the harness's 60 s budget with no response at all.
AWS: recorded behavior only. Real AWS answers the authorizer-denied request with HTTP 403 within normal request latency and publishes that request to the AWS/ApiGateway namespace as a 4XXError datapoint (not 5XXError) under the ApiName/Stage dimensions, so the subsequent GetMetricStatistics call returns promptly.
Emulator: neither leg completes. The check produces no HTTP status and no error code — it is terminated by the harness after 60.0 s ('Error: Check timed out after 60.0s'), meaning the emulator accepted the request and never produced a response.
Why: a hang is worse than a wrong answer for any client — SDK calls do not fail fast, they block until the client's own socket timeout, and no retry or error-handling path is exercised. The 4xx-vs-5xx metric classification that this check exists to verify cannot be observed at all.
Fix: the request path that serves a deployed stage and the metric emission hooked to it must terminate and respond; the hang belongs in the stage-invoke request handler and the CloudWatch metric read it feeds. Note the related open issue 20261023T125454Z-apigateway-traffic-driven-at-a-deployed-rest-api-and.md covers 41 other hanging traffic checks but does not carry this check's replay key.
Traces: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx.

## Evidence
Expected: the denied request answers HTTP 403 promptly and a 4XXError datapoint (not 5XXError) is readable for the ApiName/Stage dimensions.
Actual: Error: Check timed out after 60.0s (report line 47398). No HTTP status, no error code, no response body — the call never returned.

## Steps to Reproduce
1. Create a REST API with a Lambda TOKEN authorizer that returns Deny, deploy it to a stage.
2. Send a request to the stage invoke URL with a token the authorizer denies.
3. Expect HTTP 403 within normal latency, then read CloudWatch AWS/ApiGateway 4XXError for that ApiName/Stage.
4. Observe that the flow never completes within 60 s.
Re-replay apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx.
