Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120|_poll_until#33|Timeout|time.sleep(interval_s)
Issue-Commit: b65c35df18604e01161a221667fc419ec04636e8

# cloudwatch: a SlidingWindow (default) metric alarm never transitions inside the open period — alarm evaluation grades only closed, period-anchored windows and still ignores EvaluationWindow

> _Note: this issue was identified against the code base at commit `b65c35df18604e01161a221667fc419ec04636e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause. The replay report did not mark this check as OWNED by any open issue: its replay key has changed from the one recorded before (it no longer fails at the wall-clock assertion, it now hangs), so this is a distinct, net-new failure mode of the alarm evaluator.

Note on existing issues, none of which covers this cause: 20261028T085300Z-cloudwatch-ignores-evaluationwindow-wallclockwindow-a-wall-clock-alarm.md (open) describes the OPPOSITE direction of the same member — a WallClockWindow alarm that fired too EARLY, on a rolling window, with the recorded failure being `assert wc_mid["StateValue"] == "INSUFFICIENT_DATA"`; in this replay no assertion is reached at all because the SLIDING (default) twin never alarms. 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md and 20261028T224444Z-cloudwatch-7-net-new-root-causes-high-resolution.md cover alarms that never evaluate at all / high-resolution and metric-math alarms; here a plain Period=300 Maximum alarm DOES evaluate, but only once its clock-aligned period has closed, which is the defect.

Cause 1 of 1: metric-alarm evaluation grades only fully closed, period-anchored windows for EVERY alarm, so an alarm using the default SlidingWindow cannot leave INSUFFICIENT_DATA while the current period is still open, and the two EvaluationWindow kinds remain indistinguishable.
Failing check id: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120

AWS: Smithy (com.amazonaws.cloudwatch) models PutMetricAlarmInput.EvaluationWindow as the union EvaluationWindow{SlidingWindow, WallClockWindow}, documented "Set exactly one of its members ... If you don't set EvaluationWindow, the alarm uses a SlidingWindow by default." SlidingWindow is documented as "An evaluation window that advances each time the alarm is evaluated, forming a rolling time window. This is the default evaluation window." and "Choose a sliding window when you need the fastest detection and the calendar boundaries of the data don't matter"; WallClockWindow "aligns the evaluated range to fixed clock boundaries that match the alarm's period". The behaviour that follows, and that the check measures, is that a sliding-window alarm evaluates a ROLLING range ending at the evaluation instant: a datapoint published inside the current period is graded on the next evaluation, within the period, so the alarm reaches ALARM BEFORE the clock boundary floor(t/Period)*Period + Period passes; its StateReasonData.startDate is minute-aligned and reaches back BEFORE that boundary, StateReasonData.queryDate is before the boundary close, and that startDate must DIFFER from a wall-clock twin's startDate (which is exactly the boundary). Only the wall-clock arm is gated on the aligned period closing. Neither arm reports an EvaluationState member.

Emulator: alarm evaluation takes the newest window it grades as the period BEFORE the one the evaluation instant falls in, unconditionally and with no branch on the stored EvaluationWindow — services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state/alarm_evaluation.rs defines `fn newest_graded_period(now_secs, period) -> (now_secs / period).floor() - 1.0`, and the module mentions WallClockWindow/SlidingWindow nowhere. For a Period=300, EvaluationPeriods=1, DatapointsToAlarm=1, Maximum/GreaterThanThreshold alarm over a breaching datapoint published inside the open 300 s period, the breaching window only becomes gradeable once the clock has crossed the next 300 s boundary — i.e. exactly when the aligned window closes. So the default SlidingWindow twin stays INSUFFICIENT_DATA for the whole open period and the check's poll for it to reach ALARM before the boundary close can never succeed; the check hangs and is cut at the 60 s harness cap instead of reaching any assertion. By the same code path a WallClockWindow alarm and a SlidingWindow alarm compute the identical range, so the check's later requirement that the two startDates differ also cannot hold.

Why: the sliding window is the DEFAULT, so this is the behaviour of every ordinary metric alarm. A client that publishes a breaching datapoint and waits for its alarm sees detection deferred until the next Period boundary (up to a full period of extra latency, and for Period=86400 up to a day) instead of "the fastest detection" AWS documents; alarm-driven automation (scaling, rollback, paging, deployment gates) therefore does not fire when production fires. StateReasonData is wrong in the same way: startDate/evaluatedDatapoints sit on a period boundary rather than on a minute-aligned rolling range, and queryDate never precedes the boundary close, so a client reconciling an alarm's decision against GetMetricStatistics reads the wrong evaluated range. Finally, because the two union members take the same path, choosing WallClockWindow buys nothing and no test can distinguish the two windows.

Fix: the CloudWatch alarm evaluation engine in the cloudwatch service crate (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state/alarm_evaluation.rs — the look-back/windowing used by the metric-alarm evaluation and the StateReasonData it writes) must branch on the alarm's stored EvaluationWindow: for SlidingWindow (and for an absent EvaluationWindow, which defaults to it) grade a rolling range that ends at the evaluation instant with a minute-aligned start, so a datapoint in the still-open period is evaluated; keep the closed-aligned-period gating only for WallClockWindow, aligned in that window's Timezone (default UTC). The PutMetricAlarm handler's input validation is NOT at fault — the Period=120 + WallClockWindow refusal (HTTP 400, ValidationError, fixed message "Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week.") and the Period=120 + SlidingWindow acceptance both pass in this replay.

Traces: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120 (mismatch; "Check timed out after 60.0s" while polling DescribeAlarms for the SlidingWindow twin to reach ALARM before its wall-clock twin's aligned window closed).

## Evidence
Failing check: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120 [mismatch]
Replay output: "Error: Check timed out after 60.0s" — no assertion was reached; the check is blocked in its poll loop.

Expected (real AWS): after PutMetricData publishes a breaching datapoint (Value 100.0, Unit Count) inside an open 300 s UTC period, the twin alarm created with EvaluationWindow={"SlidingWindow":{}} (Period=300, EvaluationPeriods=1, DatapointsToAlarm=1, Statistic=Maximum, Threshold=10.0, ComparisonOperator=GreaterThanThreshold) reaches StateValue "ALARM" BEFORE the boundary floor(t/300)*300 + 300, with StateReasonData.period == 300, queryDate < that close, startDate minute-aligned and < the boundary, and evaluatedDatapoints timestamps == [startDate]; the WallClockWindow twin is still "INSUFFICIENT_DATA" at that instant and only alarms after the close, from startDate == the boundary; the two startDates differ; neither alarm carries EvaluationState.

Actual (emulator): the poll on the SlidingWindow twin never observes ALARM — its own deadline is the time remaining until the wall-clock window closes, and the emulator can first grade the breaching window only once the clock has crossed that same boundary, so the check runs out of time and is cut at the 60 s cap. The concrete alarm names, namespace and timestamps in the check are per-run values; the requirement is the relation — a default/sliding alarm transitions on a rolling range inside the open period, and its evaluated range differs from a wall-clock twin's.

Code grounding for the cause (file/line citations only, not a patch): services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state/alarm_evaluation.rs line 415, `fn newest_graded_period(now_secs: f64, period: f64) -> f64 { (now_secs / period).floor() - 1.0 }`, documented there as "the window an evaluation counts back from is the one BEFORE the period the evaluation instant falls in" — applied to every metric alarm. Neither `EvaluationWindow` nor `WallClockWindow`/`SlidingWindow` appears anywhere in that module, while services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_metric_alarm.rs does store and validate the member (WALL_CLOCK_PERIODS / WALL_CLOCK_PERIOD_REFUSAL, ~lines 435-491).

## Steps to Reproduce
Boot the emulator and point a botocore cloudwatch client at it.
1. Pick a unique custom namespace NS and metric name M (CloudWatch has no DeleteMetric).
2. Wait until the UTC wall clock is 5-150 seconds past a 300-second boundary, so the aligned period is still open.
3. PutMetricData(Namespace=NS, MetricData=[{MetricName: M, Timestamp: now, Value: 100.0, Unit: "Count"}]).
4. Create two alarms over NS/M with identical settings — Statistic=Maximum, Period=300, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, ActionsEnabled=false — differing only in EvaluationWindow: SL with {"SlidingWindow":{}} and WC with {"WallClockWindow":{"Timezone":"UTC"}}.
5. Derive the boundary from WC's AlarmConfigurationUpdatedTimestamp: boundary = floor(ts/300)*300, closes = boundary + 300.
6. Keep republishing the breaching datapoint and poll DescribeAlarms(AlarmNames=[SL], AlarmTypes=["MetricAlarm"]) until the instant `closes`.
   Expected: SL reports StateValue "ALARM" before `closes`, with StateReasonData.startDate minute-aligned and earlier than `boundary`, queryDate < `closes`, and evaluatedDatapoints == [startDate].
   Observed: SL stays "INSUFFICIENT_DATA" for the whole open period; it can only be graded once the clock passes `closes`, so the wait expires (in the suite, at the 60 s harness cap).
7. After `closes`, compare StateReasonData.startDate of SL and WC: AWS gives different ranges (WC's startDate == boundary, SL's < boundary); the emulator computes the same period-anchored range for both.

Or replay the check id in trace_ids.
