Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected
Issue-Key: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: e42d9f0f95dc249c28ad9205e8f81eb720b8f287

# CloudWatch Logs: a metric filter that is accepted with dimensions publishes no dimensioned datapoint to CloudWatch, so the check hangs polling GetMetricStatistics (1 cause)

> _Note: this issue was identified against the code base at commit `e42d9f0f95dc249c28ad9205e8f81eb720b8f287`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
Cause 1 of 1: a metric filter whose metricTransformations carry `dimensions` is accepted by PutMetricFilter and read back by DescribeMetricFilters, but matching log events ingested afterwards never materialize a CloudWatch datapoint for that metric, so any CloudWatch read of it never converges.

Related open issue: 20261024T150703Z-logs-cloudwatch-logs-metric-filters-emit-no-cloudwatch-metric.md reports the same missing metric-filter-to-CloudWatch publication for service `logs`. The replay report does NOT mark this check as owned by it, and this check is grounded in the dimensioned-metric path of the `cloudwatch__logs` pair, so it is filed separately and that issue is named here rather than duplicated silently.

AWS: Smithy (com.amazonaws.cloudwatchlogs) MetricTransformation requires metricName, metricNamespace and metricValue and has optional `dimensions` (map, documented "The fields to use as dimensions for the metric. One metric filter can include as many as three dimensions", values of the form `$.field` extracted from the log event) and optional `defaultValue` (double). AWS refuses the pair with InvalidParameterException and the fixed message "Invalid metric transformation: dimensions and default value are mutually exclusive properties" — the emulator already produces this refusal correctly, and that half is not the defect. The requirement that IS violated: for an ACCEPTED filter, every log event ingested into the subscribed log group that matches the filterPattern must publish metricValue to CloudWatch under the transformation's metricNamespace/metricName, with one CloudWatch dimension per entry of `dimensions` whose value is the value the JSON selector resolved to in that event. Three matching events each with metricValue "1" must therefore make Sum over the enclosing period equal 3 for that metric with that dimension set, and the refused filter must publish nothing under any dimension set.

Emulator: the refusal and the control filter's PutMetricFilter/DescribeMetricFilters round-trip succeed and PutLogEvents accepts the batch with no rejectedLogEventsInfo, but cloudwatch GetMetricStatistics(Namespace=<transformation namespace>, MetricName=<Control metric>, Dimensions=[{Name:"Service",Value:<the value the $.service selector resolved to>}], Period=60, Statistics=["Sum"]) keeps returning an empty Datapoints list. The check polls that read and never reaches Sum>=3, so it is killed by the harness: "Check timed out after 60.0s". No error code or non-2xx is returned anywhere — the writes are accepted and the extracted metric simply never exists, which is why the failure surfaces as a hang rather than a mismatch.

Why: a client that wires log-derived monitoring (metric filter -> metric -> alarm) sees every control-plane call succeed and then waits forever on a metric that will never appear. Dashboards read empty, alarms built on the extracted metric sit in INSUFFICIENT_DATA instead of breaching, and the negative half of the contract (the refused filter publishes nothing) is untestable because the accepted filter's datapoint — the arrival barrier that proves absence is not lag — never lands. Any test or deploy gate that waits for the extracted datapoint hangs instead of failing fast.

Fix: the ingest-time metric-extraction path of CloudWatch Logs — PutLogEvents must evaluate each stored metric filter's filterPattern against every ingested event and, on a match, publish metricValue into the CloudWatch metric store under the transformation's namespace/metricName, resolving each `dimensions` selector against the event to build the datapoint's dimension set (and honouring `unit` and `defaultValue` on non-match); and the CloudWatch metric state those datapoints must be visible in from GetMetricStatistics. The accepted filter's persistence and the dimensions/defaultValue refusal in the PutMetricFilter validation are already correct and must not regress.

Traces: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected (verdict mismatch; "Check timed out after 60.0s").

## Evidence
Failing check: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected [mismatch]
Reported error: "Check timed out after 60.0s"

Expected (AWS): after PutMetricFilter is accepted with metricTransformations=[{metricName:<M>, metricNamespace:<NS>, metricValue:"1", dimensions:{"Service":"$.service"}}] and three JSON log events matching filterPattern '{ $.status = "fail" }' are ingested, GetMetricStatistics(Namespace=<NS>, MetricName=<M>, Dimensions=[{Name:"Service",Value:<resolved $.service>}], Period=60, Statistics=["Sum"]) returns datapoints summing to 3.0. The sibling filter refused with InvalidParameterException "Invalid metric transformation: dimensions and default value are mutually exclusive properties" (fixed text; no variable parts) must publish nothing, i.e. Sum 0.0 both with and without the dimension.

Actual (emulator): the refusal envelope matches, PutMetricFilter persists only the accepted filter (DescribeMetricFilters lists it alone), PutLogEvents returns 200 with no rejectedLogEventsInfo — and GetMetricStatistics for the accepted transformation returns an empty Datapoints list on every poll. The check's poll for Sum>=3 never converges and the harness kills it at 60s, so the emulator never publishes the extracted metric at all.

## Steps to Reproduce
1. Clone the repo and boot the emulator, then replay cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected.
2. logs.CreateLogGroup(<group>) and logs.CreateLogStream(<group>, "events").
3. logs.PutMetricFilter(<group>, filterName="dimdef-refused", filterPattern='{ $.status = "fail" }', metricTransformations=[{metricName:"Refused", metricNamespace:<NS>, metricValue:"1", defaultValue:0.0, dimensions:{"Service":"$.service"}}]) — observe the correct InvalidParameterException refusal.
4. logs.PutMetricFilter(<group>, filterName="dimdef-control", same filterPattern, metricTransformations=[{metricName:"Control", metricNamespace:<NS>, metricValue:"1", dimensions:{"Service":"$.service"}}]) — accepted.
5. logs.PutLogEvents(<group>, "events", three events with bodies {"service":"checkout","status":"fail","n":i}).
6. Poll cloudwatch.GetMetricStatistics(Namespace=<NS>, MetricName="Control", Dimensions=[{Name:"Service",Value:"checkout"}], Period=60, Statistics=["Sum"]) over a ~20 minute window. Observed: Datapoints is always empty and the poll never reaches Sum 3.0, so the check times out instead of proceeding to assert the refused filter published nothing.
