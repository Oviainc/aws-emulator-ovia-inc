Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__s3.s3-tagfilter-alarm-children-filter-rejected-alongside-alarm-types cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-a-period-of-forty-five-seconds cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-an-extendedstatistic-above-p100 cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-duplicate-dimension-names cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-thirty-one-dimensions cloudwatch__s3.s3-tagfilter-alarm-set-state-is-recorded-in-alarm-history cloudwatch__s3.s3-tagfilter-alarm-e2e-check-lifecycle cloudwatch__s3.s3-tagfilter-alarm-on-an-undeclared-filter-id-never-gets-data cloudwatch__s3.s3-tagfilter-deleted-metrics-configuration-stops-its-filter-series
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-children-filter-rejected-alongside-alarm-types|run#8|AssertionError|assert msg == (
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-a-period-of-forty-five-seconds|run#9|AssertionError|assert msg == 'Period must be 10, 20, 30 or a multiple of 60', f"the rejection must say why; got {msg!r}"
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-an-extendedstatistic-above-p100|_error#7|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-duplicate-dimension-names|_error#6|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-thirty-one-dimensions|run#10|AssertionError|assert error == ("ValidationError", expected_message), (
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-set-state-is-recorded-in-alarm-history|run#16|AssertionError|assert [h["HistorySummary"] for h in created] == [f'Alarm "{name}" created'], (
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-e2e-check-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudwatch__s3.s3-tagfilter-alarm-on-an-undeclared-filter-id-never-gets-data|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__s3.s3-tagfilter-deleted-metrics-configuration-stops-its-filter-series|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: b8358f56f4f44e240ca755ea7919bfa62f43cb3d

# cloudwatch__s3: alarms on S3 request-metric series — dimension/percentile/period validation missing or misworded, alarm-history summary rewritten, CFN alarm absent, and no AWS/S3 FilterId series is ever published (8 causes)

> _Note: this issue was identified against the code base at commit `b8358f56f4f44e240ca755ea7919bfa62f43cb3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Eight independent root causes observed while alarming on S3 request metrics (AWS/S3, BucketName+FilterId). Each must be closed separately. Related open issues that do NOT carry these replay keys and so do not cover these checks: 20261025T062946Z-cloudwatch-s3-s3-request-metrics-configurations-publish-no.md (cause 8), 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md (cause 7), 20261102T112036Z-monitoring-cloudwatch-alarm-history-data-and-creationid-are-emulator.md (cause 6), 20261029T182452Z-cloudwatch-request-validation-and-error-envelope-divergences-10.md.

Cause 1 of 8: the DescribeAlarms relationship-filter exclusivity refusal is emulator-authored.
AWS: Smithy: DescribeAlarms takes ChildrenOfAlarmName and ParentsOfAlarmName alongside AlarmNames, AlarmNamePrefix, AlarmTypes, StateValue, ActionPrefix, MaxRecords, NextToken; the exclusivity is a cross-parameter service rule the model cannot express. Recorded behavior: combining a relationship filter with any selector other than MaxRecords/NextToken is refused HTTP 400 `ValidationError` with `<FilterName> requests cannot be used with the following parameters: ActionPrefix, AlarmNamePrefix, AlarmNames, AlarmTypes, StateValue` — fixed text apart from `<FilterName>`, which is the relationship parameter actually used (`ChildrenOfAlarmName` or `ParentsOfAlarmName`); the excluded-parameter list is fixed and alphabetical.
Emulator: the code is `ValidationError` but the message is `ChildrenOfAlarmName cannot be combined with any parameter other than MaxRecords and NextToken` — it names the allowed parameters instead of enumerating the excluded ones in AWS's words.
Why: callers that surface or match the refusal text get prose AWS never emits and lose the explicit list of the parameters they must drop.
Fix: the relationship-filter exclusivity rejection message in DescribeAlarms' handler.
Traces: cloudwatch__s3.s3-tagfilter-alarm-children-filter-rejected-alongside-alarm-types.

Cause 2 of 8: the Period refusal message carries an emulator-authored suffix.
AWS: Smithy: PutMetricAlarm.Period is an integer with @range min 1; the 10/20/30/multiple-of-60 grammar is a service rule. Recorded behavior: a Period outside the grammar is refused HTTP 400 `ValidationError` with the message exactly `Period must be 10, 20, 30 or a multiple of 60` — entirely fixed text, carrying no request value.
Emulator: `Period must be 10, 20, 30 or a multiple of 60; got 45`, i.e. AWS's text plus a `; got <period>` suffix taken from the request.
Why: the refusal text becomes per-request instead of constant, so anything matching AWS's wording fails.
Fix: the Period rejection message in PutMetricAlarm's validation.
Traces: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-a-period-of-forty-five-seconds.

Cause 3 of 8: PutMetricAlarm accepts an ExtendedStatistic above p100.
AWS: Smithy: PutMetricAlarm.ExtendedStatistic is a string; the percentile grammar is a service rule. Recorded behavior: a percentile above 100 is refused HTTP 400 `InvalidParameterValue` with `The value <value> for parameter ExtendedStatistic is not supported.` (fixed text apart from `<value>` from the request) and nothing is created, while `p100` itself and a fractional in-range percentile such as `p99.9` are accepted and stored verbatim — the cliff is strictly above 100.
Emulator: `ExtendedStatistic='p101'` succeeds (no error at all) and the alarm is created.
Why: a typo'd percentile yields a live alarm on a statistic CloudWatch cannot compute instead of the documented refusal, and the p100/p101 boundary is unobservable.
Fix: PutMetricAlarm's validation (percentile grammar parse).
Traces: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-an-extendedstatistic-above-p100.

Cause 4 of 8: PutMetricAlarm accepts duplicate dimension names.
AWS: Smithy: PutMetricAlarm.Dimensions targets Dimensions (@length 0..30); dimension-name uniqueness is a service rule. Recorded behavior: two dimensions with the same Name in one alarm are refused HTTP 400 `ValidationError` with `Dimension names must be unique (duplicate: <Name>)` — fixed text apart from `<Name>`, the duplicated dimension name from the request — and nothing is created.
Emulator: an alarm naming `FilterId` twice (values ImportantTag and ImportantPrefix) plus BucketName is accepted with no error, so one of the two values is silently kept.
Why: the alarm is then bound to a metric identity the caller never specified (first-or-last wins, invisibly), and the caller loses the refusal that tells it which name it duplicated.
Fix: PutMetricAlarm's validation (dimension-list uniqueness check).
Traces: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-duplicate-dimension-names.

Cause 5 of 8: the over-30-dimensions refusal uses the wrong error code and message.
AWS: Smithy: PutMetricAlarm.Dimensions has @length max 30. Recorded behavior: 31 dimensions are refused HTTP 400 `ValidationError` with `1 validation error detected: Value '[Dimension(name=<n1>, value=<v1>), Dimension(name=<n2>, value=<v2>), ...]' at 'dimensions' failed to satisfy constraint: Member must have length less than or equal to 30` — the frame and the `Dimension(name=…, value=…)` rendering are fixed text, the member list is the request's own dimensions in request order — and nothing is created; exactly 30 is accepted.
Emulator: `('InvalidParameterValue', 'The collection Dimensions must not have a size greater than 30.')` — wrong error code and a message that neither uses AWS's constraint-violation frame nor echoes the offending list.
Why: a client branching on `ValidationError` vs `InvalidParameterValue` takes the wrong path, and the message does not identify which dimensions were sent.
Fix: the collection-length constraint renderer used by PutMetricAlarm's validation (error classification and message text).
Traces: cloudwatch__s3.s3-tagfilter-alarm-put-rejected-for-thirty-one-dimensions.

Cause 6 of 8: the alarm-creation history item's HistorySummary omits the alarm name.
AWS: Smithy: DescribeAlarmHistory filters by HistoryItemType and returns AlarmHistoryItems carrying HistorySummary and HistoryData. Recorded behavior: the ConfigurationUpdate item written when an alarm is created has HistorySummary exactly `Alarm "<AlarmName>" created` — fixed text apart from `<AlarmName>`, the alarm's own name, in double quotes.
Emulator: the HistorySummary is `Alarm created`, with the quoted alarm name dropped (the item is otherwise correctly typed ConfigurationUpdate and carries a Create HistoryData document).
Why: alarm history is a user-facing audit trail; a summary that does not name the alarm cannot be read or matched the way AWS's can, and consumers that group or filter history by that text get nothing.
Fix: the alarm-history summary text written on create, in the alarm-history component of the CloudWatch state (shared by PutMetricAlarm/PutCompositeAlarm).
Traces: cloudwatch__s3.s3-tagfilter-alarm-set-state-is-recorded-in-alarm-history.

Cause 7 of 8: an alarm declared in a deployed stack does not exist after deploy.
AWS: recorded behavior only: a stack declaring an AWS::CloudWatch::Alarm that reaches CREATE_COMPLETE has created that alarm, and DescribeAlarms returns it.
Emulator: after the three-arm bucket estate deploys, the decoy alarm the template declares is absent — the deploy phase fails with 'the decoy alarm must exist after deploy; got []'.
Why: the stack's reported success does not imply its alarm exists, so no deploy-then-observe trajectory on a CloudFormation-managed alarm can run.
Fix: the CloudFormation provisioning path for AWS::CloudWatch::Alarm — create the alarm in the CloudWatch store before reporting the resource created.
Traces: cloudwatch__s3.s3-tagfilter-alarm-e2e-check-lifecycle.

Cause 8 of 8: S3 publishes no AWS/S3 request-metric series for a bucket metrics configuration.
AWS: recorded behavior only: once a bucket metrics configuration exists (PutBucketMetricsConfiguration, with or without a Prefix/Tag filter), S3 publishes AWS/S3 request metrics such as AllRequests under the dimension set `BucketName`+`FilterId` (FilterId being the configuration's Id), and only under ids a configuration declares; deleting the configuration stops its series while a sibling configuration on the same traffic keeps reporting. Datapoints begin appearing roughly 15 minutes after the configuration is added.
Emulator: with the configurations in place and traffic flowing, neither the declared-id series nor the sibling series ever receives a datapoint, and the alarms on them never leave INSUFFICIENT_DATA; both checks exhaust the harness ceiling (60 s) while still polling for the first datapoint.
Why: no S3 request-metric observability works — a client cannot tell a dead alarm on an undeclared FilterId from a live alarm with no traffic, and cannot observe that deleting a metrics configuration stops its series.
Fix: S3's request-metric emission for bucket metrics configurations (publish AWS/S3 datapoints under BucketName+FilterId for declared ids only, and stop on delete).
Traces: cloudwatch__s3.s3-tagfilter-alarm-on-an-undeclared-filter-id-never-gets-data, cloudwatch__s3.s3-tagfilter-deleted-metrics-configuration-stops-its-filter-series.

## Evidence
Replayed against the emulator (suite service heading `### cloudwatch__s3`):

1. s3-tagfilter-alarm-children-filter-rejected-alongside-alarm-types — describe_alarms(ChildrenOfAlarmName=parent, AlarmTypes=['MetricAlarm']) expected message 'ChildrenOfAlarmName requests cannot be used with the following parameters: ActionPrefix, AlarmNamePrefix, AlarmNames, AlarmTypes, StateValue'; actual 'ChildrenOfAlarmName cannot be combined with any parameter other than MaxRecords and NextToken'.
2. s3-tagfilter-alarm-put-rejected-for-a-period-of-forty-five-seconds — expected 'Period must be 10, 20, 30 or a multiple of 60'; actual 'Period must be 10, 20, 30 or a multiple of 60; got 45'.
3. s3-tagfilter-alarm-put-rejected-for-an-extendedstatistic-above-p100 — ExtendedStatistic='p101' expected ('InvalidParameterValue', 'The value p101 for parameter ExtendedStatistic is not supported.'); actual 'expected a rejection, but it succeeded: None'.
4. s3-tagfilter-alarm-put-rejected-for-duplicate-dimension-names — Dimensions=[{FilterId: ImportantTag}, {FilterId: ImportantPrefix}, {BucketName: ...}] expected ('ValidationError', 'Dimension names must be unique (duplicate: FilterId)'); actual 'expected a rejection, but it succeeded: None'.
5. s3-tagfilter-alarm-put-rejected-for-thirty-one-dimensions — expected ('ValidationError', "1 validation error detected: Value '[Dimension(name=BucketName, value=...), Dimension(name=FilterId, value=ImportantTag), Dimension(name=Extra0, value=v), ...]' at 'dimensions' failed to satisfy constraint: Member must have length less than or equal to 30"); actual ('InvalidParameterValue', 'The collection Dimensions must not have a size greater than 30.').
6. s3-tagfilter-alarm-set-state-is-recorded-in-alarm-history — describe_alarm_history(HistoryItemType='ConfigurationUpdate') expected HistorySummary 'Alarm "<alarm-name>" created'; actual HistorySummary 'Alarm created' (HistoryItemType ConfigurationUpdate, HistoryData a {"version":"1.0","type":"Create",...} document).
7. s3-tagfilter-alarm-e2e-check-lifecycle — '[phase 1/4 deploy] the decoy alarm must exist after deploy; got []'.
8. s3-tagfilter-alarm-on-an-undeclared-filter-id-never-gets-data and s3-tagfilter-deleted-metrics-configuration-stops-its-filter-series — both 'Check timed out after 60.0s' while polling (own deadline 1500 s, interval 60 s) for the FIRST AllRequests datapoint under BucketName+FilterId; the declared-id alarm never reaches ALARM and neither FilterId series ever reports a value.

## Steps to Reproduce
1. Create a bucket and install three metrics configurations: EntireBucket (no filter), ImportantPrefix (Filter={'Prefix': 'important/'}), ImportantTag (Filter={'Tag': {'Key':'priority','Value':'high'}}).
2. Cause 1: put a metric alarm on AWS/S3 AllRequests with Dimensions BucketName+FilterId=ImportantTag, put a composite alarm over it, then cloudwatch.describe_alarms(ChildrenOfAlarmName=<composite>, AlarmTypes=['MetricAlarm']) and compare the ValidationError message; repeat with ParentsOfAlarmName=<child>, AlarmNamePrefix='sim2real-'.
3. Cause 2: put_metric_alarm on the same series with Period=45 and compare the message.
4. Cause 3: put_metric_alarm with ExtendedStatistic='p101' — expect 400 InvalidParameterValue, observe success; then with 'p100' and 'p99.9' to see the boundary.
5. Cause 4: put_metric_alarm with Dimensions=[{'Name':'FilterId','Value':'ImportantTag'},{'Name':'FilterId','Value':'ImportantPrefix'},{'Name':'BucketName','Value':<bucket>}] — expect 400 ValidationError, observe success.
6. Cause 5: put_metric_alarm with 31 dimensions — compare the code and message; then 30 to confirm the boundary.
7. Cause 6: put_metric_alarm, then cloudwatch.describe_alarm_history(AlarmName=<alarm>, HistoryItemType='ConfigurationUpdate') and read HistorySummary.
8. Cause 7: deploy the three-arm bucket + decoy-alarm template, wait for CREATE_COMPLETE, then describe_alarms for the decoy alarm name.
9. Cause 8: with the configurations installed, drive GET/PUT traffic under 'important/' and 'other/' and poll cloudwatch.get_metric_statistics(Namespace='AWS/S3', MetricName='AllRequests', Dimensions=[{'Name':'BucketName','Value':<bucket>},{'Name':'FilterId','Value':'ImportantPrefix'}]) — expect datapoints, observe an empty series indefinitely; delete the configuration and confirm the sibling id keeps reporting.
