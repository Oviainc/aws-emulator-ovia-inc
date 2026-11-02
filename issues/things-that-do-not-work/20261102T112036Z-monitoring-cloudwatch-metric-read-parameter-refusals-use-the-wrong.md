Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-alarm-firing-dependency-metric-discovery
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-alarm-firing-dependency-metric-discovery cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes cloudwatch-albos-unhealthy-host-metric-query-mistakes cloudwatch-alb5xx-traj-get-metric-statistics-mistakes cloudwatch-emr-cluster-metrics-for-idle-unknown-jobflow cloudwatch-latency-metric-statistics-mistakes cloudwatch-rshealth-metric-reads-absent-cluster cloudwatch-s3tfa-traj-datapoints-of-an-idle-tag-filter-metric cloudwatch-latency-alarm-history-query-mistakes cloudwatch-latency-metric-data-stat-and-discovery-mistakes cloudwatch-s3tfa-traj-list-metrics-is-empty-for-an-idle-tag-filter cloudwatch-alarm-firing-percentile-query-mistakes
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|ListMetrics|4xx->4xx|$.__type
Issue-Key: monitoring|ListMetrics|4xx->4xx|$.message
Issue-Key: monitoring|ListMetrics|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetMetricData|4xx->4xx|$.__type
Issue-Key: monitoring|GetMetricData|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetMetricStatistics|4xx->4xx|$.__type
Issue-Key: monitoring|GetMetricStatistics|4xx->4xx|$.message
Issue-Key: monitoring|GetMetricStatistics|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|DescribeAlarmHistory|4xx->4xx|$.__type
Issue-Key: monitoring|DescribeAlarmHistory|4xx->4xx|$headers.x-amzn-query-error
Issue-Related: 20261029T203422Z-monitoring-cloudwatch-alarm-writes-and-pagination-skip-validation-or.md
Issue-Related: 20261102T112036Z-monitoring-cloudwatch-validationexception-messages-drop-the-offending-value-and.md
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# CloudWatch metric-read parameter refusals use the wrong exception type

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
This issue bundles NINE causes with one shared fault: on the CloudWatch metric-read operations (ListMetrics, GetMetricStatistics, GetMetricData, DescribeAlarmHistory) the emulator classifies parameter refusals under its own choice of exception shape and writes its own message prose, instead of the exception type and message template AWS uses for each refusal class. There are 9 causes to close.

The pattern is consistent: where AWS raises InvalidParameterValueException (x-amzn-query-error InvalidParameterValue;Sender) or MissingRequiredParameterException (MissingParameter;Sender) with a short "The parameter X ..." / "Request parameter X ..." sentence, the emulator raises com.amazon.coral.validate#ValidationException (ValidationError;Sender) with a Smithy-constraint message - and in two cases the swap runs the other way. All of these errors are modelled: Smithy lists InvalidParameterValueException (awsQueryError InvalidParameterValue, 400) and MissingRequiredParameterException (awsQueryError MissingParameter, 400) on GetMetricStatistics, and InvalidParameterValueException on ListMetrics.

Cause 1 of 9: ListMetrics RecentlyActive out-of-enum.
  AWS: Smithy: ListMetrics.RecentlyActive targets RecentlyActive, an enum with the single value [PT3H]; ListMetrics models InvalidParameterValueException (awsQueryError InvalidParameterValue, httpError 400). Recorded 400, __type com.amazon.coral.service#InvalidParameterValueException, x-amzn-query-error InvalidParameterValue;Sender, message "Request parameter recentlyActive value is invalid" - fixed text "Request parameter " + the member name + " value is invalid".
  Emulator: 400 but __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender, message "1 validation error detected: Value at 'recentlyActive' failed to satisfy constraint: Member must satisfy enum value set: [PT3H]".
  Why: an SDK deserialises the response into the modelled error named by __type/x-amzn-query-error; a client catching InvalidParameterValueException sees an unmodelled ValidationException instead and falls through to generic error handling.
  Fix: ListMetrics input validation - raise the modelled InvalidParameterValueException for RecentlyActive.
  Traces: cloudwatch-alarm-firing-dependency-metric-discovery#task#2 (plus cloudwatch-alarm-firing-dependency-metric-discovery#task#3).

Cause 2 of 9: ListMetrics invalid NextToken message.
  AWS: Recorded 400 with message "Request parameter NextToken is invalid" - fixed text "Request parameter " + the parameter name + " is invalid".
  Emulator: 400 with message "NextToken is not a token this service issued", an emulator-authored sentence that appears nowhere in AWS.
  Why: message text is a requirement; nothing matching AWS's wording is returned, so any client or test asserting on the refusal text fails.
  Fix: ListMetrics pagination-token validation message.
  Traces: cloudwatch-alarm-firing-dependency-metric-discovery#task#4.

Cause 3 of 9: GetMetricData StartTime equal to EndTime.
  AWS: Recorded 400 with __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender, message "The parameter StartTime must not equal parameter EndTime.\n" - fixed text with a trailing newline, no values interpolated.
  Emulator: 400 with __type com.amazon.coral.service#InvalidParameterValueException, x-amzn-query-error InvalidParameterValue;Sender, message "EndTime must be later than StartTime; got StartTime 1790766000 and EndTime 1790766000 (epoch seconds)" - the wrong exception type, and an emulator-authored message that also leaks raw epoch values AWS never echoes.
  Why: the error type and the message both diverge; this is the inverse of causes 1-2, so a single blanket re-classification will not fix the set - each refusal class must map to the type AWS uses for it.
  Fix: GetMetricData time-window validation.
  Traces: cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes#task#4 (plus cloudwatch-albos-unhealthy-host-metric-query-mistakes#task#4).

Cause 4 of 9: GetMetricStatistics Period=0.
  AWS: Smithy: GetMetricStatistics.Period is required with @range min 1, and the operation models InvalidParameterValueException. Recorded 400 with __type com.amazon.coral.service#InvalidParameterValueException, x-amzn-query-error InvalidParameterValue;Sender, message "The parameter Period must be greater than 0." - fixed text, parameter name interpolated.
  Emulator: 400 with __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender, message "1 validation error detected: Value '0' at 'period' failed to satisfy constraint: Member must have value greater than or equal to 1".
  Why: same mis-classification as cause 1; the client never sees the modelled InvalidParameterValueException.
  Fix: GetMetricStatistics Period validation.
  Traces: cloudwatch-alb5xx-traj-get-metric-statistics-mistakes#task#2 (plus cloudwatch-alb5xx-traj-get-metric-statistics-mistakes#task#4).

Cause 5 of 9: GetMetricStatistics Period absent.
  AWS: Smithy: GetMetricStatistics.Period is required and the operation models MissingRequiredParameterException (awsQueryError MissingParameter, 400). Recorded 400 with __type com.amazon.coral.service#MissingRequiredParameterException and message "The parameter Period is required." - fixed text "The parameter " + name + " is required.".
  Emulator: 400 with __type com.amazon.coral.validate#ValidationException and message "1 validation error detected: Value null at 'period' failed to satisfy constraint: Member must not be null".
  Why: AWS distinguishes a missing required parameter (MissingRequiredParameterException) from a present-but-invalid one; the emulator collapses both into ValidationException, so a client cannot tell them apart from the error code.
  Fix: GetMetricStatistics required-member validation.
  Traces: cloudwatch-emr-cluster-metrics-for-idle-unknown-jobflow#task#3 (plus cloudwatch-latency-metric-statistics-mistakes#task#5, cloudwatch-latency-metric-statistics-mistakes#task#9, cloudwatch-rshealth-metric-reads-absent-cluster#task#2).

Cause 6 of 9: GetMetricStatistics Period granularity rule is wrong.
  AWS: for a period above the high-resolution range, AWS requires a multiple of 60 and says so: recorded 400 message "The parameter Period must be a multiple of 60.".
  Emulator: 400 message "The parameter Period must be a value in the set [1, 5, 10, 20, 30]." - it applies only the high-resolution set and never the multiple-of-60 rule, so it both refuses with the wrong reason and (by implication) refuses legal periods such as 120 or 300 for the same reason. This is a RULE divergence, not just wording.
  Why: a client sending a legal non-high-resolution period is told to pick from a five-value set that excludes every period CloudWatch actually uses for standard-resolution metrics.
  Fix: GetMetricStatistics Period validation - accept 1/5/10/20/30 for high resolution and any multiple of 60 above that, with AWS's corresponding message.
  Traces: cloudwatch-latency-metric-statistics-mistakes#task#4 (plus cloudwatch-latency-metric-statistics-mistakes#task#6, cloudwatch-s3tfa-traj-datapoints-of-an-idle-tag-filter-metric#task#6).

Cause 7 of 9: DescribeAlarmHistory StartDate after EndDate.
  AWS: Recorded 400 with __type com.amazon.coral.service#InvalidParameterValueException, x-amzn-query-error InvalidParameterValue;Sender, message "Start date <StartDate> must be before end date <EndDate>", where the two dates are the request's StartDate and EndDate rendered in AWS's Java-style UTC form ("EEE MMM dd HH:mm:ss 'UTC' yyyy"). Fixed text: "Start date ", " must be before end date ". The dates in any one recording are request values, not part of the template.
  Emulator: 400 with __type com.amazon.coral.validate#ValidationException, x-amzn-query-error ValidationError;Sender, message "StartDate must not be later than EndDate" - wrong type, wrong header, and a message that echoes neither date.
  Why: the client loses both the modelled error code and the two timestamps that tell it which bound was wrong.
  Fix: DescribeAlarmHistory date-range validation.
  Traces: cloudwatch-latency-alarm-history-query-mistakes#task#4.

Cause 8 of 9: ListMetrics dimension filter with no Name.
  AWS: Smithy: ListMetrics.Dimensions is a list of DimensionFilter whose Name is required; the operation models InvalidParameterValueException, and for an absent member AWS answers MissingRequiredParameterException. Recorded 400 with __type com.amazon.coral.service#MissingRequiredParameterException and message "The parameter Dimensions.member.1.Name is required." - fixed text "The parameter " + the awsQuery-style member path + " is required.", the index coming from the request.
  Emulator: 400 with __type com.amazon.coral.validate#ValidationException and message "1 validation error detected: Value null at 'dimensions.1.member.name' failed to satisfy constraint: Member must not be null".
  Why: wrong error code plus a different member-path notation, so neither programmatic nor human consumers of the refusal match AWS.
  Fix: ListMetrics Dimensions member validation.
  Traces: cloudwatch-latency-metric-data-stat-and-discovery-mistakes#task#5 (plus cloudwatch-s3tfa-traj-list-metrics-is-empty-for-an-idle-tag-filter#task#3, #task#4, #task#5).

Cause 9 of 9: GetMetricData MetricStat.Stat invalid.
  AWS: Recorded 400 with __type com.amazon.coral.validate#ValidationException and message "The value for parameter MetricDataQueries.member.1.MetricStat.Stat is not a valid statistic.\n" - fixed text "The value for parameter " + the awsQuery-style member path + " is not a valid statistic." with a trailing newline; the path's index comes from the request.
  Emulator: 400 with __type com.amazon.coral.service#InvalidParameterValueException and message "Avg is not a supported statistic" - wrong exception type and an emulator-authored sentence that names the value instead of the parameter path.
  Why: the client is not told WHICH query in its MetricDataQueries array was rejected, which for a multi-query request is the only actionable part of the message.
  Fix: GetMetricData MetricStat.Stat validation.
  Traces: cloudwatch-alarm-firing-percentile-query-mistakes#task#9.

## Evidence
Cause 1 - ListMetrics RecentlyActive: expected __type "com.amazon.coral.service#InvalidParameterValueException", x-amzn-query-error "InvalidParameterValue;Sender", message "Request parameter recentlyActive value is invalid"; actual __type "com.amazon.coral.validate#ValidationException", x-amzn-query-error "ValidationError;Sender", message "1 validation error detected: Value at 'recentlyActive' failed to satisfy constraint: Member must satisfy enum value set: [PT3H]". Status 400 both.
Cause 2 - ListMetrics NextToken: expected message "Request parameter NextToken is invalid"; actual "NextToken is not a token this service issued".
Cause 3 - GetMetricData StartTime==EndTime: expected __type "com.amazon.coral.validate#ValidationException", x-amzn-query-error "ValidationError;Sender", message "The parameter StartTime must not equal parameter EndTime.\n"; actual __type "com.amazon.coral.service#InvalidParameterValueException", x-amzn-query-error "InvalidParameterValue;Sender", message "EndTime must be later than StartTime; got StartTime 1790766000 and EndTime 1790766000 (epoch seconds)".
Cause 4 - GetMetricStatistics Period=0: expected __type "com.amazon.coral.service#InvalidParameterValueException", x-amzn-query-error "InvalidParameterValue;Sender", message "The parameter Period must be greater than 0."; actual __type "com.amazon.coral.validate#ValidationException", x-amzn-query-error "ValidationError;Sender", message "1 validation error detected: Value '0' at 'period' failed to satisfy constraint: Member must have value greater than or equal to 1".
Cause 5 - GetMetricStatistics Period absent: expected __type "com.amazon.coral.service#MissingRequiredParameterException", message "The parameter Period is required."; actual __type "com.amazon.coral.validate#ValidationException", message "1 validation error detected: Value null at 'period' failed to satisfy constraint: Member must not be null".
Cause 6 - GetMetricStatistics Period granularity: expected message "The parameter Period must be a multiple of 60."; actual "The parameter Period must be a value in the set [1, 5, 10, 20, 30].".
Cause 7 - DescribeAlarmHistory date range: expected __type "com.amazon.coral.service#InvalidParameterValueException", x-amzn-query-error "InvalidParameterValue;Sender", message of the form "Start date <StartDate in EEE MMM dd HH:mm:ss UTC yyyy> must be before end date <EndDate in the same form>"; actual __type "com.amazon.coral.validate#ValidationException", x-amzn-query-error "ValidationError;Sender", message "StartDate must not be later than EndDate".
Cause 8 - ListMetrics dimension Name: expected __type "com.amazon.coral.service#MissingRequiredParameterException", message "The parameter Dimensions.member.1.Name is required."; actual __type "com.amazon.coral.validate#ValidationException", message "1 validation error detected: Value null at 'dimensions.1.member.name' failed to satisfy constraint: Member must not be null".
Cause 9 - GetMetricData Stat: expected __type "com.amazon.coral.validate#ValidationException", message "The value for parameter MetricDataQueries.member.1.MetricStat.Stat is not a valid statistic.\n"; actual __type "com.amazon.coral.service#InvalidParameterValueException", message "Avg is not a supported statistic".

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture cloudwatch-alarm-firing-dependency-metric-discovery, step task#2 - ListMetrics with RecentlyActive set to a value outside [PT3H].
3. Cause 2: fixture cloudwatch-alarm-firing-dependency-metric-discovery, step task#4 - ListMetrics with a NextToken this service did not issue.
4. Cause 3: fixture cloudwatch-alb5xx-traj-get-metric-data-period-and-window-mistakes, step task#4 - GetMetricData with StartTime == EndTime.
5. Cause 4: fixture cloudwatch-alb5xx-traj-get-metric-statistics-mistakes, step task#2 - GetMetricStatistics with Period=0.
6. Cause 5: fixture cloudwatch-emr-cluster-metrics-for-idle-unknown-jobflow, step task#3 - GetMetricStatistics with Period omitted.
7. Cause 6: fixture cloudwatch-latency-metric-statistics-mistakes, step task#4 - GetMetricStatistics with a Period that is neither in [1,5,10,20,30] nor a multiple of 60.
8. Cause 7: fixture cloudwatch-latency-alarm-history-query-mistakes, step task#4 - DescribeAlarmHistory with StartDate after EndDate.
9. Cause 8: fixture cloudwatch-latency-metric-data-stat-and-discovery-mistakes, step task#5 - ListMetrics with a Dimensions entry whose Name is omitted.
10. Cause 9: fixture cloudwatch-alarm-firing-percentile-query-mistakes, step task#9 - GetMetricData whose first MetricStat.Stat is not a valid statistic.
11. In each case compare __type, x-amzn-query-error and message against the recorded response.
