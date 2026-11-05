Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.lambda-cloudwatch-getmetricstatistics-missing-permission-negative cloudwatch__lambda__logs.lambda-custom-boto3-logs-missing-permission-negative cloudwatch__lambda__logs.lambda-custom-boto3-logs-put-log-events-positive cloudwatch__lambda__logs.lambda-custom-putmetricdata-boto3-positive cloudwatch__lambda__logs.lambda-custom-putmetricdata-missing-permission-negative cloudwatch__lambda__logs.lambda-invoke-writes-log-stream-events-positive cloudwatch__lambda__logs.cfn-deploy-lambda-missing-logs-policy-rejected cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow cloudwatch__lambda__logs.cli-lambda-missing-log-permission-negative cloudwatch__lambda__logs.cli-lambda-tail-logs-workflow cloudwatch__lambda__logs.lambda-error-log-format-stacktrace-positive cloudwatch__lambda__logs.lambda-invoke-creates-log-group-positive cloudwatch__lambda__logs.lambda-log-group-retention-configurable cloudwatch__lambda__logs.lambda-missing-logs-permission-no-log-delivery-negative cloudwatch__lambda__logs.logs-metric-filter-creates-cloudwatch-metric-positive cloudwatch__lambda__logs.logs-metric-filter-non-matching-pattern-no-metric-negative cloudwatch__lambda__logs.metrics-from-logs-default-value-zero-fills-non-matching-lambda-lines cloudwatch__lambda__logs.metrics-from-logs-filter-created-after-lines-does-not-count-them cloudwatch__lambda__logs.metrics-from-logs-insights-parse-named-capture-agrees-with-extracted-metric cloudwatch__lambda__logs.metrics-from-logs-insights-stats-by-agrees-with-dimensioned-metric-filter cloudwatch__lambda__logs.metrics-from-logs-json-selector-filter-counts-only-parsed-json-error-lines cloudwatch__lambda__logs.metrics-from-logs-no-matching-line-emits-no-datapoint-and-an-empty-complete-query cloudwatch__lambda__logs.metrics-from-logs-regex-pattern-agrees-across-metric-filter-insights-and-filter-log-events cloudwatch__lambda__logs.metrics-from-logs-size-field-metric-value-is-extracted-not-counted cloudwatch__lambda__logs.metrics-from-logs-space-delimited-filter-counts-lambda-lines-under-declared-dimension cloudwatch__lambda__logs.metrics-math-and-emf-lambda-malformed-emf-line-yields-no-metric-and-stream-keeps-working cloudwatch__lambda__logs.metrics-math-and-emf-lambda-stdout-emf-line-materializes-metric-without-put-metric-data cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected cloudwatch__lambda__logs.metrics-from-logs-dimensions-with-default-value-rejected cloudwatch__lambda__logs.logs-subscription-filter-missing-permission-negative
Issue-Key: cloudwatch__lambda__logs.lambda-cloudwatch-getmetricstatistics-missing-permission-negative|run#7|AssertionError|assert control_resp.get("FunctionError") is None, (
Issue-Key: cloudwatch__lambda__logs.lambda-custom-boto3-logs-missing-permission-negative|run#18|AssertionError|assert payload.get("ok") is False, f"handler unexpectedly reported success: {payload}"
Issue-Key: cloudwatch__lambda__logs.lambda-custom-boto3-logs-put-log-events-positive|run#18|AssertionError|assert payload.get("ok") is True, f"handler reported failure: {payload}"
Issue-Key: cloudwatch__lambda__logs.lambda-custom-putmetricdata-boto3-positive|run#9|AssertionError|assert resp.get("FunctionError") is None, f"unexpected invocation error: {resp.get('FunctionError')}"
Issue-Key: cloudwatch__lambda__logs.lambda-custom-putmetricdata-missing-permission-negative|run#11|AssertionError|assert test_resp.get("FunctionError") is None, (
Issue-Key: cloudwatch__lambda__logs.lambda-invoke-writes-log-stream-events-positive|run#11|AssertionError|assert request_id, "Invoke response carries no request id (x-amzn-RequestId)"
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-lambda-missing-logs-policy-rejected|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow|run#14|Timeout|time.sleep(3)
Issue-Key: cloudwatch__lambda__logs.cli-lambda-missing-log-permission-negative|_poll_for_streams#20|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.cli-lambda-tail-logs-workflow|run#15|Timeout|time.sleep(5)
Issue-Key: cloudwatch__lambda__logs.lambda-error-log-format-stacktrace-positive|_wait_log_events#12|Timeout|time.sleep(3)
Issue-Key: cloudwatch__lambda__logs.lambda-invoke-creates-log-group-positive|_wait_log_group#8|Timeout|time.sleep(2)
Issue-Key: cloudwatch__lambda__logs.lambda-log-group-retention-configurable|_wait_log_group#9|Timeout|time.sleep(2)
Issue-Key: cloudwatch__lambda__logs.lambda-missing-logs-permission-no-log-delivery-negative|_wait_log_group#17|Timeout|time.sleep(3)
Issue-Key: cloudwatch__lambda__logs.logs-metric-filter-creates-cloudwatch-metric-positive|_wait_log_group_exists#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.logs-metric-filter-non-matching-pattern-no-metric-negative|_wait_log_group_exists#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-default-value-zero-fills-non-matching-lambda-lines|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-filter-created-after-lines-does-not-count-them|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-insights-parse-named-capture-agrees-with-extracted-metric|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-insights-stats-by-agrees-with-dimensioned-metric-filter|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-json-selector-filter-counts-only-parsed-json-error-lines|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-no-matching-line-emits-no-datapoint-and-an-empty-complete-query|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-regex-pattern-agrees-across-metric-filter-insights-and-filter-log-events|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-size-field-metric-value-is-extracted-not-counted|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-space-delimited-filter-counts-lambda-lines-under-declared-dimension|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-math-and-emf-lambda-malformed-emf-line-yields-no-metric-and-stream-keeps-working|_poll#23|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Key: cloudwatch__lambda__logs.metrics-math-and-emf-lambda-stdout-emf-line-materializes-metric-without-put-metric-data|_poll#18|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-dimensions-with-default-value-rejected|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Related: 20261023T113357Z-logs-cloudwatch-logs-accepts-requests-real-aws-refuses-putresourcepolicy.md
Issue-Related: 20261029T182452Z-cloudwatch-lambda-logs-cloudwatch-logs-metric-filter-refusal-texts-are-rewritten.md
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cloudwatch__lambda__logs: 5 net-new root causes — no AWS SDK in the Lambda runtime, no request id on Invoke, no log delivery, reworded metric-filter refusals, and PutSubscriptionFilter never refused

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
Cause 1 of 5 — the managed python runtime ships no boto3/botocore.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call logs:PutLogEvents, cloudwatch:PutMetricData and cloudwatch:GetMetricStatistics under its execution role, returning its own result or the service's error code when the role lacks the action.
Emulator: HTTP 200, FunctionError='Unhandled', payload {"errorMessage":"Unable to import module 'lambda_function': No module named 'boto3'","errorType":"Runtime.ImportModuleError"} — fixed text "Unable to import module '<module>': No module named 'boto3'" with the handler module substituted. Checks expecting a handled ClientError in the payload instead see FunctionError='Unhandled'.
Why: no handler can emit a custom metric or a log event, and the missing-permission negatives prove nothing.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: cloudwatch__lambda__logs.lambda-cloudwatch-getmetricstatistics-missing-permission-negative, .lambda-custom-boto3-logs-missing-permission-negative, .lambda-custom-boto3-logs-put-log-events-positive, .lambda-custom-putmetricdata-boto3-positive, .lambda-custom-putmetricdata-missing-permission-negative.

Cause 2 of 5 — the Invoke response carries no request id.
AWS: every AWS response carries the `x-amzn-RequestId` header, which botocore exposes as ResponseMetadata.RequestId and which equals the invocation's request id in the log stream.
Emulator: the header is absent; the check asserts "Invoke response carries no request id (x-amzn-RequestId)".
Why: an invocation cannot be correlated with its log stream events.
Fix: the Invoke handler's response serializer in the lambda crate.
Traces: cloudwatch__lambda__logs.lambda-invoke-writes-log-stream-events-positive.

Cause 3 of 5 — an invocation creates no log group and writes no log events, so log-backed and metric-filter checks poll to their deadline.
AWS: the first invocation creates /aws/lambda/<function-name>, opens a dated stream and writes START/END/REPORT plus handler output; metric filters over that group then produce datapoints, Logs Insights queries return rows, and FilterLogEvents returns the matching lines.
Emulator: recorded behavior only — nothing is delivered, so every such check reports "Check timed out after 60.0s".
Why: log retention, error-format, metric-filter and Insights behaviour are all unobservable, and alarms built on log-derived metrics never fire.
Fix: the Lambda->CloudWatch Logs delivery path in the invoke path.
Traces: the "Check timed out after 60.0s" ids in trace_ids (cfn-deploy-lambda-missing-logs-policy-rejected, cli-describe-log-groups-after-lambda-invoke-workflow, cli-lambda-missing-log-permission-negative, cli-lambda-tail-logs-workflow, lambda-error-log-format-stacktrace-positive, lambda-invoke-creates-log-group-positive, lambda-log-group-retention-configurable, lambda-missing-logs-permission-no-log-delivery-negative, logs-metric-filter-creates-cloudwatch-metric-positive, logs-metric-filter-non-matching-pattern-no-metric-negative, metrics-from-logs-* and metrics-math-and-emf-*).

Cause 4 of 5 — PutMetricFilter's refusal messages are reworded.
AWS: PutMetricFilter refuses more than three dimensions with InvalidParameterException and the message 'Number of dimensions exceeds the maximum number of dimensions supported per metric filter', and a literal dimension value with InvalidParameterException and 'Invalid metric transformation: dimension value must be valid selector'. Those strings are fixed text with no variable part.
Emulator: returns InvalidParameterException with its own prose instead ('a metric filter can include as many as 3 dimensions, counting the system fields emitted as dimensions' and 'each dimension value must name a field of the log event, selected with ...').
Why: tooling and clients that match on the documented message mis-handle a refusal they otherwise understand.
Fix: the message construction in PutMetricFilter's validation.
Traces: cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected, .metrics-from-logs-literal-dimension-value-rejected, .metrics-from-logs-dimensions-with-default-value-rejected.

Cause 5 of 5 — PutSubscriptionFilter with a Lambda destination is accepted without the logs invoke grant.
AWS: PutSubscriptionFilter whose DestinationArn is a Lambda function is refused with InvalidParameterException unless the function's resource policy grants lambda:InvokeFunction to logs.<region>.amazonaws.com, because CloudWatch Logs verifies the grant at configuration time.
Emulator: the call succeeds with no grant in place.
Why: the subscription appears configured but can never deliver, and the configuration-time check a client relies on to catch the missing permission is gone.
Fix: the destination-permission check in PutSubscriptionFilter.
Traces: cloudwatch__lambda__logs.logs-subscription-filter-missing-permission-negative.

## Evidence
Cause 1: AssertionError: handler reported failure / control invoke must not raise inside the handler; got FunctionError='Unhandled' with payload {"errorMessage":"Unable to import module 'lambda_function': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}.
Cause 2: AssertionError: Invoke response carries no request id (x-amzn-RequestId).
Cause 3: Error: Check timed out after 60.0s on every log-delivery/metric-filter/Insights check.
Cause 4: got ('InvalidParameterException', 'a metric filter can include as many as 3 dimensions, counting the system fields emitted as dimensions'), expected ('InvalidParameterException', 'Number of dimensions exceeds the maximum number of dimensions supported per metric filter'); got ('InvalidParameterException', "each dimension value must name a field of the log event, selected with '$' ..."), expected ('InvalidParameterException', 'Invalid metric transformation: dimension value must be valid selector').
Cause 5: expected PutSubscriptionFilter to be rejected without a logs.amazonaws.com invoke permission grant on the destination Lambda, but it succeeded.

## Steps to Reproduce
1. Invoke a handler that imports boto3 and calls logs.put_log_events / cloudwatch.put_metric_data (cause 1).
2. Invoke any function and read ResponseMetadata.RequestId (cause 2).
3. Invoke a printing function and poll DescribeLogGroups/FilterLogEvents on /aws/lambda/<fn>; add a metric filter and GetMetricStatistics (cause 3).
4. PutMetricFilter with four dimensions, and with a literal dimension value; compare the messages (cause 4).
5. PutSubscriptionFilter with a Lambda DestinationArn and no AddPermission for logs.<region>.amazonaws.com (cause 5).
