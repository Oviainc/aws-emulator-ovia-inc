Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-alarm-firing-describe-filter-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-alarm-firing-describe-filter-mistakes cloudwatch-albos-alarm-lookup-and-state-mistakes cloudwatch-latency-alarm-lookup-mistakes cloudwatch-s3tfa-traj-describe-alarms-filter-mistakes-then-scoped-hit cloudwatch-rshealth-alarm-lookup-by-metric cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit cloudwatch-mstx-traj-rebuild-exclude-stream-refusals cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes cloudwatch-emr-cluster-metrics-for-idle-unknown-jobflow cloudwatch-latency-metric-data-stat-and-discovery-mistakes cloudwatch-rshealth-metric-reads-absent-cluster cloudwatch-mstx-traj-get-name-mistakes
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeAlarms|4xx->4xx|$.message
Issue-Key: monitoring|DescribeAlarmsForMetric|4xx->4xx|$.message
Issue-Key: monitoring|PutMetricStream|4xx->4xx|$.message
Issue-Key: monitoring|GetMetricData|4xx->4xx|$.message
Issue-Key: monitoring|DescribeAlarms|4xx->4xx|$.__type
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$.__type
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$.message
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|DescribeAlarmsForMetric|4xx->4xx|$.__type
Issue-Key: monitoring|DescribeAlarmsForMetric|4xx->4xx|$headers.x-amzn-query-error
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# CloudWatch ValidationException messages drop the offending value and re-sort the enum set

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
This issue bundles SEVEN causes, all rooted in the one shared fault: the CloudWatch emulator's constraint-validation message formatter builds a ValidationException message from its own template instead of AWS's, and in three cases picks the wrong exception shape altogether. There are 7 causes to close.

AWS's ValidationException message for an out-of-enum member is a fixed template whose variable parts come from the request and the model: "<N> validation error detected: Value '<the value the client sent>' at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<the enum's values, in MODEL DECLARATION ORDER>]". The two things the emulator gets wrong in every instance are (a) it omits the `Value '<sent value>'` segment entirely, and (b) it emits the value set sorted alphabetically rather than in the order the Smithy enum declares. Neither is a per-run value: the sent value comes from the request, and the declaration order is fixed by the model.

Cause 1 of 7: DescribeAlarms StateValue.
  AWS: Smithy: DescribeAlarms.StateValue targets com.amazonaws.cloudwatch#StateValue, an enum whose declared values are [OK, ALARM, INSUFFICIENT_DATA]. Recorded response 400 with __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender and message "1 validation error detected: Value 'alarm' at 'stateValue' failed to satisfy constraint: Member must satisfy enum value set: [INSUFFICIENT_DATA, ALARM, OK]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable parts: the rejected value ('alarm') from the request, the member path ('stateValue'), and the enum value list.
  Emulator: 400 with the right __type, but message "1 validation error detected: Value at 'stateValue' failed to satisfy constraint: Member must satisfy enum value set: [ALARM, INSUFFICIENT_DATA, OK]" - the sent value is missing and the set is alphabetised.
  Why: a client or operator reading the refusal cannot see WHICH value was rejected, which is the whole point of the message; anything asserting on AWS's message text (SDK test suites, support tooling, log scrapers) fails.
  Fix: the shared enum-constraint message formatter used by DescribeAlarms' input validation.
  Traces: cloudwatch-alarm-firing-describe-filter-mistakes#task#2 (plus cloudwatch-albos-alarm-lookup-and-state-mistakes#task#3, cloudwatch-latency-alarm-lookup-mistakes#task#2, cloudwatch-s3tfa-traj-describe-alarms-filter-mistakes-then-scoped-hit#task#4).

Cause 2 of 7: DescribeAlarmsForMetric Statistic.
  AWS: Smithy: DescribeAlarmsForMetric.Statistic targets com.amazonaws.cloudwatch#Statistic, declared [SampleCount, Average, Sum, Minimum, Maximum]. Recorded 400 message "1 validation error detected: Value 'p90' at 'statistic' failed to satisfy constraint: Member must satisfy enum value set: [Maximum, SampleCount, Sum, Minimum, Average]".
  Emulator: "1 validation error detected: Value at 'statistic' failed to satisfy constraint: Member must satisfy enum value set: [Average, Maximum, Minimum, SampleCount, Sum]".
  Why: same loss of the rejected value; a client that sent a percentile in Statistic (instead of ExtendedStatistic) is not told what it sent.
  Fix: DescribeAlarmsForMetric input validation / the shared enum message formatter.
  Traces: cloudwatch-alarm-firing-describe-filter-mistakes#task#8 (plus cloudwatch-rshealth-alarm-lookup-by-metric#task#4, cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit#task#2).

Cause 3 of 7: PutMetricStream OutputFormat.
  AWS: Smithy: PutMetricStream.OutputFormat is required and targets MetricStreamOutputFormat, declared [json, opentelemetry0.7, opentelemetry1.0]. Recorded 400 message "1 validation error detected: Value 'JSON' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [opentelemetry1.0, json, opentelemetry0.7]".
  Emulator: "1 validation error detected: Value at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [json, opentelemetry0.7, opentelemetry1.0]".
  Why: the enum is case-sensitive; a client that sent 'JSON' is not shown its own casing and cannot tell the value was mis-cased rather than unsupported.
  Fix: PutMetricStream input validation / the shared enum message formatter.
  Traces: cloudwatch-mstx-traj-rebuild-exclude-stream-refusals#task#1.

Cause 4 of 7: GetMetricData required-member message inserts a spurious "null".
  AWS: Smithy: MetricStat.Period is required inside GetMetricData.MetricDataQueries. Recorded 400 message "1 validation error detected: Value at 'metricDataQueries.2.member.metricStat.period' failed to satisfy constraint: Member must not be null" - AWS writes no value segment when the member is absent.
  Emulator: "1 validation error detected: Value null at 'metricDataQueries.2.member.metricStat.period' failed to satisfy constraint: Member must not be null" - it inserts the literal token "null" where AWS writes nothing. This is the same formatter defect seen from the other side: it has one value slot and fills it unconditionally.
  Why: byte-exact message assertions fail, and the message no longer matches AWS's own documented not-null wording.
  Fix: the shared not-null constraint message formatter in GetMetricData input validation.
  Traces: cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes#task#2 (plus cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes#task#3, cloudwatch-emr-cluster-metrics-for-idle-unknown-jobflow#task#6, cloudwatch-latency-metric-data-stat-and-discovery-mistakes#task#3, cloudwatch-rshealth-metric-reads-absent-cluster#task#4).

Cause 5 of 7: DescribeAlarms AlarmTypes is refused with the wrong exception AND a non-AWS message.
  AWS: Smithy: DescribeAlarms.AlarmTypes is a list of AlarmType. Recorded 400 with __type com.amazon.coral.validate#ValidationException and message "1 validation error detected: Value '[Metric]' at 'alarmTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [MetricAlarm, CompositeAlarm]]" - i.e. a list member violation is reported as a ValidationException with the nested "Member must satisfy constraint: [...]" wrapper, and the set AWS advertises on this path is [MetricAlarm, CompositeAlarm].
  Emulator: 400 with __type com.amazon.coral.service#InvalidParameterValueException and message "The parameter AlarmTypes.member.1.<list element> must be a value in the set [CompositeAlarm, MetricAlarm, LogAlarm]." - wrong exception shape, wrong member path form, wrong message template, and it advertises LogAlarm which AWS does not offer on this path.
  Why: the error TYPE is part of the contract; an SDK maps com.amazon.coral.validate#ValidationException and com.amazon.coral.service#InvalidParameterValueException to different modelled errors, so client-side error handling and retry classification diverge, not just the text.
  Fix: DescribeAlarms input validation - classify list-member enum violations as ValidationException and use the nested-constraint message template.
  Traces: cloudwatch-alarm-firing-describe-filter-mistakes#task#3 (plus cloudwatch-latency-alarm-lookup-mistakes#task#3).

Cause 6 of 7: GetMetricStream with an empty Name raises MissingRequiredParameter instead of ValidationException.
  AWS: Smithy: GetMetricStream.Name is required and targets MetricStreamName with @length min 1, max 255. An empty string is PRESENT but violates the length constraint, so AWS reports the length violation: recorded 400 with __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender, message "1 validation error detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1".
  Emulator: 400 with __type com.amazon.coral.service#MissingRequiredParameterException, x-amzn-query-error MissingParameter;Sender, message "The parameter Name is required." - it treats empty-string as absent.
  Why: the client is told the parameter is missing when it was supplied; error code, query-error header and message all diverge, so error-code-driven branching takes the wrong path.
  Fix: GetMetricStream input validation - distinguish an absent member from a present member failing @length.
  Traces: cloudwatch-mstx-traj-get-name-mistakes#task#4.

Cause 7 of 7: DescribeAlarmsForMetric empty dimension Value raises MissingRequiredParameter instead of ValidationException.
  AWS: Smithy: Dimension.Value has @length min 1; it is present-but-empty, so AWS reports the length violation: recorded 400 with __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender, message "1 validation error detected: Value '' at 'dimensions.2.member.value' failed to satisfy constraint: Member must have length greater than or equal to 1". Note the member path form AWS uses here: 'dimensions.<index>.member.value'.
  Emulator: 400 with __type com.amazon.coral.service#MissingRequiredParameterException, x-amzn-query-error MissingParameter;Sender, message "The parameter Dimensions.member.2.Value is required." - wrong exception, wrong header, wrong path form.
  Why: identical to cause 6 - an empty dimension value is a constraint violation, not a missing parameter, and clients branch on the code.
  Fix: DescribeAlarmsForMetric input validation (Dimensions member constraints).
  Traces: cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit#task#3.

## Evidence
Cause 1 - DescribeAlarms StateValue: expected 400 message "1 validation error detected: Value 'alarm' at 'stateValue' failed to satisfy constraint: Member must satisfy enum value set: [INSUFFICIENT_DATA, ALARM, OK]"; actual 400 message "1 validation error detected: Value at 'stateValue' failed to satisfy constraint: Member must satisfy enum value set: [ALARM, INSUFFICIENT_DATA, OK]".
Cause 2 - DescribeAlarmsForMetric Statistic: expected "...Value 'p90' at 'statistic'... [Maximum, SampleCount, Sum, Minimum, Average]"; actual "...Value at 'statistic'... [Average, Maximum, Minimum, SampleCount, Sum]".
Cause 3 - PutMetricStream OutputFormat: expected "...Value 'JSON' at 'outputFormat'... [opentelemetry1.0, json, opentelemetry0.7]"; actual "...Value at 'outputFormat'... [json, opentelemetry0.7, opentelemetry1.0]".
Cause 4 - GetMetricData period: expected "1 validation error detected: Value at 'metricDataQueries.2.member.metricStat.period' failed to satisfy constraint: Member must not be null"; actual the same with "Value null at".
Cause 5 - DescribeAlarms AlarmTypes: expected __type com.amazon.coral.validate#ValidationException, message "1 validation error detected: Value '[Metric]' at 'alarmTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [MetricAlarm, CompositeAlarm]]"; actual __type com.amazon.coral.service#InvalidParameterValueException, message "The parameter AlarmTypes.member.1.<list element> must be a value in the set [CompositeAlarm, MetricAlarm, LogAlarm].".
Cause 6 - GetMetricStream empty Name: expected __type com.amazon.coral.validate#ValidationException, x-amzn-query-error "ValidationError;Sender", message "1 validation error detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1"; actual __type com.amazon.coral.service#MissingRequiredParameterException, x-amzn-query-error "MissingParameter;Sender", message "The parameter Name is required.".
Cause 7 - DescribeAlarmsForMetric empty dimension value: expected __type com.amazon.coral.validate#ValidationException, x-amzn-query-error "ValidationError;Sender", message "1 validation error detected: Value '' at 'dimensions.2.member.value' failed to satisfy constraint: Member must have length greater than or equal to 1"; actual __type com.amazon.coral.service#MissingRequiredParameterException, x-amzn-query-error "MissingParameter;Sender", message "The parameter Dimensions.member.2.Value is required.".
All statuses matched at 400 for causes 1-7 except that no status divergence was reported; the divergence is in __type, x-amzn-query-error and message.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus) against it.
2. Cause 1: fixture cloudwatch-alarm-firing-describe-filter-mistakes, step task#2 - DescribeAlarms with StateValue="alarm" (lowercase).
3. Cause 2: fixture cloudwatch-alarm-firing-describe-filter-mistakes, step task#8 - DescribeAlarmsForMetric with Statistic="p90".
4. Cause 3: fixture cloudwatch-mstx-traj-rebuild-exclude-stream-refusals, step task#1 - PutMetricStream with OutputFormat="JSON".
5. Cause 4: fixture cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes, step task#2 - GetMetricData whose third MetricDataQuery omits MetricStat.Period.
6. Cause 5: fixture cloudwatch-alarm-firing-describe-filter-mistakes, step task#3 - DescribeAlarms with AlarmTypes=["Metric"].
7. Cause 6: fixture cloudwatch-mstx-traj-get-name-mistakes, step task#4 - GetMetricStream with Name="".
8. Cause 7: fixture cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit, step task#3 - DescribeAlarmsForMetric whose second Dimension has Value="".
9. In each case compare the response __type, the x-amzn-query-error header and the message against the recorded expectation.
