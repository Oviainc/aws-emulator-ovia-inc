Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-alarm-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-alarm-error-paths cwa-metrics-exactly-one-return-data cwa-promql-alarm-evaluation-grammar cwa-range-operator-threshold-metric-id cwa-string-enum-values cloudwatch-log-alarm-error-path cloudwatch-log-alarm-workflow cloudwatch-put-log-alarm-lifecycle cloudwatch-put-log-alarm-missing-log-group cloudwatch-composite-alarm-contributors-and-log-alarm cloudwatch-alarm-history-filtering-and-error-paths cloudwatch-alarm-history-invalid-next-token cloudwatch-metric-alarm-history-and-contributors cloudwatch-metric-alarm-queries cwa-alarm-actions-limit cwp-describe-alarms-prefix-pagination monitoring-metric-alarm-lifecycle monitoring-resource-tagging-workflow
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|PutMetricAlarm|4xx->2xx|$
Issue-Key: monitoring|PutMetricAlarm|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|PutMetricAlarm|4xx->2xx|status
Issue-Key: monitoring|PutLogAlarm|4xx->2xx|$
Issue-Key: monitoring|PutLogAlarm|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|PutLogAlarm|4xx->2xx|status
Issue-Key: monitoring|PutLogAlarm|4xx->4xx|$.message
Issue-Key: monitoring|DescribeAlarmHistory|4xx->4xx|$.message
Issue-Key: monitoring|PutMetricAlarm|4xx->4xx|$.message
Issue-Key: monitoring|PutMetricAlarm|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|DescribeAlarms|2xx->4xx|$.CompositeAlarms
Issue-Key: monitoring|DescribeAlarms|2xx->4xx|$.LogAlarms
Issue-Key: monitoring|DescribeAlarms|2xx->4xx|$.MetricAlarms
Issue-Key: monitoring|SetAlarmState|4xx->4xx|$.__type
Issue-Key: monitoring|SetAlarmState|4xx->4xx|$.message
Issue-Key: monitoring|SetAlarmState|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|ListTagsForResource|4xx->4xx|$.message
Issue-Related: 20261029T203422Z-monitoring-cloudwatch-awsquery-form-encoded-protocol-is-not-served.md
Issue-Related: 20261029T203422Z-monitoring-cloudwatch-error-envelope-omits-x-amzn-query-error.md
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch alarm writes and pagination skip validation or reword refusals

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
8 causes, one shared fault: the alarm write and alarm-read request paths either accept a request AWS refuses, refuse one AWS accepts, or refuse it with emulator-authored text instead of AWS's message for the error class.

Cause 1 of 8: PutMetricAlarm carrying BOTH Statistic and ExtendedStatistic. AWS: 400 with __type 'com.amazon.coral.validate#ValidationException', header 'x-amzn-query-error: ValidationError;Sender' and the fixed message 'PutMetricAlarm request can not be made with both Statistic and ExtendedStatistic parameters. Only one of them should be present' (no variable parts). Smithy: PutMetricAlarm input declares Statistic (enum Statistic) and ExtendedStatistic as separate members; they are mutually exclusive. Emulator: 200 with an empty body — the alarm is CREATED. Why: a caller that mixed the two statistic members gets a silently-created alarm whose statistic is ambiguous, and every downstream evaluation and DescribeAlarms result is built on a configuration AWS would never have stored. Fix: PutMetricAlarm's input validation in the CloudWatch crate. Trace: cloudwatch-alarm-error-paths#task#4.

Cause 2 of 8: PutLogAlarm with an invalid ScheduledQueryConfiguration. AWS: 400 with __type 'com.amazon.coral.validate#ValidationException' and header 'ValidationError;Sender'. Emulator: 200 with an empty body — the log alarm is created. Why: invalid log alarms enter state and are then read back and evaluated, so a client never learns its configuration was rejected. Fix: PutLogAlarm's input validation. Trace: cloudwatch-log-alarm-error-path#task#1.

Cause 3 of 8: PutLogAlarm whose ScheduledQueryConfiguration carries an executionRoleArn that is not an IAM role ARN. AWS: 400 ValidationException with the message "Failed to create Scheduled Query: 1 validation error detected: Value at 'executionRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(?:aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::([0-9]{12}):role\\/(.+)" — fixed text, the scheduled-query prefix plus the standard constraint sentence naming the member and its pattern. Emulator: its own prose 'ScheduledQueryRoleARN is not the ARN of an IAM role: not-a-valid-role-arn', with __type 'ValidationError' and no query-error header. Why: the member name the message blames differs from the request's member, so the caller cannot locate the bad field. Fix: PutLogAlarm's scheduled-query validation and its message rendering. Trace: cloudwatch-composite-alarm-contributors-and-log-alarm#task#3.

Cause 4 of 8: DescribeAlarmHistory with a NextToken the service did not issue. AWS: 400 with the message 'Invalid NextToken : <the submitted token>' — fixed text 'Invalid NextToken : ' plus the token taken from the request. Smithy: DescribeAlarmHistory errors include InvalidNextToken (awsQueryError code InvalidNextToken). Emulator: 'NextToken is not a token this service issued'. Why: a client that extracts the offending token from the message, or matches AWS's text, fails. Fix: DescribeAlarmHistory's pagination-token validation. Trace: cloudwatch-alarm-history-filtering-and-error-paths#task#2.

Cause 5 of 8: PutMetricAlarm with more than five AlarmActions. AWS: 400 ValidationError whose message is "1 validation error detected: Value '[<the submitted ARNs, comma-separated, in request order, inside square brackets>]' at 'alarmActions' failed to satisfy constraint: Member must have length less than or equal to 5" — the fixed constraint sentence, with the offending VALUE rendered as the actual list contents. Smithy: PutMetricAlarm.AlarmActions targets ResourceList @length(min 0, max 5). Emulator: renders the value as the summary "'6 items'" instead of the list, and omits the 'ValidationError;Sender' header. Why: the message no longer tells the caller which actions were submitted, and tooling that parses the echoed value gets a count string where AWS puts data. Fix: the constraint-message value renderer for list members. Trace: cwa-alarm-actions-limit#task#1.

Cause 6 of 8: DescribeAlarms continuing with a NextToken the emulator itself issued on the previous page. AWS: 200 with the remaining page — empty CompositeAlarms, MetricAlarms and LogAlarms lists when the listing is exhausted (all three members present). Smithy: DescribeAlarms is @paginated(inputToken NextToken, outputToken NextToken) with output members CompositeAlarms/MetricAlarms/LogAlarms. Emulator: 400 — it rejects its own continuation token and returns none of the three lists. Why: every paginated DescribeAlarms walk breaks on the last page, so clients enumerating more than one page of alarms either lose alarms or crash. Fix: DescribeAlarms' pagination-token issue/accept round trip. Trace: cwp-describe-alarms-prefix-pagination#task#7.

Cause 7 of 8: SetAlarmState with an out-of-enum StateValue. AWS: 400, __type 'com.amazon.coral.validate#ValidationException', header 'ValidationError;Sender', message "1 validation error detected: Value '<stateValue>' at 'stateValue' failed to satisfy constraint: Member must satisfy enum value set: [INSUFFICIENT_DATA, ALARM, OK]" — the allowed set is rendered in the order the SERVICE renders it (recorded behavior), not in the model's declaration order. Emulator: the same sentence but with the set as '[OK, ALARM, INSUFFICIENT_DATA]', __type 'ValidationError' and no header. Why: assertions and golden-file comparisons on the refusal text diverge, and the fault class is unrecoverable without the query-error header. Fix: the enum-constraint message renderer (allowed-value ordering) plus the shared error envelope. Open issue 20261028T043126Z-cloudwatch-enum-constraint-validationerror-messages-are-rendered-from describes the same ordering defect from the behavioral corpus but carries no wire replay key for this step. Trace: monitoring-metric-alarm-lifecycle#task#9.

Cause 8 of 8: ListTagsForResource with a ResourceARN that is not an ARN. AWS: 400 InvalidParameterValue with the message 'ResourceARN must be valid and belong to a CloudWatch resource: <the submitted ResourceARN>' — fixed text plus the value from the request. Smithy: ListTagsForResource.ResourceARN required, errors include InvalidParameterValueException. Emulator: its own prose 'The value <arn> for parameter ResourceARN is not valid: it is not the ARN of a CloudWatch alarm, dashboard, metric stream or Contributor Insights rule'. Why: the refusal text differs from AWS's for the class, breaking clients that match it. Fix: ListTagsForResource's ARN validation message. Trace: monitoring-resource-tagging-workflow#task#6.

## Evidence
Replay (simparity-wire-aws-v1), 8 clusters.
1. PutMetricAlarm, cloudwatch-alarm-error-paths#task#4: status expected 400, got 200; body empty (expected __type 'com.amazon.coral.validate#ValidationException', message 'PutMetricAlarm request can not be made with both Statistic and ExtendedStatistic parameters. Only one of them should be present'); header 'ValidationError;Sender' missing. 13 affected steps incl. cwa-string-enum-values#task#1..4, cwa-promql-alarm-evaluation-grammar#task#1..4.
2. PutLogAlarm, cloudwatch-log-alarm-error-path#task#1: status expected 400, got 200; body empty; header missing. 4 affected steps.
3. PutLogAlarm, cloudwatch-composite-alarm-contributors-and-log-alarm#task#3: __type expected 'com.amazon.coral.validate#ValidationException', got 'ValidationError'; message expected the "Failed to create Scheduled Query: 1 validation error detected: Value at 'executionRoleArn' …" text, got 'ScheduledQueryRoleARN is not the ARN of an IAM role: not-a-valid-role-arn'; header missing.
4. DescribeAlarmHistory, cloudwatch-alarm-history-filtering-and-error-paths#task#2: message expected 'Invalid NextToken : <token>', got 'NextToken is not a token this service issued'. 4 affected steps.
5. PutMetricAlarm, cwa-alarm-actions-limit#task#1: message expected "…Value '[arn:…topic-0, arn:…topic-5]' at 'alarmActions' failed to satisfy constraint: Member must have length less than or equal to 5", got "…Value '6 items' at 'alarmActions' …"; header missing.
6. DescribeAlarms, cwp-describe-alarms-prefix-pagination#task#7: status expected 200, got 400; $.CompositeAlarms/$.LogAlarms/$.MetricAlarms all missing (expected []).
7. SetAlarmState, monitoring-metric-alarm-lifecycle#task#9: enum set expected '[INSUFFICIENT_DATA, ALARM, OK]', got '[OK, ALARM, INSUFFICIENT_DATA]'; __type 'ValidationError'; header missing.
8. ListTagsForResource, monitoring-resource-tagging-workflow#task#6: message expected 'ResourceARN must be valid and belong to a CloudWatch resource: <arn>', got 'The value <arn> for parameter ResourceARN is not valid: it is not the ARN of a CloudWatch alarm, dashboard, metric stream or Contributor Insights rule'.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutMetricAlarm with both Statistic and ExtendedStatistic set (cloudwatch-alarm-error-paths#task#4) — observe 200 instead of 400.
3. PutLogAlarm with an invalid scheduled-query configuration (cloudwatch-log-alarm-error-path#task#1) and with a non-IAM-role executionRoleArn (cloudwatch-composite-alarm-contributors-and-log-alarm#task#3).
4. DescribeAlarmHistory with NextToken='not-a-real-pagination-token' (cloudwatch-alarm-history-filtering-and-error-paths#task#2).
5. PutMetricAlarm with six AlarmActions (cwa-alarm-actions-limit#task#1) and read the echoed value in $.message.
6. Walk DescribeAlarms with AlarmNamePrefix + MaxRecords until a NextToken is returned, then pass it back (cwp-describe-alarms-prefix-pagination#task#7) — observe 400.
7. SetAlarmState with StateValue='WEIRD_STATE' (monitoring-metric-alarm-lifecycle#task#9) and ListTagsForResource with ResourceARN='not-an-arn' (monitoring-resource-tagging-workflow#task#6).
