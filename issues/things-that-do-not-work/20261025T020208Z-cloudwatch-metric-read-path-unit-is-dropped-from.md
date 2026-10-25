Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.unit-is-part-of-a-datapoints-read-identity cloudwatch.get-metric-statistics-and-toomany-datapoints-rejected cloudwatch.get-metric-statistics-missing-statistic-type-rejected cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected cloudwatch.list-metrics-too-many-dimensions-rejected cloudwatch.metric-data-long-term-retention-downsampling-boundary cloudwatch.metric-data-sub-minute-resolution-3-hour-expiry-boundary
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.unit-is-part-of-a-datapoints-read-identity|run#26|AssertionError|assert len(unitless) == 3, (
Issue-Key: cloudwatch.get-metric-statistics-and-toomany-datapoints-rejected|run#15|AssertionError|assert message == expected, f"expected the exact message {expected!r}, got {message!r}"
Issue-Key: cloudwatch.get-metric-statistics-missing-statistic-type-rejected|run#8|AssertionError|assert code == "InvalidParameterCombination", f"expected InvalidParameterCombination, got {code}"
Issue-Key: cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected|run#14|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: cloudwatch.list-metrics-too-many-dimensions-rejected|run#7|AssertionError|assert error == ("InvalidParameterValue", "The collection Dimensions must not have a size greater than 30."), (
Issue-Key: cloudwatch.metric-data-long-term-retention-downsampling-boundary|run#12|AssertionError|assert fine_old["Datapoints"] == [], (
Issue-Key: cloudwatch.metric-data-sub-minute-resolution-3-hour-expiry-boundary|run#13|AssertionError|assert error == ("InvalidParameterCombination", (
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# CloudWatch metric read path: Unit is dropped from datapoint identity, retention tiers are not enforced, and four refusals use unmodeled ValidationException or the wrong code/message (7 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Seven net-new root causes on the metric read path.

Cause 1 of 7: Unit is not part of a datapoint's read identity.
AWS: Smithy GetMetricStatisticsInput has an optional `Unit` (StandardUnit enum) and GetMetricStatisticsOutput returns `Datapoints`. Datapoints published at the same timestamp under different units are distinct series: a GetMetricStatistics call that omits Unit returns one datapoint per published unit at that timestamp, not a single unit-less aggregate. Recorded behavior: 3 datapoints for 3 units at one timestamp.
Emulator: returns 1 datapoint, [{'Timestamp': ..., 'SampleCount': 3.0, 'Sum': 110.0}] — the three unit-distinct series have been merged into one aggregate and Unit is absent.
Why: a client reading a metric published with mixed units gets a nonsensical cross-unit aggregate instead of the per-unit series, and cannot tell which unit a value belongs to.
Fix: unit must be part of the stored datapoint key in the cloudwatch metric store, and in the GetMetricStatistics read/aggregation path.
Traces: cloudwatch.unit-is-part-of-a-datapoints-read-identity.

Cause 2 of 7: the 1440-datapoint cap refusal message is rewritten.
AWS: when (EndTime-StartTime)/Period exceeds 1440, GetMetricStatistics is refused 400 InvalidParameterCombination with the documented message "You have requested up to 129600 datapoints, which exceeds the limit of 1440. You may reduce the datapoints requested by increasing Period, or decreasing the time range." — built from the request's own period count and the cap.
Emulator: refuses with 'the requested time range and Period span 129600 periods; at most 1440 data points can be returned from one call'.
Why: the documented message states the remedy (increase Period or shrink the range); clients and operators key on it.
Fix: the refusal message in the GetMetricStatistics handler's datapoint-cap validation.
Traces: cloudwatch.get-metric-statistics-and-toomany-datapoints-rejected.

Cause 3 of 7: a GetMetricStatistics call with neither Statistics nor ExtendedStatistics is misclassified.
AWS: refused 400 InvalidParameterCombination (Smithy models InvalidParameterCombinationException on GetMetricStatistics) — the two members are an either/or combination, not a single missing required member.
Emulator: refuses with MissingParameter (MissingRequiredParameterException).
Why: a client branching on InvalidParameterCombination to re-issue with a statistic type never takes that branch.
Fix: error classification in the GetMetricStatistics handler's validation.
Traces: cloudwatch.get-metric-statistics-missing-statistic-type-rejected.

Cause 4 of 7: an extended statistic passed in `Statistics` is refused with an unmodeled ValidationException.
AWS: `Statistics` is a list of the Statistic enum (SampleCount|Average|Sum|Minimum|Maximum); a percentile such as 'p90' belongs in ExtendedStatistics and the mistake is refused 400 InvalidParameterValue. GetMetricStatistics models InvalidParameterValueException, InvalidParameterCombinationException, MissingRequiredParameterException, InternalServiceFault — and no ValidationException.
Emulator: answers ValidationException with the generic "1 validation error detected: Value 'p90' at 'statistics.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]".
Why: ValidationException is not in this operation's error set, so SDK error mapping and `except InvalidParameterValueException` handlers never match it.
Fix: enum/constraint rejection for GetMetricStatistics must surface as the modeled InvalidParameterValue rather than a generic protocol-level ValidationException.
Traces: cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected.

Cause 5 of 7: ListMetrics over-long Dimensions is refused with an unmodeled ValidationException.
AWS: Smithy ListMetricsInput.Dimensions is @length(0,10) and the only client error modeled on ListMetrics is InvalidParameterValueException; 31 dimension filters are refused 400 InvalidParameterValue.
Emulator: answers ValidationException "1 validation error detected: Value '31 items' at 'dimensions' failed to satisfy constraint: Member must have length less than or equal to 10".
Why: same as cause 4 — an error code the operation does not model.
Fix: constraint rejection for ListMetrics must surface as the modeled InvalidParameterValue.
Traces: cloudwatch.list-metrics-too-many-dimensions-rejected.

Cause 6 of 7: high-resolution (sub-minute) datapoints are never aged out of the 3-hour tier.
AWS: datapoints stored at sub-minute resolution are retained for 3 hours; a query with a sub-60 s Period whose StartTime is older than that returns no datapoints for the expired region (they survive only as the aggregated 1-minute/5-minute tiers). Recorded behavior: an empty Datapoints list.
Emulator: returns the original sub-minute datapoint, [{'Timestamp': ..., 'Sum': 12345.0, 'Unit': 'Count'}].
Why: a client validating retention/downsampling behavior sees data AWS has already expired, so capacity and cost assumptions built on the retention tiers are wrong.
Fix: retention-tier expiry and downsampling in the cloudwatch metric store and the GetMetricStatistics read path.
Traces: cloudwatch.metric-data-long-term-retention-downsampling-boundary.

Cause 7 of 7: a sub-minute-Period query spanning past the high-resolution tier is refused for the wrong reason.
AWS: refused 400 InvalidParameterCombination because the sub-minute Period cannot cover a range beyond the high-resolution retention tier; the refusal names that tier conflict.
Emulator: refuses InvalidParameterCombination but with the datapoint-cap message 'the requested time range and Period span 14460 periods; at most 1440 data points can be returned from one call' — the generic cap text of cause 2, not the retention-tier conflict.
Why: the caller is told to widen Period or shrink the range when the actual constraint is retention, so following the message does not produce a valid request.
Fix: ordering and message of the Period-vs-retention validation in the GetMetricStatistics handler.
Traces: cloudwatch.metric-data-sub-minute-resolution-3-hour-expiry-boundary.

## Evidence
Cause 1: "AssertionError: GetMetricStatistics with no Unit returned 1 datapoints, expected 3 -- one per published unit, at one timestamp, NOT a single aggregate: [{'Timestamp': ..., 'SampleCount': 3.0, 'Sum': 110.0}]".
Cause 2: "AssertionError: expected the exact message 'You have requested up to 129600 datapoints, which exceeds the limit of 1440. You may reduce the datapoints requested by increasing Period, or decreasing the time range.', got 'the requested time range and Period span 129600 periods; at most 1440 data points can be returned from one call'".
Cause 3: "AssertionError: expected InvalidParameterCombination, got MissingParameter".
Cause 4: "AssertionError: Statistics=['p90'] gave 'ValidationException': \"1 validation error detected: Value 'p90' at 'statistics.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SampleCount, Average, Sum, Minimum, Maximum]\"; expected InvalidParameterValue".
Cause 5: "AssertionError: expected the InvalidParameterValue rejection for 31 dimension filters, got ('ValidationException', \"1 validation error detected: Value '31 items' at 'dimensions' failed to satisfy constraint: Member must have length less than or equal to 10\")".
Cause 6: "AssertionError: expected no datapoints for a >3h-old StartTime queried at a sub-60s period, got [{'Timestamp': ..., 'Sum': 12345.0, 'Unit': 'Count'}]".
Cause 7: "AssertionError: expected the InvalidParameterCombination rejection for a sub-minute-Period query spanning beyond the high-resolution retention tier, got ('InvalidParameterCombination', 'the requested time range and Period span 14460 periods; at most 1440 data points can be returned from one call')".

## Steps to Reproduce
1. PutMetricData three datums for one metric at one timestamp with Unit=Count, Milliseconds and Bytes; GetMetricStatistics without Unit -> 1 merged datapoint instead of 3.
2. GetMetricStatistics with Period=60 over a 90-day range -> message is not the documented 1440-datapoint text.
3. GetMetricStatistics with neither Statistics nor ExtendedStatistics -> MissingParameter instead of InvalidParameterCombination.
4. GetMetricStatistics with Statistics=['p90'] -> ValidationException instead of InvalidParameterValue.
5. ListMetrics with 31 Dimensions filters -> ValidationException instead of InvalidParameterValue.
6. PutMetricData with StorageResolution=1, then GetMetricStatistics with Period=10 and StartTime more than 3 h ago -> the sub-minute datapoint is still returned.
7. GetMetricStatistics with Period=10 over a range extending past the 3 h high-resolution tier -> refused with the datapoint-cap message rather than the retention-tier conflict.
