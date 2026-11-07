Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights
Issue-Key: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights|_poll_until_settled#27|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# cloudwatch__lambda__logs__stepfunctions: a metric filter over the vended EXPRESS log group publishes no custom metric, so the filter's counter never appears in CloudWatch

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
Cause 1 of 1 — a CloudWatch Logs metric filter on a Step Functions vended log group never publishes its metric.
AWS: recorded behavior (CloudWatch Logs dev guide, MonitoringLogData): a metric filter created with PutMetricFilter on a log group transforms each matching log event into a datapoint of the configured metricTransformation (here namespace Sim2Real/<run>, metricName ExpressExecutionFailed, value 1), so after N matching events the metric's Sum in that namespace is N and then holds steady. The matching events here are the ExecutionFailed events of EXPRESS runs delivered to the vended group.
Emulator: the check polls the filter's metric until it settles at the expected count and never gets a reading — the custom metric is absent from CloudWatch for the whole replay budget (which also means no matching log event ever reached the group to be transformed).
Why: the standard way to alarm on Step Functions EXPRESS failures (metric filter -> custom metric -> alarm) produces no metric at all, so the alarm can never fire and a client reads the absence as "no failures".
Fix: the CloudWatch Logs metric-filter evaluation path (apply each filter to incoming events and publish the metricTransformation to CloudWatch), which in turn depends on EXPRESS execution logs reaching the vended group.
Related OPEN issue: 20261104T035532Z-cloudwatch-lambda-logs-stepfunctions-express-execution-logs-never.md reports the missing EXPRESS log delivery for this suite and does not carry this check; it does not cover it, so this cause is filed with that issue named.
Traces: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights

## Evidence
From this replay of simparity-behavioral-aws-v1: TimeoutError: metric-filter metric Sim2Real/obssfn3-<run>/ExpressExecutionFailed: the run budget ended first; not graded — raised from the check's _poll_until_settled after 2 succeeding and 3 failing EXPRESS executions, i.e. no datapoint for the filter's metric was ever served.

## Steps to Reproduce
1. Create a /aws/vendedlogs/states/ log group and an EXPRESS state machine logging to it at level ALL.
2. PutMetricFilter on that group with filterPattern 'type="ExecutionFailed"' and metricTransformation {metricNamespace: Sim2Real/<run>, metricName: ExpressExecutionFailed, metricValue: 1}.
3. Run 2 succeeding and 3 failing executions.
4. Poll cloudwatch:GetMetricStatistics for Sim2Real/<run>:ExpressExecutionFailed — the Sum must reach 3 and then hold.
