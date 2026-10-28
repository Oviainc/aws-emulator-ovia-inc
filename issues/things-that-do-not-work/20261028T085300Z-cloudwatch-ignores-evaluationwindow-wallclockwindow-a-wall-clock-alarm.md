Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120|run#35|AssertionError|assert wc_mid["StateValue"] == "INSUFFICIENT_DATA", (
Issue-Commit: ed95744dcbaaa36aa6e9433f77a7509d0df7ddaf

# CloudWatch ignores EvaluationWindow.WallClockWindow: a wall-clock alarm evaluates on a rolling window and transitions before its aligned period closes

> _Note: this issue was identified against the code base at commit `ed95744dcbaaa36aa6e9433f77a7509d0df7ddaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause.

Cause 1 of 1: the alarm evaluator stores EvaluationWindow but does not honour it — a metric alarm created with EvaluationWindow={"WallClockWindow":{"Timezone":"UTC"}} is evaluated exactly like a SlidingWindow alarm.
Failing check id: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120

AWS: Smithy (com.amazonaws.cloudwatch) models PutMetricAlarmInput.EvaluationWindow as the union EvaluationWindow with members SlidingWindow and WallClockWindow; its documentation states "Set exactly one of its members ... If you don't set EvaluationWindow, the alarm uses a SlidingWindow by default." SlidingWindow is documented as "a sliding window, which advances each time the alarm is evaluated, forming a rolling time window". WallClockWindow is documented as "An evaluation window that aligns the evaluated range to fixed clock boundaries that match the alarm's period, such as the top of the hour, midnight, or the start of the calendar week, optionally in a specific time zone", and "When you use a wall clock window, the alarm's period must be 1 minute (60 seconds), 5 minutes (300 seconds), 1 hour (3,600 seconds), 1 day (86,400 seconds), or 1 week (604,800 seconds)." The behavioural consequence AWS implements, and which the check measures, is that a wall-clock alarm evaluates only CLOSED, clock-aligned periods: for a given Period P the evaluated range starts exactly at the UTC boundary floor(t/P)*P and the alarm cannot leave INSUFFICIENT_DATA until that boundary + P has passed. In the returned alarm, StateReasonData.startDate equals that boundary, evaluatedDatapoints carries exactly the boundary timestamp, StateReasonData.queryDate and StateUpdatedTimestamp are both at or after the window close, and startDate must differ from the sliding twin's startDate. A SlidingWindow twin on the same series, by contrast, alarms inside the still-open period with a minute-aligned startDate that reaches back BEFORE the boundary. Neither alarm shape reports an EvaluationState member. (The input-validation arm of this rule is already correct in the emulator: Period=120 with a WallClockWindow is refused with HTTP 400, error code ValidationError and message "Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week." — that fixed text is the whole message, it has no variable parts — and the same Period=120 with a SlidingWindow is accepted and reads back Period=120. So the defect is purely in evaluation, not in validation.)

Emulator: with two otherwise identical Period=300, EvaluationPeriods=1, DatapointsToAlarm=1, Maximum/GreaterThanThreshold alarms over one freshly published breaching datapoint — one with WallClockWindow, one with SlidingWindow — the emulator moves BOTH to ALARM while the aligned 300s window is still open. DescribeAlarms for the wall-clock alarm returned StateValue 'ALARM' with StateReason 'Threshold Crossed: 1 out of the last 1 datapoints [100.0 (28/10/26 08:35:00)] was greater than the threshold (10.0) (minimum 1 datapoint for OK -> ALARM transition).' while its own window (boundary .. boundary+300s, derived from the server's AlarmConfigurationUpdatedTimestamp) had not yet closed. AWS requires StateValue 'INSUFFICIENT_DATA' at that moment. The reported reason text is also the generic sliding-window reason, which is consistent with the wall-clock arm never taking a distinct evaluation path.

Why: a client that selects WallClockWindow is asking for calendar-aligned alerting — daily reports, batch jobs, backups, dashboard-aligned periods. Against this emulator the alarm fires on partial, unaligned data, so it notifies early and on a different dataset than production: the evaluated range is a rolling window instead of the aligned one, StateReasonData.startDate/evaluatedDatapoints point at the wrong instants, and StateUpdatedTimestamp/queryDate precede the window close. Any test or runbook that distinguishes the two evaluation windows cannot distinguish them here, and alarm-driven automation (scaling, rollback, paging) triggers on data AWS would not yet have evaluated.

Fix: the CloudWatch alarm evaluation engine in the cloudwatch service crate — the evaluator that selects a metric alarm's evaluated range and writes StateValue/StateReason/StateReasonData must branch on the stored EvaluationWindow union: for WallClockWindow, align the range start to floor(now/Period)*Period in the window's Timezone (default UTC), refuse to evaluate until that period has closed, set StateReasonData.startDate and the single evaluatedDatapoints timestamp to that boundary, and set queryDate/StateUpdatedTimestamp at or after the close. SlidingWindow keeps today's rolling, minute-aligned behaviour. Neither path may emit an EvaluationState member. The PutMetricAlarm handler already persists and echoes EvaluationWindow correctly (the alarm reads back EvaluationWindow={"WallClockWindow":{"Timezone":"UTC"}}), so no input-side change is needed.

Traces: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120 (mismatch; fails at the assertion that the wall-clock twin is INSUFFICIENT_DATA while its window is open, after the Period=120 refusal and the SlidingWindow Period=120 control both passed).

## Evidence
Expected (real AWS): DescribeAlarms on the WallClockWindow twin, polled at the moment the SlidingWindow twin reaches ALARM and while the aligned 300s window boundary..boundary+300s is still open, returns StateValue == "INSUFFICIENT_DATA". Only after the window closes does it become ALARM, and then StateReasonData.startDate == the UTC 300s boundary, StateReasonData.evaluatedDatapoints == exactly [that boundary timestamp], StateReasonData.queryDate >= window close, StateUpdatedTimestamp >= window close, and startDate != the sliding twin's startDate. No EvaluationState member on either alarm.

Actual (emulator): AssertionError — "the wall-clock twin reports 'ALARM' (reason 'Threshold Crossed: 1 out of the last 1 datapoints [100.0 (28/10/26 08:35:00)] was greater than the threshold (10.0) (minimum 1 datapoint for OK -> ALARM transition).') while its window 2026-10-28T08:35:00+00:00 .. 2026-10-28T08:40:00+00:00 is still open and its sliding twin has already alarmed; it may evaluate only a closed period". (The concrete timestamps and alarm names are per-run values; the requirement is the relation — no transition before the aligned window closes.)

Passing control arms in the same check, which localise the defect to evaluation: (a) PutMetricAlarm with Period=120 + WallClockWindow is correctly refused with HTTP 400 / ValidationError / "Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week." and leaves no alarm behind; (b) PutMetricAlarm with Period=120 + SlidingWindow is accepted and reads back Period=120 with EvaluationWindow={"SlidingWindow":{}}; (c) the wall-clock Period=300 alarm reads back EvaluationWindow={"WallClockWindow":{"Timezone":"UTC"}}.

## Steps to Reproduce
1. Pick a unique custom namespace NS and metric name M.
2. Wait until the UTC wall clock is 5-150 seconds past a 300-second boundary.
3. PutMetricData(Namespace=NS, MetricData=[{MetricName:M, Timestamp:now, Value:100.0, Unit:"Count"}]).
4. Create two alarms over NS/M with identical settings — Statistic=Maximum, Period=300, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, ActionsEnabled=false — differing only in EvaluationWindow: alarm WC with {"WallClockWindow":{"Timezone":"UTC"}}, alarm SL with {"SlidingWindow":{}}.
5. Keep republishing the breaching datapoint and poll DescribeAlarms(AlarmNames=[SL], AlarmTypes=["MetricAlarm"]) until SL reports StateValue ALARM. This happens before the current 300s UTC boundary closes.
6. Immediately DescribeAlarms(AlarmNames=[WC], AlarmTypes=["MetricAlarm"]).
   Expected: StateValue "INSUFFICIENT_DATA" (the aligned window has not closed).
   Observed: StateValue "ALARM" with the generic sliding-window StateReason.
7. After the boundary closes, inspect WC's StateReasonData: startDate must equal the UTC 300s boundary and evaluatedDatapoints must contain exactly that boundary timestamp, differing from SL's startDate; the emulator produces the same rolling range for both.

Replay: run_behavioral_suite and re-run check cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120.
