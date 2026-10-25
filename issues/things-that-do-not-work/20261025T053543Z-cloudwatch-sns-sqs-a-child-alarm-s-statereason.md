Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.alarm-composite-and-actions-and-rule-waits-for-both-real-children cloudwatch__sns__sqs.cw-alarm-action-to-deleted-topic-is-recorded-as-failed-and-delivers-nothing
Issue-Key: cloudwatch__sns__sqs.alarm-composite-and-actions-and-rule-waits-for-both-real-children|run#22|AssertionError|assert f"[{_BREACH}" in first["StateReason"], (
Issue-Key: cloudwatch__sns__sqs.cw-alarm-action-to-deleted-topic-is-recorded-as-failed-and-delivers-nothing|run#42|AssertionError|assert one_failed["error"] == "Topic does not exist", (
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# cloudwatch__sns__sqs: a child alarm's StateReason omits the datapoints it graded, and a failed alarm action records a Rust SDK debug dump instead of the SNS error (2 causes)

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmHistory

## Description
Two net-new root causes reached only after alarm evaluation and alarm-action dispatch actually run. The open issue 20261025T020208Z-cloudwatch-sns-sqs-cloudwatch-alarm-actions-never-fire-to-sns-so.md reports that alarm actions never fire at all; in this run they DO fire and an Action history item IS recorded (actionState present, notificationResource set), so what diverges now is the content of the evaluated reason and of the recorded failure - neither is covered by that issue.

=== Cause 1 of 2: the child alarm's evaluated StateReason does not cite the datapoint that moved it ===
AWS: Smithy: MetricAlarm.StateReason -> StateReason @length 0..1023 ("An explanation for the alarm state, in text format."); the wording is recorded behavior only. Measured on real AWS, an evaluated reason lists the datapoints the evaluator graded in square brackets, e.g. 'Threshold Crossed: 1 out of the last 1 datapoints [137.0 (dd/mm/yy HH:MM:SS)] was greater than the threshold (50.0) (minimum 1 datapoints for OK -> ALARM transition).' The requirement is that the reason names the value (and its timestamp) that was graded.
Emulator: the reason omits the bracketed datapoint list entirely - 'Threshold Crossed: 1 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (50); 1 datapoints must for the alarm to be in ALARM state.' - after the check published 137.0.
Why: a composite alarm's operator cannot attribute the composite's state to a value on a specific child, because no child reason names the datapoint it graded.
Fix: the alarm evaluator's StateReason renderer in the cloudwatch service crate.
Traces: cloudwatch__sns__sqs.alarm-composite-and-actions-and-rule-waits-for-both-real-children

=== Cause 2 of 2: a failed alarm action records the emulator's internal Rust SDK error debug dump as the history `error` field ===
AWS: Smithy: AlarmHistoryItem.HistoryData -> HistoryData @length 1..4095, "Data about the alarm, in JSON format." Recorded behavior: for an Action history item whose delivery failed because the SNS topic was deleted, the JSON object is {version, actionState: "Failed", notificationResource: <topic arn>, stateValue, error} and `error` is the SNS error text - measured verbatim as 'Topic does not exist'. The rule is that `error` carries the destination service's error message, not a transport dump.
Emulator: actionState="Failed" and notificationResource are right, but `error` is a multi-kilobyte Rust debug rendering of the internal SDK error value, beginning 'service error: NotFoundException: Topic does not exist: NotFoundException: Topic does not exist (ServiceError(ServiceError { source: NotFoundException(NotFoundException { message: Some("Topic does not exist"), meta: ErrorMetadata { code: Some("NotFound"), ... } }), raw: Response { status: StatusCode(404), headers: {}, body: SdkBody { inner: Once(Some(b"<ErrorResponse xmlns=...' - including the raw XML body, HTTP status struct and SDK extension structs.
Why: it leaks emulator internals into a customer-visible field; it is not parseable as AWS's error text; and because HistoryData is capped at 4095 characters it risks truncating or overflowing the history item for every failed action.
Fix: the alarm-action dispatcher's failure recording path - map the destination service's error code/message into the Action HistoryData `error` field instead of formatting the internal SDK error with Debug.
Traces: cloudwatch__sns__sqs.cw-alarm-action-to-deleted-topic-is-recorded-as-failed-and-delivers-nothing

## Evidence
Replayed behavioral suite (simparity-behavioral-ga-prod-v1).

Cause 1 - AssertionError: child a's StateReason does not cite the published datapoint 137.0: 'Threshold Crossed: 1 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (50); 1 datapoints must for the alarm to be in ALARM state.'

Cause 2 - AssertionError: the dispatcher recorded error 'service error: NotFoundException: Topic does not exist: NotFoundException: Topic does not exist (ServiceError(ServiceError { source: NotFoundException(NotFoundException { message: Some("Topic does not exist"), meta: ErrorMetadata { code: Some("NotFound"), message: Some("Topic does not exist"), extras: None } }), raw: Response { status: StatusCode(404), headers: {}, body: SdkBody { inner: Once(Some(b"<ErrorResponse xmlns=\\"http://sns.amazonaws.com/doc/2010-03-31/\\"><Error><Type>Sender</Type><Code>NotFound</Code><Message>Topic does not exist</Message></Error><RequestId>...</RequestId></ErrorResponse>")), retryable: true }, extensions: ... } }))', expected the measured 'Topic does not exist'. Full history item: {'version': '1.0', 'actionState': 'Failed', 'notificationResource': 'arn:aws:sns:us-west-2:<acct>:<topic>', 'stateValue': 'OK', 'error': <the dump above>}

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay the behavioral suite.
2. Cause 1: create two metric alarms (threshold 50) plus a composite with an AND rule over them, PutMetricData 137.0 into one child, wait for it to reach ALARM, then DescribeAlarms and read the child's StateReason.
3. Cause 2: create an SNS topic, put a metric alarm with that topic plus a second live topic as OK/Alarm actions, delete the first topic, drive a state change, then DescribeAlarmHistory(HistoryItemType='Action') and json.loads the HistoryData of the Failed item; inspect the `error` field.
4. Re-replay the two check ids in trace_ids.
