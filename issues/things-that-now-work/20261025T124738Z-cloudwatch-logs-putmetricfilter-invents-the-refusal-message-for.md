Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected
Issue-Key: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected|run#14|AssertionError|assert err == expected, (
Issue-Commit: 140be5bc82ff93d772ac6ec97ebe27a7ad9b5162

# CloudWatch Logs PutMetricFilter invents the refusal message for a metric transformation that sets both dimensions and defaultValue (1 cause)

> _Note: this issue was identified against the code base at commit `140be5bc82ff93d772ac6ec97ebe27a7ad9b5162`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
One net-new root cause in the CloudWatch Logs emulator. The refusal carries the RIGHT error code and HTTP status but a message the emulator authors itself instead of the text AWS returns for this error class.

Cause 1 of 1: PutMetricFilter's dimensions+defaultValue mutual-exclusion refusal uses emulator-authored prose.

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) types MetricTransformation.dimensions (map, "The fields to use as dimensions for the metric") and MetricTransformation.defaultValue (double, "(Optional) The value to emit when a filter pattern does not match a log event") as INDEPENDENTLY OPTIONAL members and declares no mutual exclusion between them; PutMetricFilter declares InvalidParameterException, errorKind client, httpError 400, doc "A parameter is specified incorrectly." The rule and its wording are therefore recorded behavior only: a PutMetricFilter whose single metricTransformations entry sets BOTH dimensions and defaultValue is refused with HTTP 400, code InvalidParameterException and the message "Invalid metric transformation: dimensions and default value are mutually exclusive properties". That message is ENTIRELY fixed text for this error class — it interpolates nothing from the request or from state (no filter name, no log group name, no metric name, no dimension name, no default value).

Emulator: same error code and same 400 status, but the message is "defaultValue cannot be specified for a metric transformation that declares dimensions" — emulator-authored prose naming the two request members instead of AWS's "Invalid metric transformation: ..." sentence. The guard itself is correct and does fire (the check's control filter without defaultValue is still accepted), so the ONLY divergence is the message text.

Why: a client that distinguishes this validation failure from the other InvalidParameterException causes of PutMetricFilter (bad filter pattern, too many regexes, bad emitSystemFieldDimensions, dimension budget) has only the message to key on, because code and status are shared across all of them. Anything that matches on, logs, or surfaces that message — a deployment tool reporting why a metric filter was rejected, a test asserting the recorded response, an operator reading the error — sees text AWS never emits and cannot classify the failure. The same class of defect is already filed for other Logs operations in open issue 20261025T114825Z-logs-cloudwatch-logs-error-messages-are-invented-deleteindexpolicy-s.md (DeleteIndexPolicy not-found, DeleteResourcePolicy stale revision) under service `logs`; that issue does not name PutMetricFilter or this validation branch, and does not carry this check's replay key.

Fix: the PutMetricFilter handler's metric-transformation validation and the shared state helper it calls — services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_metric_filter.rs lines 255-269 (the loop over input.metric_transformations that maps Err(message) onto the local `invalid` constructor) and crate::state::check_metric_transformation in services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/state.rs, which produces the message string. Keep the guard and the InvalidParameterException/400 classification; emit AWS's fixed text for this error class.

Traces: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected.

## Evidence
Check cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected, verdict mismatch:

AssertionError: PutMetricFilter with both dimensions and defaultValue returned ('InvalidParameterException', 'defaultValue cannot be specified for a metric transformation that declares dimensions'), expected ('InvalidParameterException', 'Invalid metric transformation: dimensions and default value are mutually exclusive properties')

Expected (AWS, recorded): HTTP 400, code InvalidParameterException, message "Invalid metric transformation: dimensions and default value are mutually exclusive properties" — all fixed text for the error class, no request- or state-derived parts.
Actual (emulator): HTTP 400, code InvalidParameterException, message "defaultValue cannot be specified for a metric transformation that declares dimensions".

The error code and the 400 status already match; only the message diverges. The refusal's effect is otherwise correct in the recording's own terms: the check's next assertion (the accepted control filter is the only filter DescribeMetricFilters lists for the group) is what it proceeded to.

## Steps to Reproduce
1. logs.CreateLogGroup(logGroupName=<group>); logs.CreateLogStream(logGroupName=<group>, logStreamName='events').
2. logs.PutMetricFilter(logGroupName=<group>, filterName='dimdef-refused', filterPattern='{ $.status = "fail" }', metricTransformations=[{'metricName':'Refused','metricNamespace':<ns>,'metricValue':'1','defaultValue':0.0,'dimensions':{'Service':'$.service'}}]).
   Expect HTTP 400, code InvalidParameterException, message "Invalid metric transformation: dimensions and default value are mutually exclusive properties"; the emulator answers code InvalidParameterException with message "defaultValue cannot be specified for a metric transformation that declares dimensions".
3. Positive control (passes today): the identical PutMetricFilter WITHOUT defaultValue is accepted, and DescribeMetricFilters(logGroupName=<group>) lists only that accepted filter — confirming the guard fires and only its message text is wrong.
