Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected cloudwatch.alarm-math-and-anomaly-search-and-undefined-id-rejected cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120 cloudwatch.health-alarm-round-trips-declared-fields-and-omits-treat-missing-data
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected|run#9|AssertionError|assert code == "ValidationError", (
Issue-Key: cloudwatch.alarm-math-and-anomaly-search-and-undefined-id-rejected|_error#11|AssertionError|raise AssertionError(f"real AWS refuses {subject}, but the call SUCCEEDED: {result!r}")
Issue-Key: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period|_error#22|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error to compare")
Issue-Key: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120|_error#8|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it")
Issue-Key: cloudwatch.health-alarm-round-trips-declared-fields-and-omits-treat-missing-data|run#15|AssertionError|assert actions == {"AlarmActions": [], "OKActions": [], "InsufficientDataActions": []}, (
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# PutMetricAlarm accepts four alarm configurations real AWS refuses, and DescribeAlarms omits the empty action lists (5 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Five net-new root causes on the PutMetricAlarm control plane. Smithy: PutMetricAlarmInput { AlarmName @required, Period @range(min 1), EvaluationPeriods @range(min 1), DatapointsToAlarm @range(min 1), Metrics (MetricDataQueries), ThresholdMetricId, EvaluationWindow, WarmUpConfiguration, EvaluationCriteria, EvaluationInterval @range(10,3600), OKActions/AlarmActions/InsufficientDataActions @length(0,5) }; the only modeled error is LimitExceededFault, so every refusal below is a service-level validation AWS performs and the emulator does not.

Cause 1 of 5: an evaluation horizon beyond one day is accepted.
AWS: Period x EvaluationPeriods may not exceed one day (86400 s); Period=60 with EvaluationPeriods=1441 (86460 s) is refused 400 ValidationError.
Emulator: the call SUCCEEDS and the alarm is created.
Why: an alarm AWS would never create exists and appears healthy, so the configuration passes validation in the emulator and fails at deploy time against AWS.
Fix: horizon validation in the PutMetricAlarm handler.
Traces: cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected.

Cause 2 of 5: a metric-math alarm with a bare SEARCH() expression, or referencing an undefined query id, is accepted.
AWS: an alarm's Metrics expression may not be a bare SEARCH() (an alarm must resolve to a single time series) and may not reference an Id that is not defined in the same request; both are refused 400 ValidationError.
Emulator: the call returns HTTP 200 with an empty body.
Why: an unresolvable alarm is created, so a client believes a metric-math alarm is armed when AWS would have rejected the definition.
Fix: metric-math expression validation (SEARCH-bareness and id resolution) in the PutMetricAlarm handler.
Traces: cloudwatch.alarm-math-and-anomaly-search-and-undefined-id-rejected.

Cause 3 of 5: an invalid high-resolution Period is accepted.
AWS: a high-resolution alarm's Period must be 10, 20 or 30 s (or a multiple of 60); other sub-minute values are refused 400 ValidationError.
Emulator: the call succeeds.
Why: the alarm is created with a Period AWS cannot schedule, so evaluation cadence silently differs between the emulator and AWS.
Fix: Period validation in the PutMetricAlarm handler.
Traces: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period.

Cause 4 of 5: a WarmUpConfiguration alarm with Period=120 is accepted.
AWS: the warm-up wall-clock window constrains the permitted Period; Period=120 with a WarmUpConfiguration is refused.
Emulator: the call succeeds.
Why: same as cause 3 — an unschedulable alarm is accepted.
Fix: WarmUpConfiguration/Period cross-validation in the PutMetricAlarm handler.
Traces: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120.

Cause 5 of 5: DescribeAlarms omits AlarmActions / OKActions / InsufficientDataActions when no actions were declared.
AWS: all three ResourceList members are always present in a described MetricAlarm, as empty lists, when no actions were configured.
Emulator: all three are absent, so the client reads {'AlarmActions': None, 'OKActions': None, 'InsufficientDataActions': None}.
Why: a client that iterates the lists has to special-case None, and a read-modify-write of an alarm loses the distinction between "no actions" and "not reported".
Fix: the DescribeAlarms serializer / alarm representation.
Traces: cloudwatch.health-alarm-round-trips-declared-fields-and-omits-treat-missing-data.

## Evidence
Cause 1: "AssertionError: Period=60 x EvaluationPeriods=1441 = 86460 s must be refused with ValidationError, got '<no error: the call SUCCEEDED>' ('the target accepted a configuration real AWS refuses')".
Cause 2: "AssertionError: real AWS refuses a search-bare alarm expression, but the call SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}}}".
Cause 3: "AssertionError: the call succeeded; real AWS rejects it, so there is no error to compare" (check_cloudwatch_high_resolution_alarm_evaluates_ten_second_periods_and_rejects_invalid_period.py:154).
Cause 4: "AssertionError: the call SUCCEEDED; real AWS rejects it" (check_cloudwatch_deep_cloudwatch_1_cw_warmup_wall_clock_window_gates_evaluation_and_rejects_period_120.py:93).
Cause 5: "AssertionError: no actions were declared, so all three lists should be present and empty: {'AlarmActions': None, 'OKActions': None, 'InsufficientDataActions': None}".

## Steps to Reproduce
1. PutMetricAlarm(Period=60, EvaluationPeriods=1441, ...) -> succeeds; AWS refuses 400 ValidationError.
2. PutMetricAlarm(Metrics=[{Id:'e1', Expression:"SEARCH('...')"}]) and PutMetricAlarm(Metrics=[...] referencing an undefined Id) -> both return 200; AWS refuses 400 ValidationError.
3. PutMetricAlarm with a sub-minute Period that is not 10/20/30 -> succeeds; AWS refuses.
4. PutMetricAlarm with WarmUpConfiguration and Period=120 -> succeeds; AWS refuses.
5. PutMetricAlarm with no OKActions/AlarmActions/InsufficientDataActions, then DescribeAlarms -> all three members absent instead of present and empty.
