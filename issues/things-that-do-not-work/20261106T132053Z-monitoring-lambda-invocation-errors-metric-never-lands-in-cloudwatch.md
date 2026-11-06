Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-refarch-rekreup-lambda-invocation-metric-reads
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-refarch-rekreup-lambda-invocation-metric-reads
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetMetricStatistics|2xx->2xx|$
Issue-Key: monitoring|GetMetricStatistics|2xx->2xx|$.Datapoints[*]
Issue-Key: monitoring|GetMetricStatistics|2xx->2xx|$.Datapoints[*].Sum
Issue-Commit: 6628eaf729fd043eb1e3d7ab7ab4bb9988b48bad

# Lambda invocation Errors metric never lands in CloudWatch, so GetMetricStatistics returns an empty Datapoints list forever

> _Note: this issue was identified against the code base at commit `6628eaf729fd043eb1e3d7ab7ab4bb9988b48bad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: CloudWatch GetMetricStatistics over the AWS/Lambda `Errors` metric for a function that has just been invoked and has failed never yields any datapoint — the metric series stays permanently empty, so a client that polls for it never converges.

AWS: Smithy (CloudWatch, graniteserviceversion20100801) GetMetricStatistics -> GetMetricStatisticsOutput{Label: MetricLabel, Datapoints: Datapoints}; input requires Namespace (@length 1..255, @pattern ^[^:]), MetricName, StartTime, EndTime, Period (@range min 1) and takes Statistics (@length 1..5) plus optional Dimensions and Unit. The model describes only the read shape; that the AWS/Lambda namespace is populated with per-function `Errors`/`Invocations` data by the Lambda service itself is recorded behavior only. The recording shows a 200 whose body carries Label equal to the requested MetricName ("Errors") and a non-empty Datapoints list holding one Datapoint per aggregation period, each with a Timestamp, the requested statistic (here Sum, whose value is the number of failed invocations the function actually recorded in the window) and Unit "Count". The Sum value and the Timestamp are per-run values filled in from the function's actual invocation history — the requirement is that a datapoint for the period exists at all, with Unit "Count" and the statistic reflecting the failed-invocation count, not any one run's number.

Emulator: the call answers 200 with the correct Label ("Errors") but `Datapoints: []`. The replay polled the read for the full 60s cap (7 attempts) and the list never became non-empty, so `$.Datapoints[0]` and `$.Datapoints[0].Sum` stay absent. The response shape and status are right; the metric data behind it is simply never produced, which means the emulator's Lambda invoke path does not emit an AWS/Lambda `Errors` datum into the monitoring metric store (or the store does not aggregate it into the queried Namespace/MetricName/Dimensions series).

Why: every client that uses CloudWatch to confirm a Lambda failure — retry/alarm wiring, dashboards, deployment canaries, any "wait until Errors > 0" or "alarm on Errors" loop — sees a function that errored as a function with no errors. On AWS the poll ends after the aggregation period; against the emulator it spins until the client's own timeout and then either fails or silently concludes the invocation succeeded. Alarms keyed on AWS/Lambda Errors can never leave INSUFFICIENT_DATA, so error-driven workflows built on this signal cannot be exercised at all.

Fix: the metric-publication side of the invoke path plus the monitoring service's metric state — the emulator's Lambda invocation handler must publish the per-invocation AWS/Lambda datums (at minimum `Errors` alongside `Invocations`, dimensioned by FunctionName, Unit Count) into the same metric store GetMetricStatistics/GetMetricData read, and the monitoring metric store's aggregation must bucket them into the requested Period so the Sum statistic resolves. No change is needed in GetMetricStatistics' request validation or serializer: Label and the envelope are already correct.

Traces: fixture cloudwatch-refarch-rekreup-lambda-invocation-metric-reads step task#10 (expected {'Label': 'Errors', 'Datapoints': [{'Sum': 2.0, 'Timestamp': <any>, 'Unit': 'Count'}]}, got {'Label': 'Errors', 'Datapoints': []}; poll capped at 60s after 7 attempts). No open issue covers this: the open monitoring issues concern alarm field coverage, validation wording/exception types, dashboards, contributor insights, metric streams, anomaly detectors and PutMetricData Unit validation — none of them concerns whether Lambda-emitted metric data reaches the metric store.

## Evidence
Fixture cloudwatch-refarch-rekreup-lambda-invocation-metric-reads, step task#10, operation GetMetricStatistics (service monitoring), verdict mismatch, expected status 200 / actual status 200.

Expected (recorded AWS response):
  {'Label': 'Errors', 'Datapoints': [{'Sum': 2.0, 'Timestamp': '<shaped-not-asserted>', 'Unit': 'Count'}]}

Actual (emulator):
  {'Label': 'Errors', 'Datapoints': []}

Replay detail, verbatim:
  $: poll capped at 60s (expected {'Datapoints': [{'Sum': 2.0, 'Timestamp': '<shaped-not-asserted>', 'Unit': 'Count'}], 'Label': 'Errors'}, got {'Label': 'Errors', 'Datapoints': []})
  $.Datapoints[0]: missing (expected {'Sum': 2.0, 'Timestamp': '<shaped-not-asserted>', 'Unit': 'Count'}, got None)
  $.Datapoints[0].Sum: poll: not in [2] after 7 attempt(s) (expected 2.0, got None)

The Sum value 2.0 and the Timestamp are per-run values (they follow from how many invocations failed in that fixture and when); the requirement under test is that at least one Datapoint exists for the window with Unit 'Count' and the Sum statistic equal to the failed-invocation count the emulator itself recorded. Status, Label and the response envelope already match, so this is a missing-metric-data defect, not a protocol or serializer one.

## Steps to Reproduce
1. Replay fixture `cloudwatch-refarch-rekreup-lambda-invocation-metric-reads` against the emulator and watch step `task#10`.
2. Equivalently, by hand: create a Lambda function, invoke it so that the invocations fail (function error), then call CloudWatch GetMetricStatistics with Namespace='AWS/Lambda', MetricName='Errors', Dimensions=[{Name:'FunctionName', Value:<the function>}], Statistics=['Sum'], Period=60 and a StartTime/EndTime window covering the invocations.
3. Observe the emulator answers 200 with Label='Errors' and Datapoints=[] and keeps doing so on repeated polls (the replay retried for 60s / 7 attempts).
4. On AWS the same read returns a Datapoint for the covering period with Unit='Count' and Sum equal to the number of failed invocations.
