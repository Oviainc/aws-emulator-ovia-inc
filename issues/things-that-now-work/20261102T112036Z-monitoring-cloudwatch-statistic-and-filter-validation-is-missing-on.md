Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-alarm-firing-percentile-query-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-alarm-firing-percentile-query-mistakes cloudwatch-latency-metric-statistics-mistakes cloudwatch-latency-metric-data-stat-and-discovery-mistakes cloudwatch-s3tfa-traj-datapoints-of-an-idle-tag-filter-metric cloudwatch-mstx-traj-rebuild-exclude-stream-refusals cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes cloudwatch-s3tfa-traj-list-metrics-is-empty-for-an-idle-tag-filter
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetMetricStatistics|4xx->2xx|$.__type
Issue-Key: monitoring|GetMetricStatistics|4xx->2xx|$.message
Issue-Key: monitoring|GetMetricStatistics|4xx->2xx|status
Issue-Key: monitoring|GetMetricData|4xx->2xx|$.__type
Issue-Key: monitoring|GetMetricData|4xx->2xx|$.message
Issue-Key: monitoring|GetMetricData|4xx->2xx|status
Issue-Key: monitoring|PutMetricStream|4xx->2xx|$.__type
Issue-Key: monitoring|PutMetricStream|4xx->2xx|$.message
Issue-Key: monitoring|PutMetricStream|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|PutMetricStream|4xx->2xx|status
Issue-Key: monitoring|DescribeAlarmsForMetric|4xx->2xx|$.__type
Issue-Key: monitoring|DescribeAlarmsForMetric|4xx->2xx|$.message
Issue-Key: monitoring|DescribeAlarmsForMetric|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|DescribeAlarmsForMetric|4xx->2xx|status
Issue-Key: monitoring|GetMetricData|2xx->4xx|$.Messages
Issue-Key: monitoring|GetMetricData|2xx->4xx|$.MetricDataResults
Issue-Key: monitoring|GetMetricData|2xx->4xx|$.NextToken
Issue-Key: monitoring|GetMetricData|2xx->4xx|status
Issue-Key: monitoring|ListMetrics|2xx->4xx|$.Metrics
Issue-Key: monitoring|ListMetrics|2xx->4xx|status
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# CloudWatch statistic and filter validation is missing on metric reads

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
This issue bundles SIX causes sharing one fault: the emulator's accept/refuse decision on CloudWatch metric-read and metric-stream input is wrong in BOTH directions - four requests AWS refuses with 400 are answered 200, and two requests AWS answers 200 are refused with 400. The HTTP STATUS diverges in every cause, which makes this the highest-impact family in the sweep. There are 6 causes to close.

Cause 1 of 6: GetMetricStatistics accepts an unsupported ExtendedStatistics value.
  AWS: Smithy: GetMetricStatistics.ExtendedStatistics is a list (length 1..10) and the operation models InvalidParameterValueException (awsQueryError InvalidParameterValue, httpError 400). The percentile form is case-sensitive and must be lower-case 'p' (p0.0..p100), so an upper-case 'P90' is unsupported. Recorded: 400, __type com.amazon.coral.service#InvalidParameterValueException, message "The value P90 for parameter ExtendedStatistics.member.1.<list element> is not supported." - fixed text "The value " + the rejected value + " for parameter " + the awsQuery member path + " is not supported.".
  Emulator: 200 with no error body at all - the request is accepted.
  Why: a client that mis-cased its percentile gets a successful, silently empty/partial result instead of a refusal, so a dashboard or alarm-sizing script reports "no data" when the real problem is a malformed statistic.
  Fix: GetMetricStatistics input validation - validate each ExtendedStatistics element against the percentile grammar.
  Traces: cloudwatch-alarm-firing-percentile-query-mistakes#task#3 (plus cloudwatch-latency-metric-statistics-mistakes#task#2, cloudwatch-latency-metric-statistics-mistakes#task#7).

Cause 2 of 6: GetMetricData accepts an invalid MetricStat.Stat.
  AWS: Recorded 400, __type com.amazon.coral.validate#ValidationException, message "The value for parameter MetricDataQueries.member.2.MetricStat.Stat is not a valid statistic.\n" - fixed text "The value for parameter " + the awsQuery member path + " is not a valid statistic." plus a trailing newline; the index comes from the request.
  Emulator: 200 with no error body - accepted.
  Why: as above, but worse for GetMetricData, where one bad query in a batch silently yields no data for that id while the response still looks successful.
  Fix: GetMetricData input validation - validate MetricStat.Stat for every element of MetricDataQueries. (Note: when the emulator DOES refuse an invalid Stat it uses the wrong exception type - see the separate issue "CloudWatch metric-read parameter refusals use the wrong exception type" - so both the decision and the shape need work.)
  Traces: cloudwatch-latency-metric-data-stat-and-discovery-mistakes#task#1 (plus cloudwatch-s3tfa-traj-datapoints-of-an-idle-tag-filter-metric#task#1).

Cause 3 of 6: PutMetricStream accepts an ExcludeFilters entry with a null Namespace.
  AWS: Smithy: PutMetricStream.ExcludeFilters is a list of MetricStreamFilter; its Namespace must be present. Recorded 400, __type com.amazon.coral.service#InvalidParameterValueException, x-amzn-query-error InvalidParameterValue;Sender, message "Value null at 'ExcludeFilters.0.member.Namespace' failed to satisfy constraint: Member must not be null" - fixed text "Value null at '", "' failed to satisfy constraint: Member must not be null", with the zero-based member path from the request.
  Emulator: 200 with no error body - the stream is created with a meaningless filter.
  Why: the stream is persisted with an unusable exclude filter, so subsequent GetMetricStream/ListMetricStreams reads describe a configuration AWS would never have accepted, and the client never learns its filter was malformed.
  Fix: PutMetricStream input validation of IncludeFilters/ExcludeFilters members.
  Traces: cloudwatch-mstx-traj-rebuild-exclude-stream-refusals#task#3.

Cause 4 of 6: DescribeAlarmsForMetric accepts both Statistic and ExtendedStatistic together.
  AWS: Smithy: DescribeAlarmsForMetric has both Statistic (enum Statistic) and ExtendedStatistic, and they are mutually exclusive. Recorded 400, __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender, message "DescribeAlarmsForMetric request can not be made with both Statistic and ExtendedStatistic parameters. No more than one of them should be present" - entirely fixed text apart from the operation name.
  Emulator: 200 with no error body - both are accepted.
  Why: the emulator must pick one silently, so the client gets alarms matched on a statistic it did not unambiguously ask for and has no signal that its request was contradictory.
  Fix: DescribeAlarmsForMetric input validation - the mutual-exclusion check. (The same check is also absent on PutMetricAlarm; that instance is already owned by the open issue "CloudWatch alarm writes and pagination skip validation or reword refusals".)
  Traces: cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit#task#5.

Cause 5 of 6: GetMetricData refuses a request AWS answers 200.
  AWS: Recorded 200 with a full GetMetricDataOutput - Messages: [], one MetricDataResult carrying Id, Label, StatusCode "PartialData", empty Timestamps and Values, and a NextToken. Per Smithy, GetMetricData models only InvalidNextToken as a client error; a window/period combination that yields no data is NOT an error, it is a PartialData result.
  Emulator: 400 - the whole response body is absent, so the client gets a refusal instead of an empty-but-valid result set.
  Why: this is the opposite failure mode and the most damaging: a legal query is rejected, so a client polling a metric window that currently has no datapoints errors out instead of looping until data arrives. Note that the StatusCode "PartialData" signal, and the NextToken for continuation, never reach the client at all.
  Fix: GetMetricData's handler/validation - stop refusing legal period/window combinations and return the PartialData result shape. (The NextToken value in the recording is a per-run opaque token and is not itself a requirement; the requirement is that one is returned.)
  Traces: cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes#task#1.

Cause 6 of 6: ListMetrics refuses a request AWS answers 200 with an empty list.
  AWS: Recorded 200 with Metrics: [] - a filter that matches nothing is an empty result, not an error. Smithy: ListMetrics models only InvalidParameterValueException and InternalServiceFault.
  Emulator: 400 with no Metrics member at all.
  Why: a client enumerating metrics for an idle namespace or dimension filter sees an error rather than "nothing yet", so discovery loops abort instead of returning empty.
  Fix: ListMetrics' handler/validation - accept the filter combination and return an empty Metrics list.
  Traces: cloudwatch-s3tfa-traj-list-metrics-is-empty-for-an-idle-tag-filter#task#6.

## Evidence
Cause 1 - GetMetricStatistics ExtendedStatistics 'P90': expected status 400 with __type "com.amazon.coral.service#InvalidParameterValueException" and message "The value P90 for parameter ExtendedStatistics.member.1.<list element> is not supported."; actual status 200 with both members absent.
Cause 2 - GetMetricData invalid Stat: expected status 400 with __type "com.amazon.coral.validate#ValidationException" and message "The value for parameter MetricDataQueries.member.2.MetricStat.Stat is not a valid statistic.\n"; actual status 200 with both members absent.
Cause 3 - PutMetricStream null ExcludeFilters Namespace: expected status 400 with __type "com.amazon.coral.service#InvalidParameterValueException", x-amzn-query-error "InvalidParameterValue;Sender" and message "Value null at 'ExcludeFilters.0.member.Namespace' failed to satisfy constraint: Member must not be null"; actual status 200 with none of those present.
Cause 4 - DescribeAlarmsForMetric Statistic+ExtendedStatistic: expected status 400 with __type "com.amazon.coral.validate#ValidationException", x-amzn-query-error "ValidationError;Sender" and message "DescribeAlarmsForMetric request can not be made with both Statistic and ExtendedStatistic parameters. No more than one of them should be present"; actual status 200 with none of those present.
Cause 5 - GetMetricData legal window: expected status 200 with Messages=[] and MetricDataResults=[{Id, Label, StatusCode "PartialData", Timestamps: [], Values: []}] plus a NextToken; actual status 400 with Messages, MetricDataResults and NextToken all absent.
Cause 6 - ListMetrics empty filter result: expected status 200 with Metrics=[]; actual status 400 with Metrics absent.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture cloudwatch-alarm-firing-percentile-query-mistakes, step task#3 - GetMetricStatistics with ExtendedStatistics=["P90"] (upper-case P). Expect 400, observe 200.
3. Cause 2: fixture cloudwatch-latency-metric-data-stat-and-discovery-mistakes, step task#1 - GetMetricData whose second MetricDataQuery has an invalid MetricStat.Stat. Expect 400, observe 200.
4. Cause 3: fixture cloudwatch-mstx-traj-rebuild-exclude-stream-refusals, step task#3 - PutMetricStream with an ExcludeFilters entry whose Namespace is null. Expect 400, observe 200.
5. Cause 4: fixture cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit, step task#5 - DescribeAlarmsForMetric with both Statistic and ExtendedStatistic set. Expect 400, observe 200.
6. Cause 5: fixture cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes, step task#1 - the legal GetMetricData query. Expect 200 with a PartialData result, observe 400.
7. Cause 6: fixture cloudwatch-s3tfa-traj-list-metrics-is-empty-for-an-idle-tag-filter, step task#6 - ListMetrics over an idle filter. Expect 200 with Metrics=[], observe 400.
