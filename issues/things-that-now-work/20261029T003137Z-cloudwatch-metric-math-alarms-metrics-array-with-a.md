Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value cloudwatch.alarm-math-and-anomaly-raw-input-breach-does-not-fire-expression-alarm
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value|_poll_until#21|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; "
Issue-Key: cloudwatch.alarm-math-and-anomaly-raw-input-breach-does-not-fire-expression-alarm|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: e801dd13e6dc58f681efedb1366f7c686b5e4d9a

# cloudwatch: metric-math alarms (Metrics array with a ReturnData expression) are evaluated at most once and never advance on later datapoints — no OK -> ALARM transition when the expression crosses the threshold (2 causes)

> _Note: this issue was identified against the code base at commit `e801dd13e6dc58f681efedb1366f7c686b5e4d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
TWO net-new root causes in the CloudWatch alarm evaluation engine, both in the metric-math alarm path (PutMetricAlarm with a `Metrics` array rather than a single MetricName/Statistic). Both are grounded in failing behavioral checks that this replay marked NEITHER 'ALREADY FILED' NOR 'OWNED by' any open issue. The bundle carries exactly two causes, numbered 'Cause 1 of 2' and 'Cause 2 of 2'; both must be closed before this issue is submitted.

Related but NOT covering: the open issue 20261028T224444Z-cloudwatch-7-net-new-root-causes-high-resolution.md describes high-resolution (sub-minute Period) alarms and several other evaluator defects, and the open issue 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md describes alarms that never evaluate at all. Neither carries the replay keys of the two checks below, and neither matches the symptom here: in the metric-math path the evaluator DOES produce a first, correctly computed verdict and then stops advancing. Also related: 20261026T031713Z-cloudwatch-2-net-new-root-causes-putmetricalarm-refuses.md (anomaly-band alarms refused) — a different code path.

=== Cause 1 of 2: an IF() metric-math alarm renders a correct first verdict, then never re-evaluates, so it never transitions OK -> ALARM when the expression output crosses the threshold ===
Failing check id: cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value

AWS: Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801) models PutMetricAlarm.Metrics as a list of MetricDataQuery. MetricDataQuery documents that in PutMetricAlarm 'one must have true as the value for ReturnData. The result of this expression is the value the alarm watches', with Expression (@length 1..2048) holding 'a metric math expression to be performed on the returned data'. MetricAlarm.StateValue targets the StateValue enum, StateReason is @length max 1023 ('An explanation for the alarm state, in text format'), StateReasonData @length max 4000 ('An explanation for the alarm state, in JSON format'), and StateTransitionedTimestamp is 'The date and time that the alarm's StateValue most recently changed'. The evaluation cadence itself is recorded behavior only: AWS re-evaluates the alarm once per Period, grading the ReturnData expression's value for each newly closed period, so an alarm with Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, ComparisonOperator=GreaterThanThreshold and Threshold=500 moves OK -> ALARM on the first closed minute whose expression output exceeds 500, publishes a matching StateUpdate item to DescribeAlarmHistory (HistoryData oldState.stateValue = the previous state, newState.stateReasonData.recentDatapoints = the EXPRESSION's values, not the raw inputs), and keeps doing so for every subsequent period — a single evaluation is never terminal.

Emulator: the first evaluation is right and every later one is absent. For an alarm over Metrics = [m1 = MetricStat(Load, Period 60, Stat Minimum, ReturnData false), e1 = Expression 'IF(m1 > 100, 0, 1000)', ReturnData true] with Threshold 500 / GreaterThanThreshold / EvaluationPeriods 1 / DatapointsToAlarm 1 / TreatMissingData 'missing', the emulator correctly leaves INSUFFICIENT_DATA and reports StateValue 'OK' with StateReason 'Threshold Crossed: 1 out of the last 1 datapoints [<value> (<dd/mm/yy HH:MM>)] was not greater than the threshold (<threshold>) (minimum <DatapointsToAlarm> datapoint for ALARM -> OK transition).' and StateReasonData citing the IF output (the fixed text is AWS's; the bracketed datapoint value/timestamp, the threshold and the datapoint count are the variable parts taken from the request and the stored series). The check then publishes a NEW datapoint into the following minute whose Minimum makes IF(m1 > 100, 0, 1000) evaluate to 1000 — above the threshold — and republishes it on every poll. The emulator never evaluates that minute: DescribeAlarms keeps returning the stale verdict (StateValue 'OK', StateReasonData.recentDatapoints [0.0], evaluatedDatapoints a single datapoint stamped with the FIRST minute, StateUpdatedTimestamp frozen at the first evaluation) until the check's wait is exhausted ('AssertionError: alarm ... fired once m1=50.0 made the IF output 1000.0 did not happen within 31.8s'). No OK -> ALARM transition and no second StateUpdate history item is ever produced.

=== Cause 2 of 2: a ratio metric-math alarm ((m1/m2)*100 over two MetricStat inputs) produces no terminal verdict at all ===
Failing check id: cloudwatch.alarm-math-and-anomaly-raw-input-breach-does-not-fire-expression-alarm

AWS: same modeled surface as Cause 1 — PutMetricAlarm.Metrics with two MetricStat members (ReturnData false) and one Expression member '(m1/m2)*100' (ReturnData true). Per MetricDataQuery's documentation the alarm watches ONLY the expression result, so with Threshold 50 / GreaterThanThreshold the alarm must be OK while the expression is 20 even though both raw inputs (80 and 400) are above 50, and must then reach ALARM from OK once the expression reaches 75; the ALARM verdict's StateReasonData.evaluatedDatapoints and the DescribeAlarmHistory StateUpdate item's newState.stateReasonData.recentDatapoints must cite the computed expression value, never a raw input. Recorded behavior only for the cadence: AWS reaches each of those verdicts within a small number of Period(=60 s) ticks of the data being published.

Emulator: no verdict is ever observable. The check publishes Errors/Requests into the current minute, creates the alarm and polls DescribeAlarms, republishing each iteration; it never satisfies even the first predicate (StateValue != 'INSUFFICIENT_DATA' for leg A, then StateValue == 'ALARM' for leg B) and is killed at the suite's harness cap ('Error: Check timed out after 60.0s'). Nothing downstream is produced: no OK verdict citing 20, no ALARM verdict citing 75, no StateUpdate history item whose oldState is OK, and no agreement between the evaluator and GetMetricData over the same Metrics array.

Why: a client that builds an alarm on a metric math expression — the documented way to alarm on an error RATE, a ratio, or an IF()/transformed series — gets an alarm that either freezes on its first computed verdict or never leaves INSUFFICIENT_DATA. It therefore never fires, so every consumer gated on it (SNS/paging, CodeDeploy and AppConfig rollback monitors, Application Auto Scaling target tracking, EventBridge alarm-state-change routing) silently never runs, and the client cannot distinguish 'the ratio is healthy' from 'the alarm stopped evaluating' because DescribeAlarms keeps serving a plausible, stale OK with a stale StateUpdatedTimestamp.

Fix: the CloudWatch alarm evaluation engine in the cloudwatch service crate — specifically the periodic evaluation tick for alarms configured with a Metrics array (the metric-math/ReturnData-expression path), which must re-run for every closed Period and not latch after its first decision, plus the StateValue/StateReason/StateReasonData and DescribeAlarmHistory StateUpdate rendering it drives. The expression arithmetic itself is already correct for the first evaluation (Cause 1 shows IF() computed right), so the defect is in the scheduling/advance of the evaluator over the stored series, not in the math.

## Evidence
Cause 1 — cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value
Expected (AWS): after a datapoint whose Minimum makes IF(m1 > 100, 0, 1000) evaluate to 1000 is published, the alarm transitions OK -> ALARM; DescribeAlarms reports StateValue 'ALARM' with StateReasonData.evaluatedDatapoints citing a single datapoint of value 1000.0 stamped with that minute, and DescribeAlarmHistory gains a StateUpdate item whose HistoryData oldState.stateValue is 'OK' and newState.stateReasonData.recentDatapoints is [1000.0].
Actual (emulator): DescribeAlarms keeps returning the previous minute's verdict unchanged for the whole wait —
  StateValue: 'OK'
  StateReason: "Threshold Crossed: 1 out of the last 1 datapoints [0.0 (29/10/26 00:18:00)] was not greater than the threshold (500.0) (minimum 1 datapoint for ALARM -> OK transition)."
  StateReasonData: {"version":"1.0","queryDate":"...","startDate":"2026-10-29T00:18:00.000+0000","period":60.0,"threshold":500.0,"recentDatapoints":[0.0],"evaluatedDatapoints":[{"timestamp":"2026-10-29T00:18:00.000+0000","value":0.0,"datapoint":"notBreaching"}]}
  StateUpdatedTimestamp frozen at the first evaluation; Metrics read back correctly (e1 Expression 'IF(m1 > 100, 0, 1000)', ReturnData true).
Failure: AssertionError: alarm '<name>' fired once m1=50.0 made the IF output 1000.0 did not happen within 31.8s. (The values 50.0/1000.0/500.0/0.0 and the minute stamps are this run's fill of the template; the requirement is that a newly closed period is graded at all.)

Cause 2 — cloudwatch.alarm-math-and-anomaly-raw-input-breach-does-not-fire-expression-alarm
Expected (AWS): leg A — with Errors=80, Requests=400 (both above Threshold 50) the expression (m1/m2)*100 is 20, so StateValue is 'OK' and StateReasonData.evaluatedDatapoints cites 20.0 (never 80 or 400), and no ALARM StateUpdate exists. Leg B — with Errors=300 the expression is 75, so the alarm reaches 'ALARM' from 'OK' and the StateUpdate item's newState.stateReasonData.recentDatapoints is [75.0].
Actual (emulator): the alarm never leaves INSUFFICIENT_DATA / never reaches ALARM within the run; the check is terminated by the harness — 'Error: Check timed out after 60.0s'. No OK verdict, no ALARM verdict and no StateUpdate item is ever served for this alarm.

Shared signal: the only metric-math alarm evaluation the emulator ever performs is the first one (Cause 1); every alarm that needs a second, later-period evaluation — or any evaluation at all in Cause 2's two-input shape — gets none.

## Steps to Reproduce
Cause 1:
1. PutMetricData into a fresh namespace: MetricName 'Load', Unit 'Count', Timestamp = now truncated to the minute, Value 700.
2. PutMetricAlarm with Metrics = [{Id:'m1', MetricStat:{Metric:{Namespace:<ns>, MetricName:'Load'}, Period:60, Stat:'Minimum'}, ReturnData:false}, {Id:'e1', Expression:'IF(m1 > 100, 0, 1000)', Label:'Inverted', ReturnData:true}], Threshold=500, ComparisonOperator='GreaterThanThreshold', EvaluationPeriods=1, DatapointsToAlarm=1, TreatMissingData='missing', ActionsEnabled=false.
3. Poll DescribeAlarms (AlarmTypes=['MetricAlarm']) until StateValue != 'INSUFFICIENT_DATA' — observe 'OK' with StateReasonData citing the IF output 0.0. Correct.
4. PutMetricData the same metric with Value 50 into the CURRENT minute and keep republishing it.
5. Poll DescribeAlarms for StateValue == 'ALARM'. Expected: ALARM, StateReasonData citing 1000.0 for the new minute, and a DescribeAlarmHistory StateUpdate item with oldState 'OK' and recentDatapoints [1000.0]. Observed: the step-3 verdict is returned unchanged indefinitely (StateUpdatedTimestamp never advances) and no new StateUpdate item appears.

Cause 2:
1. PutMetricData into a fresh namespace, same minute stamp: 'Errors'=80 (Count) and 'Requests'=400 (Count).
2. PutMetricAlarm with Metrics = [{Id:'m1', MetricStat:{Metric:{Namespace:<ns>, MetricName:'Errors'}, Period:60, Stat:'Maximum'}, ReturnData:false}, {Id:'m2', MetricStat:{Metric:{Namespace:<ns>, MetricName:'Requests'}, Period:60, Stat:'Maximum'}, ReturnData:false}, {Id:'e1', Expression:'(m1/m2)*100', Label:'ErrorRate', ReturnData:true}], Threshold=50, ComparisonOperator='GreaterThanThreshold', EvaluationPeriods=1, DatapointsToAlarm=1, TreatMissingData='missing', ActionsEnabled=false.
3. Poll DescribeAlarms while republishing. Expected: StateValue 'OK' with StateReasonData.evaluatedDatapoints citing 20.0. Observed: the alarm stays INSUFFICIENT_DATA and the check is cut off at the 60 s harness cap.
4. Raise 'Errors' to 300 and keep republishing. Expected: OK -> ALARM citing 75.0, plus a StateUpdate history item with oldState 'OK' and recentDatapoints [75.0]. Observed: never reached.
5. Cross-check with GetMetricData over the SAME Metrics array (all ReturnData=true, StartTime = evaluated timestamp, EndTime = +60 s): the query path must compute the same expression value the evaluator cites.
