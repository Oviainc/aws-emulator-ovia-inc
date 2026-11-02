Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-refarch-rekreup-lambda-invocation-metric-reads
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-refarch-rekreup-lambda-invocation-metric-reads
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetMetricStatistics|2xx->2xx|$
Issue-Key: monitoring|GetMetricStatistics|2xx->2xx|$.Datapoints[*]
Issue-Key: monitoring|GetMetricStatistics|2xx->2xx|$.Datapoints[*].Sum
Issue-Key: monitoring|ListMetrics|2xx->2xx|$
Issue-Key: monitoring|ListMetrics|2xx->2xx|$.Metrics[*]
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# CloudWatch never records AWS/Lambda Invocations metrics for invoked functions

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
This issue bundles TWO causes sharing one fault: the emulator publishes no AWS/Lambda Invocations metric data into CloudWatch, so both the datapoint read and the metric-discovery read come back empty and the fixture's polling loop exhausts its budget. There are 2 causes to close.

Dependency to be aware of: the same fixture's setup steps call lambda Invoke, which the emulator answers 501. That lambda-side defect is already owned by the open issue "lambda synchronous Invoke data plane answers 501 NotImplemented" (20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md) and is NOT re-filed here. The two causes below are nonetheless CloudWatch-side requirements: once invocations occur, CloudWatch must carry the AWS/Lambda Invocations metric, and nothing in the emulator today publishes it. Both causes should be re-verified after the lambda Invoke issue is closed.

Cause 1 of 2: GetMetricStatistics over AWS/Lambda Invocations returns no datapoints.
  AWS: recorded behavior only - the Smithy model does not enumerate service-published metrics. The recording shows 200 with Label "Invocations" and Datapoints containing one entry whose Sum equals the number of invocations performed and whose Unit is "Count" (the Timestamp is a per-run value and is asserted only for shape). AWS Lambda publishes an Invocations metric into the AWS/Lambda namespace, dimensioned by FunctionName, for every invocation.
  Emulator: 200 with Label "Invocations" but Datapoints: [], and it stays empty for the whole polling window (the fixture polled 7 times over a 60s cap).
  Why: the 200 and the Label make the response look healthy, so a client waiting for an invocation count to appear - an autoscaling check, a deployment smoke test, an alarm on Invocations - polls forever and then concludes the function was never called. Note the count in the recording (Sum 5.0) is the number of invocations the fixture performed, not a fixed requirement; the requirement is that the Sum reflects the invocations and Unit is "Count".
  Fix: the metric ingestion/state layer - Lambda invocations must publish an AWS/Lambda Invocations datapoint that GetMetricStatistics aggregates.
  Traces: cloudwatch-refarch-rekreup-lambda-invocation-metric-reads#task#9 (plus the same fixture's task#10).

Cause 2 of 2: ListMetrics does not discover the AWS/Lambda Invocations metric.
  AWS: recorded behavior only. The recording shows 200 with two Metrics entries, both Namespace "AWS/Lambda" and MetricName "Invocations" - one dimensioned [FunctionName] and one dimensioned [FunctionName, Resource]. The function name is a per-run value; the requirement is that an invoked function registers both dimension sets under AWS/Lambda.
  Emulator: 200 with Metrics: [], still empty after the full polling window.
  Why: metric discovery is how a client finds what it can alarm or chart on; with an empty list, tooling that enumerates AWS/Lambda metrics before creating an alarm concludes there is nothing to watch. This is a separate read path from cause 1 and will not be exercised by fixing cause 1 alone.
  Fix: the metric registry/state that backs ListMetrics - register the metric name, namespace and both dimension sets when a Lambda invocation is recorded.
  Traces: cloudwatch-refarch-rekreup-lambda-invocation-metric-reads#task#13.

## Evidence
Cause 1 - GetMetricStatistics: expected status 200 with {"Label": "Invocations", "Datapoints": [{"Sum": <number of invocations>, "Timestamp": <shaped>, "Unit": "Count"}]}; actual status 200 with {"Label": "Invocations", "Datapoints": []}. The replay reports "$.Datapoints[0].Sum: poll: not in [5] after 7 attempt(s) (expected 5.0, got None)" and "poll capped at 60s", so the datapoint never appeared within the polling window.
Cause 2 - ListMetrics: expected status 200 with Metrics = [{Namespace "AWS/Lambda", MetricName "Invocations", Dimensions [{Name "FunctionName", Value <fn>}]}, {Namespace "AWS/Lambda", MetricName "Invocations", Dimensions [{Name "FunctionName", Value <fn>}, {Name "Resource", Value <fn>}]}]; actual status 200 with Metrics = []. The replay reports "$.Metrics[0]: poll: still absent after 7 attempt(s)" and "poll capped at 60s".

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Fixture cloudwatch-refarch-rekreup-lambda-invocation-metric-reads creates a Lambda function and invokes it several times in setup, then reads the resulting CloudWatch metrics.
3. Cause 1: step task#9 - GetMetricStatistics for Namespace AWS/Lambda, MetricName Invocations, Statistics [Sum], dimensioned by FunctionName. Expect one datapoint with Unit "Count" and a Sum equal to the invocation count; observe Datapoints: [] for the whole 60s polling window.
4. Cause 2: step task#13 - ListMetrics for Namespace AWS/Lambda. Expect two Invocations entries (FunctionName, and FunctionName+Resource); observe Metrics: [].
5. Re-run after the lambda Invoke 501 issue is closed, since the invocations themselves currently fail in setup.
