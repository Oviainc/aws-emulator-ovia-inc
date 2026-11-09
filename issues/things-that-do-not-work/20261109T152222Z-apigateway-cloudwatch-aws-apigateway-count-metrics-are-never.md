Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch.apigw-method-metrics-reach-cloudwatch-only-when-detailed-metrics-are-enabled apigateway__cloudwatch.obs-apigateway-3-per-method-metrics-only-for-the-enabled-method
Issue-Key: apigateway__cloudwatch.apigw-method-metrics-reach-cloudwatch-only-when-detailed-metrics-are-enabled|run#36|AssertionError|assert observed == expected_total, (
Issue-Key: apigateway__cloudwatch.obs-apigateway-3-per-method-metrics-only-for-the-enabled-method|run#33|AssertionError|assert 0 < warm_seen <= counter.by_path["/ok"], (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__cloudwatch: AWS/ApiGateway Count metrics are never published for a stage, at either stage or per-method resolution

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: API Gateway publishes no AWS/ApiGateway metrics for requests served through a stage.
AWS: when a stage (or a method, via methodSettings /{resourcePath}/{httpMethod}/metrics/enabled) has metrics enabled, API Gateway emits the AWS/ApiGateway Count metric for every request it serves — at stage resolution under the dimension set (ApiName, Stage), and, with detailed metrics enabled for a method, additionally under (ApiName, Method, Resource, Stage). GetMetricStatistics/GetMetricData then report those datapoints, with Sum/SampleCount equal to the number of requests served. A method whose detailed metrics are NOT enabled must produce no per-method series.
Emulator: after driving a known number of requests through the stage (9 in one check, 97 /ok calls in the other), both the stage-level (ApiName, Stage) Count series and the per-method (ApiName, Method, Resource, Stage) Count series report 0 datapoints for up to 480s — the metric is never produced at all.
Why: a client cannot observe API traffic, cannot build a dashboard or an alarm on Count/4XXError/5XXError/Latency, and — because nothing is produced — cannot even distinguish "metrics correctly withheld for a method without detailed metrics" from "metrics never emitted". The negative half of the contract passes vacuously.
Fix: the execute-api request path's AWS/ApiGateway metric emission (stage-level Count, and the per-method series gated on the method's metrics/enabled setting), feeding the cloudwatch metric store.
Traces: apigateway__cloudwatch.apigw-method-metrics-reach-cloudwatch-only-when-detailed-metrics-are-enabled, apigateway__cloudwatch.obs-apigateway-3-per-method-metrics-only-for-the-enabled-method

## Evidence
"expected AWS/ApiGateway Count on [('ApiName', '<api>'), ('Stage', '<stage>')] to sum (SampleCount) to exactly 9 — the number of HTTP requests this run sent to the stage — got 0 within 480.0s (Sum statistic was 0; datapoints [])".
"expected the per-method Count series on [('ApiName', '<api>'), ('Method', 'GET'), ('Resource', '/ok'), ('Stage', '<stage>')] to appear for the method whose detailed metrics were enabled, carrying at most the 97 /ok call(s) made, got 0 within 480.0s (datapoints [])".
The request counts and dimension values come from the request/state; the requirement is that a Count datapoint exists under those dimensions at all.

## Steps to Reproduce
1. Create + deploy a REST API with a MOCK or Lambda method; UpdateStage with patchOperations enabling /*/*/metrics/enabled (and, for the per-method case, /~1ok/GET/metrics/enabled on one method only).
2. Send a known number of requests to the stage URL.
3. cloudwatch GetMetricStatistics(Namespace='AWS/ApiGateway', MetricName='Count', Dimensions=[{ApiName},{Stage}], Statistics=['Sum','SampleCount']) and again with (ApiName, Method, Resource, Stage).
Expected: datapoints summing to the number of requests sent at stage resolution, and a per-method series only for the method with detailed metrics enabled. Actual: 0 datapoints for both, for 480s.
