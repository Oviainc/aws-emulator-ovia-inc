Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period|run#35|AssertionError|assert len(hires_srd["recentDatapoints"]) == _HIRES_N, (
Issue-Key: cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m|run#25|AssertionError|f"minimum {_N - _M + 1} datapoints for ALARM -> OK transition" in ok["StateReason"]
Issue-Commit: d354604e0ab1934a4bfe31b4f8651346efb6794f

# CloudWatch alarm evaluator: a Period=10 high-resolution alarm's decision window is not EvaluationPeriods wide, and an evaluated OK's StateReason reports the breaching count instead of the ALARM->OK predicate (2 causes)

> _Note: this issue was identified against the code base at commit `d354604e0ab1934a4bfe31b4f8651346efb6794f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Two net-new root causes in the CloudWatch emulator's alarm evaluator, both reached only AFTER an evaluation has actually run. They are therefore NOT covered by 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md (which reports that no evaluation happens at all): in this replay the alarms DO transition from published data, and even the sub-minute ordering/latency assertions pass. What diverges is the content of the decision the evaluator emits.

=== Cause 1 of 2: a high-resolution (Period=10) alarm's decision record does not hold one 10-second bucket per evaluation period ===
Failing check id: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period.
AWS: Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model) models PutMetricAlarm.Period only as integer @range{min:1}, but its own documentation fixes the lattice and the semantics: "The length, in seconds, used each time the metric specified in MetricName is evaluated. Valid values are 10, 20, 30, and any multiple of 60. ... Specifying 10, 20, or 30 also sets this alarm as a high-resolution alarm". DatapointsToAlarm is documented as the M of an "M out of N" alarm and EvaluationPeriods as the N. MetricAlarm.StateReasonData (@length 0..4000, "An explanation for the alarm state, in JSON format") is recorded behavior only; measured on real AWS it carries period, recentDatapoints (one value per evaluation period) and evaluatedDatapoints (each {timestamp, value, sampleCount}). For Period=10 / EvaluationPeriods=3 / DatapointsToAlarm=3 real AWS reported recentDatapoints of exactly 3 values and evaluatedDatapoints whose timestamps are spaced exactly 10 seconds apart — three consecutive 10-second buckets.
Emulator: the Period=45 refusal is correct (ValidationError naming the legal periods), both alarms are created INSUFFICIENT_DATA, the Period=10 alarm does fire before the Period=60 control and inside the sub-minute latency bound, and StateReasonData.period correctly reads 10. But the decision window is short: StateReasonData.recentDatapoints held only [99.0, 99.0] where a 3-period window requires 3 values. The evaluator therefore does not materialize one bucket per period at sub-minute resolution — it grades a window narrower than EvaluationPeriods.
Why: an M-of-N predicate over a window that is not N wide cannot be evaluated correctly. A client configuring a 3-of-3 high-resolution alarm gets a verdict computed from 2 datapoints, so the alarm fires (or holds) on a different condition than AWS would, and anything reconstructing the decision from StateReasonData sees a window that contradicts the alarm's own EvaluationPeriods. Sub-minute alarming is the whole point of high-resolution metrics, so this silently breaks the fast-detection use case the configuration was chosen for.
Fix: the alarm evaluator's window-assembly component for sub-minute periods in the cloudwatch service crate — the code that lays EvaluationPeriods buckets of Period seconds over the metric store and then renders them into StateReasonData.recentDatapoints / evaluatedDatapoints. No existing open issue names this check id; the nearest, 20261025T053543Z-cloudwatch-alarm-evaluation-and-metric-read-fidelity-statereason.md, reports MISSING StateReasonData keys for other checks, whereas here the keys are present and the window is the wrong width.
Traces: cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period.

=== Cause 2 of 2: an evaluated OK's StateReason reports the breaching count and the ALARM-direction requirement instead of the ALARM->OK predicate and its minimum ===
Failing check id: cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m.
AWS: Smithy models MetricAlarm.StateReason as StateReason @length 0..1023, "An explanation for the alarm state, in text format" — the wording is recorded behavior only. Measured on real AWS for an M-of-N alarm that deliberately stays OK (DatapointsToAlarm=3 of EvaluationPeriods=5), the reason reports the datapoints that satisfied the OK predicate — the NON-breaching ones — and names the direction and its minimum, which for ALARM->OK is EvaluationPeriods - DatapointsToAlarm + 1, not DatapointsToAlarm. The rule is built entirely from the request: the numerator is the count of datapoints satisfying the direction being reported, the denominator is EvaluationPeriods, and the trailing clause states "minimum <EvaluationPeriods - DatapointsToAlarm + 1> datapoints for ALARM -> OK transition" (a 5-of-5 control's OK reason says minimum 1). Both integers of the configuration are recoverable from that one string.
Emulator: the state itself is right (OK) and recentDatapoints now holds the correct 5-value window with exactly 2 breaching values, but the reason reports the ALARM-direction view: 'Threshold Crossed: 2 out of the last 5 datapoints satisfied GreaterThanThreshold with the threshold (10), fewer than the 3 datapoints required for the alarm to be in ALARM state.' The numerator is the breaching count (2) rather than the count that satisfied the OK predicate (3), and the ALARM -> OK transition minimum clause is absent, so the OK-direction minimum is not stated at all.
Why: a client cannot read which direction was evaluated or what the OK threshold of datapoints was, so it cannot distinguish "held OK because 3 of 5 were clean" from "has not reached M yet", and cannot recover DatapointsToAlarm/EvaluationPeriods from the reason the way AWS permits. Runbooks and incident tooling that parse StateReason to attribute a verdict to a count get the complement of the number AWS reports.
Fix: the alarm evaluator's StateReason renderer (the direction-aware reason text: numerator = datapoints satisfying the reported direction, plus the transition-minimum clause). Related and open: 20261025T053543Z-cloudwatch-alarm-evaluation-and-metric-read-fidelity-statereason.md cause 1 already lists this check id in its Issue-Trace-Ids and describes the missing transition-minimum clause; the replay no longer matches that issue's recorded Issue-Key (the KeyError on recentDatapoints is gone and the 'Threshold Crossed' prefix is now present), so the report did not mark this check owned. This entry records the CURRENT residual signature so the remaining gap is not closed as fixed.
Traces: cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the built emulator: 88/409 passed.

Cause 1 — cloudwatch.high-resolution-alarm-evaluates-ten-second-periods-and-rejects-invalid-period [mismatch].
Passed first: PutMetricAlarm Period=45 refused with code ValidationError and a message containing "Period must be 10, 20, 30 or a multiple of 60" and no alarm left behind; all three alarms created INSUFFICIENT_DATA on an unwritten metric; the Period=10 alarm reached ALARM before the Period=60 control and within the 150 s sub-minute bound; StateReasonData.period == 10.
Expected: len(StateReasonData["recentDatapoints"]) == 3 for EvaluationPeriods=3 at Period=10, every value the published 99.0, and evaluatedDatapoints timestamps spaced exactly 10.0 s apart.
Actual: AssertionError: "the engine compared [99.0, 99.0]; a 3-period window means 3 10-second buckets, not one minute bucket" (check body line 209).

Cause 2 — cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m [mismatch].
Passed first: DescribeAlarms round-tripped (EvaluationPeriods, DatapointsToAlarm) == (5, 3); the alarm was created INSUFFICIENT_DATA; it reached an evaluated OK; StateReasonData.recentDatapoints held 5 values aggregating to two 99.0 and three 1.0, i.e. exactly 2 breaching — one short of M.
Expected: the OK verdict's StateReason contains "minimum 3 datapoints for ALARM -> OK transition" (EvaluationPeriods - DatapointsToAlarm + 1 = 3) and "out of the last 5 datapoints".
Actual: AssertionError — StateReason was 'Threshold Crossed: 2 out of the last 5 datapoints satisfied GreaterThanThreshold with the threshold (10), fewer than the 3 datapoints required for the alarm to be in ALARM state.' (check body line 173). The numerator is the breaching count, and no ALARM -> OK transition minimum is stated.

## Steps to Reproduce
Cause 1 (high-resolution window width):
1. cloudwatch:PutMetricData into a fresh custom namespace with StorageResolution=1, publishing the value 99.0 roughly once per second.
2. cloudwatch:PutMetricAlarm AlarmName=hires, Namespace=<ns>, MetricName=<m>, Statistic=Maximum, Period=10, EvaluationPeriods=3, DatapointsToAlarm=3, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, ActionsEnabled=false; and a control AlarmName=standard with Period=60, EvaluationPeriods=1, DatapointsToAlarm=1.
3. Confirm the negative arm still holds: the same PutMetricAlarm with Period=45 is refused ValidationError "Period must be 10, 20, 30 or a multiple of 60" and creates nothing.
4. Poll cloudwatch:DescribeAlarms until hires reports ALARM, then json.loads(StateReasonData).
5. Expected: period == 10, len(recentDatapoints) == 3 all equal 99.0, and the 3 evaluatedDatapoints timestamps spaced exactly 10 s apart. Observed: recentDatapoints == [99.0, 99.0] (2 values for a 3-period window).

Cause 2 (OK-direction StateReason):
1. cloudwatch:PutMetricData into a fresh custom namespace, one datapoint per minute over the last 5 minutes, values (oldest..newest) 1.0, 1.0, 1.0, 99.0, 99.0, Unit=Count.
2. cloudwatch:PutMetricAlarm Statistic=Maximum, Period=60, EvaluationPeriods=5, DatapointsToAlarm=3, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, TreatMissingData=notBreaching, ActionsEnabled=false; re-publish 1.0 at the current minute each poll pass to keep the window hole-free.
3. Poll cloudwatch:DescribeAlarms until StateValue == "OK" (2 of 5 breaching is one short of M, so OK is the correct verdict and the emulator does reach it).
4. Read StateReason. Expected it to contain "out of the last 5 datapoints" and "minimum 3 datapoints for ALARM -> OK transition", with the numerator being the 3 non-breaching datapoints that satisfied the OK predicate. Observed numerator 2 (the breaching count) and no transition-minimum clause.
5. Re-replay both check ids in trace_ids to confirm the fix.
