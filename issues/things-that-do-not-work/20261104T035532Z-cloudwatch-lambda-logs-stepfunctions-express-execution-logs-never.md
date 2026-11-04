Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights
Issue-Key: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights|_await_delivery_ready#19|TimeoutError|raise TimeoutError(f"a warm-up execution reaching {group}: the run budget ended first; not graded")
Issue-Commit: c083bea380b29cc5238d586af0774e0d5864cbe3

# cloudwatch__lambda__logs__stepfunctions: EXPRESS execution logs never reach the vended log group, so the metric-filter / subscription-filter / Insights counting of failed executions never starts

> _Note: this issue was identified against the code base at commit `c083bea380b29cc5238d586af0774e0d5864cbe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: Step Functions writes no execution-history log events into the CloudWatch Logs vended log group named by an EXPRESS state machine's loggingConfiguration, so the log-based consumers attached to that group (a metric filter, a Lambda subscription filter and a Logs Insights query) never receive an ingested event and the check dies in its delivery-ready warm-up.

AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): CreateStateMachine.loggingConfiguration targets LoggingConfiguration{level: LogLevel [ALL, ERROR, FATAL, OFF], includeExecutionData, destinations "Limited to size 1. Required, if your log level is not set to OFF"}; LogDestination.cloudWatchLogsLogGroup.logGroupArn "must end with :*". Beyond the model this is recorded behavior only: with logging on, each execution-history event of an EXPRESS run is delivered to the group as a JSON log event carrying `type` (e.g. ExecutionStarted, FailStateEntered, ExecutionFailed), `id` and `execution_arn`. Because those events are ordinary ingested log events, a logs metric filter with pattern {$.type = "ExecutionFailed"} increments its metric once per failed execution, a subscription filter delivers exactly the matching events to its destination, and a Logs Insights query `filter type="ExecutionFailed" | stats count(*) by execution_arn` returns one row per failed execution — and no successful execution's ARN appears on any of those surfaces.

Emulator: the group stays empty. The check's readiness gate, which simply waits for one warm-up execution's events to appear in the group, exhausts its budget and the check errors with `TimeoutError: a warm-up execution reaching /aws/vendedlogs/states/<group>: the run budget ended first; not graded` (raised from _await_delivery_ready). Nothing downstream — metric-filter sums, subscription delivery, Insights rows — is ever exercised.

Why: every log-derived signal a client builds on Step Functions execution logging reads zero. Alarms on a metric filter counting ExecutionFailed never fire, subscription-filter consumers (e.g. a Lambda forwarder) are never invoked, and Insights queries return no rows — so a failing production workflow is indistinguishable from a healthy one.

Fix: the Step Functions execution-logging path — the component that materializes a state machine's loggingConfiguration as CloudWatch Logs delivery (log-stream creation plus a per-history-event PutLogEvents emitter honouring level and includeExecutionData), in the sfn crate's execution engine/state. Once events are ingested, the existing Logs metric-filter / subscription-filter / Insights machinery can be re-graded by replaying this check.

Traces: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights, whose detail is the _await_delivery_ready TimeoutError above. Same root cause as the two log-delivery issues filed in this sweep for the `stepfunctions` and `logs__stepfunctions` headings; filed separately because it is a different service heading. No open issue covers Step Functions vended log delivery.

## Evidence
Expected (AWS, per the recording): after 2 successful and 3 failing EXPRESS executions on a state machine logging at level ALL into the group, the metric filter 'express-failed' with pattern {$.type = "ExecutionFailed"} sums 3 in its namespace, the subscription filter delivers exactly the 3 FailStateEntered events, and `filter type="ExecutionFailed" | stats count(*) by execution_arn` returns 3 rows of count 1, with no successful execution's ARN on any surface.

Actual (emulator): the check never gets past its readiness gate — no warm-up execution's events ever reach the group:
  File "...obs_sfn_3_express_failures_counted_by_metric_filter_subscriber_and_insights.py", line 96, in run -> _await_delivery_ready(...)
  File "...", line 370, in _await_delivery_ready -> raise TimeoutError(...)
  TimeoutError: a warm-up execution reaching /aws/vendedlogs/states/sim2real-xsvc-obssfn3-<id>: the run budget ended first; not graded
(the group name is per-run; the requirement is that some execution's events arrive at the configured group at all).

## Steps to Reproduce
1. logs.CreateLogGroup for /aws/vendedlogs/states/<unique>.
2. logs.PutMetricFilter on that group with filterPattern '{$.type = "ExecutionFailed"}' and a metric transformation, and logs.PutSubscriptionFilter to a Lambda destination.
3. sfn.CreateStateMachine with type=EXPRESS, a role trusted by states.amazonaws.com holding the documented logs:* delivery actions, and loggingConfiguration={level:"ALL", includeExecutionData:true, destinations:[{cloudWatchLogsLogGroup:{logGroupArn:"arn:aws:logs:<region>:<account>:log-group:<group>:*"}}]}.
4. sfn.StartExecution a few times with inputs that succeed and inputs that reach a Fail state.
5. Poll logs.FilterLogEvents on the group. Expected: ExecutionStarted/FailStateEntered/ExecutionFailed events for those executions; then cloudwatch.GetMetricStatistics sums 3 and logs.StartQuery/GetQueryResults returns 3 rows. Observed: the group never receives any event, so nothing downstream is observable.
