Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected|run#15|AssertionError|assert message == (
Issue-Commit: 7ce2b3eec7df75677d4abb942fccfdb7efd0ecba

# GetMetricStatistics refuses a percentile in Statistics with an emulator-authored message instead of AWS's "must be a value in the set [...]" text (1 cause)

> _Note: this issue was identified against the code base at commit `7ce2b3eec7df75677d4abb942fccfdb7efd0ecba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
REOPENS 20261025T020208Z-cloudwatch-metric-read-path-unit-is-dropped-from.md: its fix left the call refused. That issue's cause 4 covered exactly this assertion (Statistics=['p90'] on GetMetricStatistics). The fix corrected the error CODE — the call is now refused with the modeled InvalidParameterValue rather than an unmodeled ValidationException — but the refusal MESSAGE is still emulator-authored, so the same check still fails, now one assertion further on. This is not a new defect; it is the un-finished half of that fix.

Cause 1 of 1: the GetMetricStatistics refusal message for a name that is not a member of the Statistic enum is invented, and the sibling refusal message for a standard statistic placed in ExtendedStatistics is invented in the same handler.
Failing check ids: cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json) models GetMetricStatisticsInput$Statistics as `Statistics`, a list @length(1,5) whose member targets the `Statistic` enum with exactly the values [SampleCount, Average, Sum, Minimum, Maximum], and GetMetricStatisticsInput$ExtendedStatistics as `ExtendedStatistics`, a list @length(1,10) of the plain-string `ExtendedStatistic`. The operation models InvalidParameterValueException (errorKind client, httpError 400, awsQueryError code `InvalidParameterValue`, httpResponseCode 400). Recorded behavior pins the two messages for the two name/field mix-ups:
  * a percentile placed in `Statistics` is refused 400 `InvalidParameterValue` with the message `The parameter Statistics.member.<n>.<list element> must be a value in the set [SampleCount, Average, Sum, Minimum, Maximum].` The fixed text is `The parameter ` + `.<list element> must be a value in the set [SampleCount, Average, Sum, Minimum, Maximum].`; the only variable part is the member path `Statistics.member.<n>`, where `<n>` is the 1-based index of the offending element in the request's list. The message does NOT echo the submitted value, and the allowed-value set is the `Statistic` enum the model declares.
  * a standard statistic placed in `ExtendedStatistics` is refused 400 `InvalidParameterValue` with the message `The value <name> for parameter ExtendedStatistics.member.<n>.<list element> is not supported.` Fixed text is `The value ` + ` for parameter ` + `.<list element> is not supported.`; the variable parts are the submitted name (from the request) and the member path `ExtendedStatistics.member.<n>` with the same 1-based index.

Emulator: the error code is right and every surrounding leg of the check passes — Statistics=['Average'] reads Average 50.5, ExtendedStatistics=['p90'] reads ~90 with no standard statistic alongside it, and `code == 'InvalidParameterValue'` holds. Only the message diverges: for Statistics=['p90'] it answers `The value p90 for parameter Statistics.member.1 is invalid.` That text is invented — it echoes the submitted value (AWS's message for this error class does not), it omits the allowed-value set entirely, and it drops the `.<list element>` suffix from the member path. The sibling leg is not reached by this replay (the check aborts at the Statistics message assertion), but the handler source shows the same defect for it: `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_metric_statistics.rs` builds the ExtendedStatistics refusal as `invalid_value(format!("{name} is not a supported statistic"))`, i.e. `Average is not a supported statistic`, not AWS's `The value Average for parameter ExtendedStatistics.member.1.<list element> is not supported.`

Why: these two messages are the only thing that tells a caller WHICH field a statistic name belongs in. AWS's Statistics message enumerates the five legal names, so a client (or an operator reading a console/CLI error) can repair the request; the emulator's message says only that the value "is invalid" and names no legal value, so the repair is unguessable. Clients and test suites that string-match on AWS's refusal text — to route a percentile into ExtendedStatistics automatically, or to assert the refusal in CI — never match, and an emulator-based test that is written against the emulator's text asserts a sentence real AWS never sends.

Fix: the statistic-name validation and its refusal messages for GetMetricStatistics. The `Statistics` leg is refused by the generic enum-constraint path of the request walk, which emits a uniform "The value X for parameter P is invalid." sentence; CloudWatch's wording for an enum list member must be produced instead, from the member path plus the enum's own value set. The `ExtendedStatistics` leg is refused inside the handler's own pre-read loop (see Traces for the file and lines) and must carry AWS's `is not supported.` wording with the `.<list element>` member path.

Traces: cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected. Handler: services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_metric_statistics.rs lines 228-233 (Statistics collected with no name validation of its own), lines 253-261 (the ExtendedStatistics refusal text), lines 349-357 (`invalid_value`, the InvalidParameterValue constructor).

## Evidence
Failing check cloudwatch.metrics-math-and-emf-statistic-and-extended-statistic-fields-not-interchangeable-rejected, verdict mismatch:

  File "check_cloudwatch_metrics_math_and_emf_statistic_and_extended_statistic_fields_not_interchangeable_rejected.py", line 57, in run
    assert message == (
  AssertionError: The value p90 for parameter Statistics.member.1 is invalid.

The asserted expectation at that line is:
  assert message == (
      "The parameter Statistics.member.1.<list element> must be a value in the set "
      "[SampleCount, Average, Sum, Minimum, Maximum]."
  ), message

Expected (AWS): HTTP 400, Error.Code `InvalidParameterValue`, Error.Message `The parameter Statistics.member.1.<list element> must be a value in the set [SampleCount, Average, Sum, Minimum, Maximum].`
Actual (emulator): HTTP 400, Error.Code `InvalidParameterValue`, Error.Message `The value p90 for parameter Statistics.member.1 is invalid.`

Assertions that PASSED immediately before, which localize the failure to the message alone:
  * `assert [d["Average"] for d in avg_dps] == [50.5]` — Statistics=['Average'] over 100 published Values reads correctly.
  * `assert 88.0 <= p90 <= 92.0` and `assert "Average" not in p90_dps[0]` — ExtendedStatistics=['p90'] reads ~90 and returns no standard statistic alongside it.
  * `assert code == "InvalidParameterValue"` for Statistics=['p90'] — the error CODE is already correct (this is what the fixed issue 20261025T020208Z-cloudwatch-metric-read-path-unit-is-dropped-from.md cause 4 changed from the unmodeled ValidationException).

Not reached by this replay, same cause, from the handler source: the next leg asserts ExtendedStatistics=['Average'] is refused with `The value Average for parameter ExtendedStatistics.member.1.<list element> is not supported.`, while get_metric_statistics.rs:253-261 emits `invalid_value(format!("{name} is not a supported statistic"))` → `Average is not a supported statistic`.

## Steps to Reproduce
1. cloudwatch:PutMetricData into a fresh namespace: Namespace=<ns>, MetricData=[{MetricName: 'Dist', Timestamp: <a closed whole minute ~3 min ago>, Values: [1.0 .. 100.0]}].
2. Positive controls (pass today):
   a. cloudwatch:GetMetricStatistics(Namespace=<ns>, MetricName='Dist', StartTime=<ts>, EndTime=<ts+60s>, Period=60, Statistics=['Average']) -> one datapoint with Average 50.5.
   b. same call with ExtendedStatistics=['p90'] instead -> one datapoint whose ExtendedStatistics['p90'] is ~90 and which carries no 'Average' key.
3. cloudwatch:GetMetricStatistics(Namespace=<ns>, MetricName='Dist', StartTime=<ts>, EndTime=<ts+60s>, Period=60, Statistics=['p90']).
   Expected: HTTP 400, Error.Code 'InvalidParameterValue', Error.Message exactly 'The parameter Statistics.member.1.<list element> must be a value in the set [SampleCount, Average, Sum, Minimum, Maximum].'
   Observed: HTTP 400, Error.Code 'InvalidParameterValue', Error.Message 'The value p90 for parameter Statistics.member.1 is invalid.'
   (Place the bad name second — Statistics=['Average','p90'] — and the member path must become 'Statistics.member.2.<list element>'; the index comes from the request, the rest of the sentence is fixed.)
4. cloudwatch:GetMetricStatistics(..., ExtendedStatistics=['Average']).
   Expected: HTTP 400, Error.Code 'InvalidParameterValue', Error.Message exactly 'The value Average for parameter ExtendedStatistics.member.1.<list element> is not supported.'
   Observed (from the handler source; this leg is not reached while step 3 still fails): Error.Message 'Average is not a supported statistic'.
5. Final control (passes today): cloudwatch:GetMetricData with MetricDataQueries=[{Id:'avg', MetricStat:{Metric:{Namespace:<ns>,MetricName:'Dist'}, Period:60, Stat:'Average'}}, {Id:'p90', MetricStat:{..., Stat:'p90'}}] -> GetMetricData's single Stat field accepts both kinds of name and returns [50.5] and the same p90.
