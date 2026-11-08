Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__logs.bat-transition-is-processed-under-the-table-name-parsed-from-the-stream-arn dynamodb__lambda__logs.bulk-insert-in-progress-is-never-processed-for-want-of-an-old-image dynamodb__lambda__logs.deleting-a-request-delivers-a-remove-with-only-the-old-image dynamodb__lambda__logs.new-image-only-stream-view-makes-the-bat-transition-skip-too dynamodb__lambda__logs.patched-handler-processes-direct-in-progress-inserts dynamodb__lambda__logs.re-saving-an-in-progress-request-is-skipped-as-no-transition
Issue-Key: dynamodb__lambda__logs.bat-transition-is-processed-under-the-table-name-parsed-from-the-stream-arn|run#7|AssertionError|assert first["lines"] == [
Issue-Key: dynamodb__lambda__logs.bulk-insert-in-progress-is-never-processed-for-want-of-an-old-image|run#13|AssertionError|assert inv["lines"] == [
Issue-Key: dynamodb__lambda__logs.deleting-a-request-delivers-a-remove-with-only-the-old-image|run#15|AssertionError|assert inv["lines"] == [
Issue-Key: dynamodb__lambda__logs.new-image-only-stream-view-makes-the-bat-transition-skip-too|run#18|AssertionError|assert inv["lines"] == [
Issue-Key: dynamodb__lambda__logs.patched-handler-processes-direct-in-progress-inserts|run#19|AssertionError|assert inv["lines"] == [
Issue-Key: dynamodb__lambda__logs.re-saving-an-in-progress-request-is-skipped-as-no-transition|run#17|AssertionError|assert resave["lines"] == [
Issue-Commit: db4c6069cd883a188788d98b6996561329db7d32

# dynamodb__lambda__logs: every stream-skip invocation logs an extra 'not a value an AWS SDK call can carry' line, because the in-function SDK refuses a datetime for PutMetricData's Timestamp

> _Note: this issue was identified against the code base at commit `db4c6069cd883a188788d98b6996561329db7d32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
One net-new root cause, the same defect seen under the cloudwatch__dynamodb__lambda heading but reported here as a log-content divergence. Open issue 20261106T020636Z-dynamodb-lambda-logs-the-cloudformation-create-of-the.md owns the e2e lifecycle check in this service; the suite marked none of the six checks below as owned by it.

=== Cause 1 of 1: a datetime passed for MetricDatum.Timestamp is rejected by the in-function SDK, adding a third log line to every skip ===
AWS: CloudWatch MetricDatum.Timestamp is a modeled timestamp member (Smithy: com.amazonaws.cloudwatch#MetricDatum$Timestamp -> Timestamp). An AWS SDK accepts a native datetime there, so a handler that publishes a skip counter with cloudwatch.put_metric_data(..., MetricData=[{'MetricName': 'SkippedRecords', 'Value': n, 'Unit': 'Count', 'Timestamp': datetime.utcnow()}]) inside try/except dispatches the call, it succeeds, and the except-clause never runs. The invocation's CloudWatch Logs stream therefore holds exactly the handler's own two lines for a skipped record: the per-record skip reason ('Skipping record - no OldImage present (event type: <INSERT|MODIFY>)' or 'Skipping record - not a state change to IN_PROGRESS (old: <old status>, new: <new status>)' — fixed text with the event type and the two status values taken from the stream record) followed by the summary 'Processed <n> records, skipped <m> records' with the counts from that batch. No third line.
Emulator: the in-process Lambda Python runtime's bundled SDK raises on the datetime before the request is dispatched, so the except-clause fires and the runtime appends a third line to every skip invocation: 'Failed to publish skipped records metric: datetime.datetime(<naive datetime>) is not a value an AWS SDK call can carry'. The two expected lines are correct; the extra line appears on every DynamoDB-stream skip path — the BAT BUFFER INSERT, a bulk INSERT with no OldImage, a REMOVE carrying only the old image, a NEW_IMAGE-only MODIFY, a direct IN_PROGRESS insert and a no-transition re-save — and no SkippedRecords datapoint is published.
Why: the log is the contract here. Anything that reads a function's log (a log-based assertion, a metric filter, a subscription filter, an operator diagnosing a skip) sees an error line AWS never emits, and is told the metric publish failed when in AWS it succeeded. The in-function metric itself is silently lost.
Fix: the in-process Lambda Python runtime's bundled AWS SDK — its request-parameter serializer must accept a datetime (naive and tz-aware) for any member the model declares as a timestamp and convert it to the protocol's timestamp form. Not the CloudWatch handler and not the stream/ESM path: both the skip logic and the record envelope are already correct in these runs.
Traces: dynamodb__lambda__logs.bat-transition-is-processed-under-the-table-name-parsed-from-the-stream-arn, .bulk-insert-in-progress-is-never-processed-for-want-of-an-old-image, .deleting-a-request-delivers-a-remove-with-only-the-old-image, .new-image-only-stream-view-makes-the-bat-transition-skip-too, .patched-handler-processes-direct-in-progress-inserts, .re-saving-an-in-progress-request-is-skipped-as-no-transition.

## Evidence
All six checks fail on the same assertion shape — the invocation's log lines must equal exactly the handler's two lines, and a third line is present:

- bat-transition-is-processed-under-the-table-name-parsed-from-the-stream-arn: logged ['Skipping record - no OldImage present (event type: INSERT)', 'Processed 0 records, skipped 1 records', 'Failed to publish skipped records metric: datetime.datetime(2026, 11, 8, 20, 13, 55, 175365) is not a value an AWS SDK call can carry']; expected only the first two.
- bulk-insert-in-progress-is-never-processed-for-want-of-an-old-image: same two lines plus 'Failed to publish skipped records metric: datetime.datetime(...) is not a value an AWS SDK call can carry'.
- deleting-a-request-delivers-a-remove-with-only-the-old-image: ['Skipping record - not a state change to IN_PROGRESS (old: IN_PROGRESS, new: )', 'Processed 0 records, skipped 1 records', 'Failed to publish skipped records metric: datetime.datetime(...) ...'].
- new-image-only-stream-view-makes-the-bat-transition-skip-too: ['Skipping record - no OldImage present (event type: MODIFY)', 'Processed 0 records, skipped 1 records', 'Failed to publish ...'].
- patched-handler-processes-direct-in-progress-inserts: ['Skipping record - not a state change to IN_PROGRESS (old: , new: BUFFER)', 'Processed 0 records, skipped 1 records', 'Failed to publish ...'].
- re-saving-an-in-progress-request-is-skipped-as-no-transition: ['Skipping record - not a state change to IN_PROGRESS (old: IN_PROGRESS, new: IN_PROGRESS)', 'Processed 0 records, skipped 1 records', 'Failed to publish ...'].

The datetime value inside the message is per-run and is not part of the requirement; the requirement is that the line does not exist at all, because put_metric_data with a datetime Timestamp succeeds. The publishing call in the handler is cloudwatch.put_metric_data(Namespace=os.environ['ONYX_METRIC_NAMESPACE'], MetricData=[{'MetricName': 'SkippedRecords', 'Value': skipped_count, 'Unit': 'Count', 'Timestamp': datetime.utcnow()}]) wrapped in try/except logging logger.warning(f'Failed to publish skipped records metric: {e}').

## Steps to Reproduce
1. Deploy the Onyx shape: a DynamoDB table with a stream, an ESM to a python3.12 function whose handler skips records with no OldImage or no IN_PROGRESS transition, logs the skip reason and a 'Processed n records, skipped m records' summary, and then publishes a SkippedRecords datapoint via boto3 cloudwatch.put_metric_data with 'Timestamp': datetime.utcnow(); grant the role cloudwatch:PutMetricData.
2. Write an item that produces a skipped record (a BUFFER/bulk INSERT with no OldImage, a delete, a NEW_IMAGE-only MODIFY, or a re-save with no status change).
3. Read the invocation's CloudWatch Logs lines.
4. Observe the third line 'Failed to publish skipped records metric: datetime.datetime(...) is not a value an AWS SDK call can carry'; on AWS only the skip-reason and summary lines are present and the datapoint is published.
Then replay the checks named in trace_ids.
