Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p|run#18|Timeout|time.sleep(20.0)
Issue-Commit: 4c075dcdeacefbec5f942454c2320d1c6eabf26d

# cloudwatch: percentile (ExtendedStatistic) alarms never evaluate and EvaluateLowSampleCountPercentile's 10/(1-p) low-sample floor is not implemented (1 cause)

> _Note: this issue was identified against the code base at commit `4c075dcdeacefbec5f942454c2320d1c6eabf26d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: A metric alarm whose statistic is a percentile (`ExtendedStatistic` = p90/p99) over data published with `PutMetricData` `Values`/`Counts` never evaluates, and the `EvaluateLowSampleCountPercentile` policy (`evaluate` | `ignore`) has no effect on evaluation. Failing check: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p.

AWS: Smithy: PutMetricAlarm.ExtendedStatistic (target ExtendedStatistic) and PutMetricAlarm.EvaluateLowSampleCountPercentile (@length min 1 max 255). The modelled documentation for EvaluateLowSampleCountPercentile states: "Used only for alarms based on percentiles. If you specify `ignore`, the alarm state does not change during periods with too few data points to be statistically significant. If you specify `evaluate` or omit this parameter, the alarm is always evaluated and possibly changes state no matter how many data points are available." / "Valid Values: evaluate | ignore". The AWS-documented rule for "too few data points" is a per-period sample count below 10/(1-p) — 100 samples for p90, 1000 for p99 — not a flat 10 samples (Percentile-Based CloudWatch Alarms and Low Data Samples). So AWS behaviour is: (a) a percentile alarm whose period holds at least 10/(1-p) samples and whose percentile value breaches the threshold transitions INSUFFICIENT_DATA -> ALARM, with StateReasonData carrying `statistic` equal to the configured ExtendedStatistic and one `evaluatedDatapoints` entry whose `sampleCount` is the published count per period and whose `value` is the computed percentile; (b) the same alarm configured `EvaluateLowSampleCountPercentile=evaluate` transitions even below that floor; (c) the same alarm configured `ignore` holds its state and its StateReason unchanged below the floor and records no ALARM StateUpdate in DescribeAlarmHistory; (d) the percentile the alarm withholds is still computable on the query path — GetMetricData with Stat=p99 must return StatusCode `Complete` with the percentile value, and Stat=SampleCount must return the published per-period count.

Emulator: the check is cut off by the harness cap — "Error: Check timed out after 60.0s". The only thing that can block for that long is the check's 420 s poll that waits for the three control alarms (`p90-100-ignore` at exactly the p90 floor of 100 samples, `p90-99-evaluate`, `p99-200-evaluate`) to reach ALARM over a constant 100.0-millisecond series republished every minute with Threshold=10, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1. Everything before that poll (five PutMetricAlarm calls plus DescribeAlarms read-backs) is non-blocking. The emulator therefore leaves percentile alarms in INSUFFICIENT_DATA indefinitely, so neither the floor boundary, the `evaluate` override, the `ignore` withholding, the StateReasonData sampleCount/statistic/value fields, nor the GetMetricData p99/SampleCount leg can be observed.

Related but NOT covering this: open issues 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md, 20261027T035003Z-cloudwatch-3-net-new-root-causes-m-of.md, 20261028T224444Z-cloudwatch-7-net-new-root-causes-high-resolution.md and 20261029T003137Z-cloudwatch-metric-math-alarms-metrics-array-with-a.md describe alarm evaluation for simple-statistic, M-of-N, high-resolution, anomaly-band and metric-math alarms; none of them names ExtendedStatistic/percentile alarms, the EvaluateLowSampleCountPercentile policy or the 10/(1-p) low-sample floor, which is the distinct rule asserted here.

Why: a client that alarms on latency percentiles (the standard pattern for p90/p99 SLOs) gets an alarm that is accepted and read back correctly but never fires, so no notification and no history is produced; and a client that relies on `ignore` to suppress statistically insignificant periods cannot distinguish the suppression from the emulator's blanket non-evaluation. Because the alarm sits in INSUFFICIENT_DATA rather than returning an error, the failure is silent.

Fix: the CloudWatch alarm evaluator — it must compute percentile statistics (ExtendedStatistic pNN) from the stored Values/Counts distribution for each evaluation period, count that period's samples, and gate the state transition on the EvaluateLowSampleCountPercentile policy using the 10/(1-p) threshold; it must also populate StateReasonData's `statistic`, `evaluatedDatapoints[].sampleCount` and `evaluatedDatapoints[].value` for percentile alarms, and GetMetricData must serve pNN and SampleCount for the same series. Belongs in the cloudwatch service crate's alarm-evaluation state/handler and in the GetMetricData statistic computation.

Traces: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p.

## Evidence
Failing check (verdict: mismatch): cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p
Reported detail: "Error: Check timed out after 60.0s"

Expected (AWS): over a constant 100.0 Milliseconds series with Threshold=10, ComparisonOperator=GreaterThanThreshold, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, TreatMissingData=missing:
- `p90-100-ignore` (ExtendedStatistic=p90, 100 samples/period = exactly 10/(1-0.9)) -> StateValue ALARM; StateReasonData.statistic == "p90"; exactly one evaluatedDatapoints entry with sampleCount == 100.0 and value == 100.0
- `p90-99-evaluate` (p90, 99 samples/period, EvaluateLowSampleCountPercentile=evaluate) -> StateValue ALARM; evaluatedDatapoints sampleCount == 99.0
- `p99-200-evaluate` (p99, 200 samples/period, evaluate) -> StateValue ALARM; evaluatedDatapoints sampleCount == 200.0
- `p90-99-ignore` (p90, 99 samples, ignore) and `p99-200-ignore` (p99, 200 samples, ignore) -> StateValue stays INSUFFICIENT_DATA with an unchanged StateReason for the whole hold, and DescribeAlarmHistory(HistoryItemType=StateUpdate) contains no item whose HistoryData.newState.stateValue == "ALARM"
- GetMetricData on the 200-sample series: query Stat=p99 -> StatusCode "Complete" with values == 100.0; query Stat=SampleCount -> values == 200.0 per period

Actual (emulator): the five alarms are created and read back, then the wait for the three control alarms to leave INSUFFICIENT_DATA never completes; the check is killed at the 60 s harness cap, so no percentile alarm ever reaches ALARM. The low-sample floor, the evaluate/ignore differential, the StateReasonData sampleCount/statistic/value fields and the GetMetricData percentile leg are all unobservable.

## Steps to Reproduce
1. PutMetricData into a fresh namespace once per minute, publishing MetricData entries with Values=[100.0] and Counts=[N], Unit=Milliseconds, for N = 99 (metric P90Below), N = 100 (metric P90AtFloor) and N = 200 (metric P99Below).
2. PutMetricAlarm five times against that namespace with Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, TreatMissingData="missing", Unit="Milliseconds", ActionsEnabled=false:
   - MetricName=P90AtFloor, ExtendedStatistic=p90, EvaluateLowSampleCountPercentile="ignore"
   - MetricName=P90Below,  ExtendedStatistic=p90, EvaluateLowSampleCountPercentile="evaluate"
   - MetricName=P99Below,  ExtendedStatistic=p99, EvaluateLowSampleCountPercentile="evaluate"
   - MetricName=P90Below,  ExtendedStatistic=p90, EvaluateLowSampleCountPercentile="ignore"
   - MetricName=P99Below,  ExtendedStatistic=p99, EvaluateLowSampleCountPercentile="ignore"
3. Keep publishing once per minute and poll DescribeAlarms(AlarmTypes=["MetricAlarm"]) for each alarm.
4. Observe that the three alarms at or above the 10/(1-p) floor (or set to `evaluate`) never transition to ALARM — they stay INSUFFICIENT_DATA — so the poll never terminates and the check is cut at the harness cap.
5. Also call GetMetricData over the P99Below series with MetricStat Stat="p99" and Stat="SampleCount", Period=60, and compare against the expected 100.0 / 200.0 values.
