Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs__sns__sqs.alarm-log-metric-chain-deleted-filter-stops-counting-but-keeps-datapoints cloudwatch__lambda__logs__sns__sqs.alarm-log-metric-chain-removed-subscription-stops-chain-at-sqs
Issue-Key: cloudwatch__lambda__logs__sns__sqs.alarm-log-metric-chain-deleted-filter-stops-counting-but-keeps-datapoints|_poll_until#3|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: cloudwatch__lambda__logs__sns__sqs.alarm-log-metric-chain-removed-subscription-stops-chain-at-sqs|_poll_until#14|Timeout|last = probe()
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# cloudwatch__lambda__logs__sns__sqs: a Lambda invocation's output never reaches /aws/lambda/<function>, so the log-metric-filter → alarm → SNS → SQS chain produces no datapoint and never settles

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: a function that prints a line during a successful invocation leaves its own CloudWatch Logs group empty, so nothing downstream of the log group — metric filters, the metric they publish, the alarm on that metric, its SNS action and the subscribed SQS queue — can ever be exercised.

AWS: the Lambda runtime writes every invocation's log to `/aws/lambda/<function-name>`: the group is created on the first invocation when the execution role allows logs:CreateLogGroup/CreateLogStream/PutLogEvents, a stream is opened per execution environment, and the invocation contributes a `START RequestId: <id> Version: <version>` line, each line the handler wrote to stdout, an `END RequestId: <id>` line and a `REPORT RequestId: <id>\tDuration: … ms\tBilled Duration: … ms\tMemory Size: … MB\tMax Memory Used: … MB` line. The `START`/`END`/`REPORT RequestId:` prefixes and the `Duration`/`Billed Duration`/`Memory Size`/`Max Memory Used` field names are fixed text; ids, durations and sizes are per-invocation values. Those events are readable through FilterLogEvents/GetLogEvents within seconds, and every metric filter on the group counts the matching lines into its CloudWatch metric, which a datapoint query then returns and an alarm then evaluates. Recorded behavior for these checks: each printed `ERROR …<marker>` line is counted exactly once by each matching filter, a deleted filter stops counting new lines while keeping the datapoints it already produced, and the alarm's SNS action reaches the subscribed queue until the subscription is removed.

Emulator: the printed line never appears in the group at all. The harness polls for it and gives up: `TimeoutError: a printed line reaching log group /aws/lambda/<function-name>: the run budget ended first; not graded`. With no log event, no metric filter can emit a datapoint, so the metric, the alarm transition, the SNS publish and the SQS delivery behind it are all unreachable and the second check simply exhausts its 60s budget. The Invoke itself succeeds — only the log write-through is missing.

Why: a client relying on AWS's behavior loses the entire log-observability chain: `aws logs tail`, log-driven metric filters, log-based alarms and the notification paths built on them all read an empty group, and an operator cannot tell a function that logged nothing from one whose logs were dropped.

Fix: the Lambda Invoke execution path's log emission — services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs together with src/state/execution (which already captures the run's log) must write that log through to the CloudWatch Logs state store under `/aws/lambda/<function-name>`, creating the group and stream and emitting the START/END/REPORT framing around the handler's own stdout, so that metric filters on the group see the events. The related open issue 20261029T182452Z-cloudwatch-lambda-logs-cloudwatch-logs-metric-filter-refusal-texts-are-rewritten.md covers metric-filter refusal wording and CLI invocations blocked by the old NotImplemented Invoke; it does not carry this log write-through cause or these replay keys.

Traces: both failing checks in trace_ids.

## Evidence
Expected (AWS): after the invocation, FilterLogEvents on /aws/lambda/<function-name> returns the handler's printed line (and the START/END/REPORT framing), each matching metric filter reports Sum == the number of printed matching lines, and the alarm's SNS action reaches the subscribed queue.
Actual (emulator):
  cloudwatch__lambda__logs__sns__sqs.alarm-log-metric-chain-deleted-filter-stops-counting-but-keeps-datapoints —
    TimeoutError: a printed line reaching log group /aws/lambda/sim2real-xsvc-fn-<id>: the run budget ended first; not graded
  cloudwatch__lambda__logs__sns__sqs.alarm-log-metric-chain-removed-subscription-stops-chain-at-sqs —
    Error: Check timed out after 60.0s (the chain never produces its first datapoint, so the removed-subscription arm is never reached).

## Steps to Reproduce
Replay both checks in trace_ids. Minimal repro: CreateFunction (Runtime=python3.12) whose handler prints a marker line and returns; PutMetricFilter on /aws/lambda/<function-name> with a pattern matching that marker and a metricTransformation into a test namespace; Invoke the function N times; then FilterLogEvents on the group (returns nothing on the emulator, the printed lines on AWS) and GetMetricStatistics/GetMetricData on the transformation's metric (no datapoints on the emulator, Sum == N on AWS).
