Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric
Issue-Key: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions|_poll_until#31|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx|_poll_until#28|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series|_poll_until#22|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns|_poll_until#28|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked|_poll_until#28|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric|run#16|AssertionError|assert first[0] == 500, (
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigatewayv2 HTTP APIs publish no AWS/ApiGateway metrics, and a route with no lambda:InvokeFunction grant is served 200 instead of 500 (2 causes)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two net-new root causes behind 6 failing behavioral checks in this service heading. A fixer must close BOTH before submitting.

Cause 1 of 2: no AWS/ApiGateway metric series is ever published for an HTTP API stage.
AWS: an HTTP API stage meters its traffic into the AWS/ApiGateway namespace dimensioned on exactly {ApiId, Stage}. Count carries one datapoint per request the stage served, 4xx/5xx carry one per response in that class (lower-case metric names '4xx'/'5xx' for HTTP APIs). With DefaultRouteSettings/RouteSettings.DetailedMetricsEnabled=true (Smithy: apigatewayv2 RouteSettings$DetailedMetricsEnabled, jsonName detailedMetricsEnabled) the same traffic additionally appears under {ApiId, Stage, Method, Resource}. Metric emission itself is not modelled, so this is recorded/documented behavior only: the recordings show non-zero Sum for Count/4xx/5xx after the requests were served, and alarms over those series transition to ALARM.
Emulator: every GetMetricStatistics/GetMetricData poll over those series returns 0.0 (no datapoints) for up to 240s after the requests were demonstrably served; an alarm over AWS/ApiGateway 5xx stays StateValue 'OK' with StateReason "Threshold Crossed: no datapoints were received for 1 period and 1 missing datapoint was treated as [NonBreaching]." The stage serves the traffic (the checks observe the 200s, the 429s and the 500s on the wire) but nothing is metered — neither the stage-level nor the detailed per-route series.
Why: a client that watches API health through AWS/ApiGateway sees a permanently silent API — dashboards read zero traffic, 4xx/5xx alarms never leave OK/INSUFFICIENT_DATA, and no notification pipeline downstream of such an alarm ever fires. Every autoscaling or paging decision wired to HTTP API metrics is wrong.
Fix: the apigatewayv2 data-plane request path (services/service-com-amazonaws-apigatewayv2-apigatewayv2) must publish Count/4xx/5xx/Latency/IntegrationLatency datapoints into the cloudwatch metric store (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) under {ApiId, Stage}, plus the {ApiId, Stage, Method, Resource} set when DetailedMetricsEnabled is set on the route/default route settings.
Traces: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions (Count Sum stayed 0.0 against 8 matched-route requests), apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx (4xx stayed 0.0 against 5 observed 429s), apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series (Count stayed 0.0 against 7 served 200s), apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns, apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked.

Cause 2 of 2: an HTTP API Lambda integration with no lambda:InvokeFunction resource policy grant is served a 200.
AWS: an HTTP API route whose Lambda integration target has no resource-based policy statement permitting apigateway.amazonaws.com to lambda:InvokeFunction cannot invoke the function; the gateway answers HTTP 500 with body {"message":"Internal Server Error"} and counts the response in the stage's 5xx series. Recorded behavior only (the permission check is not in the API model); the recording shows the 500.
Emulator: the same request is answered HTTP 200 carrying the integration's own body (the recorded run returned the function's payload 'sim2real-xsvc-perm-<id>' — that value is the run's generated function response, the requirement is only that no 2xx may be served when the grant is absent).
Why: a client cannot observe or test the single most common HTTP API misconfiguration; a missing invoke grant silently looks like a healthy API, and the 5xx series and its alarms stay empty.
Fix: the integration-invoke path of the apigatewayv2 data plane (services/service-com-amazonaws-apigatewayv2-apigatewayv2) must consult the target function's resource policy before invoking and refuse with 500 Internal Server Error when no statement grants lambda:InvokeFunction to the API.
Traces: apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric.

## Evidence
Cause 1 — expected: AWS/ApiGateway Count Sum for (ApiId=<api>, Stage=<stage>) >= the number of requests served (8 in the recorded run); actual: AssertionError '... reaching at least the 8 matched-route requests this check deliberately made not met within 240.0s. Last result: 0.0'. Same shape for 4xx ('reaching the 5 observed 429s ... Last result: 0.0') and for the detailed-metrics opt-in ('reaching the 7 200s served while detailed metrics were off ... Last result: 0.0'). Alarm path: DescribeAlarms keeps StateValue='OK', StateReason='Threshold Crossed: no datapoints were received for 1 period and 1 missing datapoint was treated as [NonBreaching].', StateReasonData recentDatapoints=[] after 8 integration-served 500s, for up to 360s.
Cause 2 — expected: HTTP 500 from the stage when the Lambda integration has no lambda:InvokeFunction grant; actual: AssertionError 'with no lambda:InvokeFunction grant the gateway must reject with 500, got 200'.

## Steps to Reproduce
1. CreateApi (ProtocolType=HTTP) with a Lambda integration and a $default or explicit route; CreateStage with AutoDeploy and a configured stage name.
2. Grant apigateway.amazonaws.com lambda:InvokeFunction on the target function and issue N requests to the published invoke URL; observe the 200s/429s/500s on the wire.
3. Poll cloudwatch GetMetricStatistics for namespace AWS/ApiGateway, MetricName Count (then 4xx, then 5xx), Statistics=[Sum], Dimensions=[{ApiId},{Stage}] over the request window -> emulator returns no datapoints (0.0) indefinitely.
4. PutMetricAlarm over AWS/ApiGateway 5xx with Threshold 3, Period 60, EvaluationPeriods 1, TreatMissingData notBreaching, AlarmActions=[an SNS topic]; serve 8 integration 500s and poll DescribeAlarms -> stays OK with 'no datapoints were received'.
5. Repeat step 2 on a second route whose function has NO resource-policy grant -> emulator answers 200 instead of 500.
