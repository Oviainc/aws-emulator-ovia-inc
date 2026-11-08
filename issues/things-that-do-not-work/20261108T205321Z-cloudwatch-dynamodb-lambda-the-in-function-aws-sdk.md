Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data cloudwatch__dynamodb__lambda.skipped-records-metric-sums-one-per-skipped-insert cloudwatch__dynamodb__lambda.skipped-records-alarm-fires-on-six-skips-in-one-period cloudwatch__dynamodb__lambda.skipped-records-alarm-holds-ok-at-exactly-five-skips
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data|run#8|AssertionError|assert "AccessDenied" in warn[0] and "cloudwatch:PutMetricData" in warn[0], warn[0]
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-metric-sums-one-per-skipped-insert|_await_sum#11|Timeout|time.sleep(2)
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-alarm-fires-on-six-skips-in-one-period|_await_sum#14|Timeout|time.sleep(2)
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-alarm-holds-ok-at-exactly-five-skips|_await_sum#10|Timeout|time.sleep(2)
Issue-Commit: db4c6069cd883a188788d98b6996561329db7d32

# cloudwatch__dynamodb__lambda: the in-function AWS SDK refuses a Python datetime for a modeled timestamp member, so PutMetricData never publishes and the SkippedRecords metric/alarm flow never completes

> _Note: this issue was identified against the code base at commit `db4c6069cd883a188788d98b6996561329db7d32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
One net-new root cause. Open issue 20261105T111951Z-cloudwatch-dynamodb-lambda-the-lambda-python-runtime-has.md covers the neighbouring 'runtime has no AWS SDK' failure and 20261107T212447Z-cloudwatch-lambda-the-lambda-python-runtime-s-bundled-botocore-knows.md the missing 'cloudwatch' client, but the suite marked none of the checks below as owned by them: here boto3 IS importable and the cloudwatch client IS constructed, and the call fails at parameter marshalling instead.

=== Cause 1 of 1: a datetime passed for MetricDatum.Timestamp is rejected by the in-function SDK ===
AWS: CloudWatch MetricDatum.Timestamp is a modeled timestamp member (Smithy: com.amazonaws.cloudwatch#MetricDatum$Timestamp -> Timestamp, 'the time the metric data was received'). Every AWS SDK accepts a native datetime for a timestamp member — a boto3 handler calling cloudwatch.put_metric_data(Namespace=ns, MetricData=[{'MetricName': 'SkippedRecords', 'Value': n, 'Unit': 'Count', 'Timestamp': datetime.utcnow()}]) serializes it and the call is dispatched, so with cloudwatch:PutMetricData allowed the datapoint lands (Sum over the period equals the number of publishes), and with the permission missing the call is refused AccessDenied naming the cloudwatch:PutMetricData action. Either way the handler's own log shows the dispatch outcome, not a client-side type refusal.
Emulator: the call never leaves the function. The in-process Lambda Python runtime's bundled SDK raises on the datetime before dispatch, and the handler's except-clause logs 'Failed to publish skipped records metric: datetime.datetime(<naive datetime>) is not a value an AWS SDK call can carry'. That text is the emulator's own SDK shim refusing a legal value for a modeled timestamp member; no request reaches CloudWatch. Consequently no SkippedRecords datapoint is ever published: GetMetricStatistics returns an empty Datapoints list forever, the stack's SkippedRecords alarm (Sum, Period 300, EvaluationPeriods 1, GreaterThanThreshold) never leaves INSUFFICIENT_DATA, and in the negative case the refusal the handler reports is this marshalling error instead of an AccessDenied naming cloudwatch:PutMetricData.
Why: any handler that stamps its own metric timestamp — the ordinary pattern for back-dating a datapoint into the period it belongs to — cannot publish at all here, and the failure is a client-side TypeError-class message rather than the service's answer. A metric/alarm pipeline that works in production is unobservable in the emulator, and an authorization defect in the same path is masked by the marshalling refusal, so a caller cannot tell a permission problem from a serialization problem.
Fix: the in-process Lambda Python runtime's bundled AWS SDK — its request-parameter serializer must accept a datetime (naive and tz-aware) wherever the model declares a timestamp member and convert it to the protocol's timestamp form. This is not the CloudWatch PutMetricData handler: the handler is never reached.
Traces: cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data (the marshalling message is asserted directly), and the three checks that time out at 180 s waiting for the datapoint/alarm transition that publish never produces: skipped-records-metric-sums-one-per-skipped-insert, skipped-records-alarm-fires-on-six-skips-in-one-period, skipped-records-alarm-holds-ok-at-exactly-five-skips.

## Evidence
cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data: AssertionError at 'assert "AccessDenied" in warn[0] and "cloudwatch:PutMetricData" in warn[0]' — the handler's single warning line was 'Failed to publish skipped records metric: datetime.datetime(2026, 11, 8, 20, 12, 39, 999200) is not a value an AWS SDK call can carry' (the datetime value is per-run; the invariant is the refusal of a datetime for MetricDatum.Timestamp and the absence of any AccessDenied / cloudwatch:PutMetricData text).

The handler under test publishes exactly: cloudwatch.put_metric_data(Namespace=os.environ['ONYX_METRIC_NAMESPACE'], MetricData=[{'MetricName': 'SkippedRecords', 'Value': skipped_count, 'Unit': 'Count', 'Timestamp': datetime.utcnow()}]) inside try/except, logging logger.warning(f'Failed to publish skipped records metric: {e}').

cloudwatch__dynamodb__lambda.skipped-records-metric-sums-one-per-skipped-insert, .skipped-records-alarm-fires-on-six-skips-in-one-period, .skipped-records-alarm-holds-ok-at-exactly-five-skips: 'Check timed out after 180.0s' — each polls GetMetricStatistics for Namespace=<stack namespace>, MetricName='SkippedRecords' (Sum/SampleCount) and then DescribeAlarms/DescribeAlarmHistory for the ALARM transition; no datapoint ever appears because the publish above never dispatches.

## Steps to Reproduce
1. Deploy the Onyx stack shape: a DynamoDB table with a stream, a python3.12 function whose handler skips records lacking an OldImage and then publishes MetricDatum {'MetricName': 'SkippedRecords', 'Value': <count>, 'Unit': 'Count', 'Timestamp': datetime.utcnow()} via boto3 cloudwatch.put_metric_data, with an execution role granting cloudwatch:PutMetricData.
2. Write one item that produces a skipped record (a bulk INSERT with no OldImage) and read the invocation's log lines.
3. Observe the third line 'Failed to publish skipped records metric: datetime.datetime(...) is not a value an AWS SDK call can carry' instead of a silent, successful publish.
4. cloudwatch.get_metric_statistics(Namespace=<ns>, MetricName='SkippedRecords', Period=60, Statistics=['Sum','SampleCount']) over the surrounding window — observe an empty Datapoints list.
5. put_metric_alarm(Namespace=<ns>, MetricName='SkippedRecords', Statistic='Sum', Period=300, EvaluationPeriods=1, Threshold=5, ComparisonOperator='GreaterThanThreshold'), force six skipped INSERTs, and observe the alarm stays INSUFFICIENT_DATA.
6. Re-run step 2 with the role NOT granting cloudwatch:PutMetricData and observe the handler reports the same marshalling refusal rather than AccessDenied naming cloudwatch:PutMetricData.
Then replay the checks named in trace_ids.
