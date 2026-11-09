Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative cloudwatch__dynamodb__lambda.lambda-ddb-getitem-consumed-capacity-metric-cloudwatch cloudwatch__dynamodb__lambda.lambda-ddb-missing-permission-no-capacity-metric-negative cloudwatch__dynamodb__lambda.lambda-ddb-query-latency-metric-cloudwatch cloudwatch__dynamodb__lambda.lambda-ddb-throttle-triggers-cloudwatch-alarm cloudwatch__dynamodb__lambda.lambda-error-metric-increments-on-exception cloudwatch__dynamodb__lambda.reserved-concurrency-admission-control-and-metric-agreement-rejected cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-event-count-metrics-are-opt-in-per-mapping
Issue-Key: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm|run#11|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative|run#11|AssertionError|assert refused == "AccessDenied", (
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-getitem-consumed-capacity-metric-cloudwatch|run#10|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-missing-permission-no-capacity-metric-negative|_poll_until#21|AssertionError|raise AssertionError(f"poll timed out after {timeout_s}s; last value was {last!r}")
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-query-latency-metric-cloudwatch|run#10|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-throttle-triggers-cloudwatch-alarm|_invoke#10|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: cloudwatch__dynamodb__lambda.lambda-error-metric-increments-on-exception|run#10|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb__lambda.reserved-concurrency-admission-control-and-metric-agreement-rejected|_wave#32|AssertionError|assert bad == [], (
Issue-Key: cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-event-count-metrics-are-opt-in-per-mapping|run#21|AssertionError|raise AssertionError(
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# cloudwatch__dynamodb__lambda: 7 net-new causes — alarms never transition, AWS/Lambda and AWS/DynamoDB metrics never publish, PutMetricData is not IAM-gated, the Python runtime lacks botocore.config, and reserved-concurrency invocations time out

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Seven distinct root causes. Each is listed with its own failing check ids.

Cause 1 of 7: a CloudWatch alarm on AWS/Lambda Errors never transitions to ALARM.
AWS: an alarm on `AWS/Lambda` `Errors` with a FunctionName dimension evaluates the datapoints Lambda publishes for failing invocations and transitions OK -> ALARM once the threshold is breached, with a `Threshold Crossed: ...` StateReason. Recorded behavior: the check drives failing invocations and polls DescribeAlarms for up to 6 minutes.
Emulator: the alarm never transitions within the window; the alarm evaluator never sees an Errors datapoint.
Why: alarm-based error-rate monitoring reports a permanently healthy function while it is failing.
Fix: publication of AWS/Lambda Errors from the invoke path plus the CloudWatch alarm evaluator.
Traces: cloudwatch-alarm-on-lambda-error-rate-transitions-alarm.

Cause 2 of 7: AWS/Lambda Errors is never published at all.
AWS: each invocation that ends in a function error increments the `AWS/Lambda` `Errors` metric for that function (Sum == number of failing invocations) in the same dimension set `FunctionName` (and `Resource`/`ExecutedVersion` variants).
Emulator: `GetMetricStatistics`/`GetMetricData` for AWS/Lambda Errors never reaches the expected count for the failing invocations the check issued.
Why: the built-in error metric is the canonical signal for function health dashboards and deployment gates; it reads zero while the function is failing.
Fix: the Lambda invoke path's metric emission into the CloudWatch metric store.
Traces: lambda-error-metric-increments-on-exception.

Cause 3 of 7: AWS/DynamoDB per-operation metrics for in-function SDK calls are never published.
AWS: DynamoDB publishes `ConsumedReadCapacityUnits` and `SuccessfulRequestLatency` (dimension set TableName, and TableName+Operation for latency) for every served request, including requests made by a Lambda function's SDK client; the datapoints appear within the metric-ingestion window.
Emulator: neither metric ever appears. `ConsumedReadCapacityUnits` never produces a datapoint within the 5-minute poll window after a GetItem, `SuccessfulRequestLatency` for `Query` likewise never appears, and the negative control that polls the same pair observes `(0, 0)` until it times out at 200s.
Why: capacity and latency dashboards, autoscaling signals and the whole "two-plane agreement" between ConsumedCapacity in the API response and the metric plane are unavailable.
Fix: AWS/DynamoDB metric emission from the DynamoDB request path, with the exact per-metric dimension sets.
Traces: lambda-ddb-getitem-consumed-capacity-metric-cloudwatch, lambda-ddb-query-latency-metric-cloudwatch, lambda-ddb-missing-permission-no-capacity-metric-negative.

Cause 4 of 7: `cloudwatch:PutMetricData` from a Lambda function is not authorized against the execution role.
AWS: an in-function `PutMetricData` call made with the execution role's credentials is denied with `AccessDeniedException` when the role has no `cloudwatch:PutMetricData` allow, and the custom metric is never stored. Smithy (cloudwatch): PutMetricData; the refusal is the service-common AccessDeniedException at HTTP 400.
Emulator: the call is not refused — the handler reports `{"refused": null}`, i.e. PutMetricData succeeded for a role with no such permission.
Why: a least-privilege role is silently more capable than its policy; IAM regression tests that assert the deny pass vacuously, and custom metrics land from principals that must not be able to write them.
Fix: the IAM authorization gate in the CloudWatch PutMetricData handler.
Traces: deploy-cloudwatch-composite-alarm-missing-metric-negative.

Cause 5 of 7: the managed Python runtime cannot import `botocore.config`.
AWS: the `python3.x` managed runtimes ship a complete boto3/botocore, so `from botocore.config import Config` in a handler imports cleanly and the invocation runs.
Emulator: the invocation fails before the handler body with `{"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module 'handler': No module named 'botocore.config'"}` — the bundled botocore is missing the `config` submodule.
Why: configuring retries/timeouts via `botocore.config.Config` is routine in Lambda handlers; every such function fails at import, so the behavior under test (throttle-driven alarm) is never reached.
Fix: the Python runtime image/bundle shipped by the Lambda runtime component.
Traces: lambda-ddb-throttle-triggers-cloudwatch-alarm. (The same missing-module cause is already filed for other checks as 20261105T111951Z-cloudwatch-dynamodb-lambda-the-lambda-python-runtime-has.md; that issue does not carry this check's replay key.)

Cause 6 of 7: reserved-concurrency admission control does not run admitted invocations concurrently; they time out.
AWS: with `PutFunctionConcurrency` reserving N, up to N simultaneous invocations are admitted and run in parallel (and the N+1st is throttled with `TooManyRequestsException`/429). The admitted invocations all complete.
Emulator: of 5 admitted invocations, 4 failed or were never observed running concurrently, each returning `{"errorType": "Sandbox.Timedout", "errorMessage": "<requestId> Task timed out after 30.00 seconds"}` — the invocations are serialised, so a barrier that requires all of them to be in flight at once can never be met.
Why: concurrency is a correctness property, not just a performance one: fan-out workloads, barrier/latch patterns and reserved-concurrency throttle tests all break, and callers see spurious timeouts.
Fix: the Lambda invoke scheduler's concurrency admission (reserved concurrency accounting and parallel sandbox execution).
Traces: reserved-concurrency-admission-control-and-metric-agreement-rejected.

Cause 7 of 7: event-source-mapping event-count metrics are never published when MetricsConfig opts in.
AWS: an event source mapping created or updated with `MetricsConfig: {Metrics: [EventCount]}` publishes `AWS/Lambda` `PolledEventCount`, `InvokedEventCount` and `FilteredOutEventCount` for that mapping (dimension set including the EventSourceMappingArn/FunctionName), while a mapping without MetricsConfig publishes none of them.
Emulator: for the opted-in mapping all three read `0.0` with 0 datapoints for the whole 420s window, while the opted-out function's `Invocations` does advance — so the opt-in produces nothing and the per-mapping opt-in semantics cannot be observed.
Why: ESM observability (how many records were polled, invoked, filtered out) is the only way to diagnose filter criteria and poller health; it is permanently empty.
Fix: ESM poller metric emission gated on MetricsConfig, in the event-source-mapping poller.
Traces: xsvc-lambda-1-esm-event-count-metrics-are-opt-in-per-mapping.

## Evidence
Cause 1: AssertionError: alarm never transitioned to ALARM within 6 minutes (AWS/Lambda Errors).
Cause 2: AssertionError: AWS/Lambda Errors metric never reached the expected count within 4 minutes of the failing invocations.
Cause 3: AssertionError: ConsumedReadCapacityUnits datapoint did not appear within a 5min poll window; AssertionError: SuccessfulRequestLatency datapoint for Query did not appear within a 5min poll window; AssertionError: poll timed out after 200s; last value was (0, 0).
Cause 4: AssertionError: invoke 1: PutMetricData without cloudwatch:PutMetricData was not refused AccessDenied (handler returned b'{"refused": null}'). Expected AccessDeniedException at HTTP 400 with the standard "User: <principal> is not authorized to perform: cloudwatch:PutMetricData" text (principal and resource are request/state derived).
Cause 5: AssertionError: lambda invocation failed: {'errorMessage': "Unable to import module 'handler': No module named 'botocore.config'", 'errorType': 'Runtime.ImportModuleError'}.
Cause 6: AssertionError: 4 of 5 admitted invocations of <function> failed or never saw all 5 registered within 30.0s (the target must run them concurrently): [('Unhandled', 200, {'errorType': 'Sandbox.Timedout', 'errorMessage': '<requestId> Task timed out after 30.00 seconds'}), ...] (function name and request ids are per-run values).
Cause 7: AssertionError: barrier not reached in 420s: metrics-on counts {'PolledEventCount': (0.0, 0), 'InvokedEventCount': (0.0, 0), 'FilteredOutEventCount': (0.0, 0)} (want {'PolledEventCount': 5, 'InvokedEventCount': 4, 'FilteredOutEventCount': 1}), metrics-off function Invocations (2.0, 1).

## Steps to Reproduce
Cause 1/2: deploy a function that raises; invoke it N times; PutMetricAlarm on AWS/Lambda Errors FunctionName=<fn>; poll GetMetricStatistics and DescribeAlarms.
Cause 3: from a Lambda function call dynamodb:GetItem and Query on a table; poll GetMetricStatistics for AWS/DynamoDB ConsumedReadCapacityUnits (TableName) and SuccessfulRequestLatency (TableName,Operation=Query).
Cause 4: give the execution role no cloudwatch:PutMetricData; invoke a handler that calls PutMetricData; observe it is not refused.
Cause 5: deploy a python3.x handler whose module does `from botocore.config import Config`; invoke it.
Cause 6: PutFunctionConcurrency reserving 5; fire 5 concurrent Invoke calls at a handler that waits on a shared latch; observe serialisation and Sandbox.Timedout.
Cause 7: CreateEventSourceMapping with MetricsConfig {Metrics:[EventCount]} on a DynamoDB stream and a second mapping without it; write records; poll AWS/Lambda PolledEventCount/InvokedEventCount/FilteredOutEventCount.
