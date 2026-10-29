Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p|run#18|Timeout|time.sleep(20.0)
Issue-Commit: f0c814ee744767522be6756500cd69fea7713687

# cloudwatch: percentile alarms (ExtendedStatistic + EvaluateLowSampleCountPercentile) never evaluate, so the documented 10/(1-p) low-sample floor is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `f0c814ee744767522be6756500cd69fea7713687`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause in the CloudWatch alarm evaluation engine, on the percentile/low-sample axis. The symptom family is already filed for the non-percentile evaluation axes in 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md (that issue mentions this check id in its trace list but does NOT carry this run's replay key, so the replay below is not gated by it); it is filed here as its own work item because the failing point has moved and the percentile-specific behavior (ExtendedStatistic + EvaluateLowSampleCountPercentile + StateReasonData.evaluatedDatapoints) is a distinct piece of evaluation logic that must be closed on its own.

Cause 1 of 1: a metric alarm whose statistic is a percentile never advances its state from published datapoints, and the EvaluateLowSampleCountPercentile policy is therefore inert.
AWS: Smithy: PutMetricAlarmInput.ExtendedStatistic (target ExtendedStatistic), PutMetricAlarmInput.EvaluateLowSampleCountPercentile (@length 1..255), PutMetricAlarmInput.Unit (StandardUnit enum), Period (@range min 1), EvaluationPeriods / DatapointsToAlarm (@range min 1), TreatMissingData (@length 1..255); the operation models only LimitExceededFault, so a well-formed percentile alarm is accepted and then evaluated. AWS evaluates such an alarm once per Period against the datapoints PutMetricData ingested for that metric+dimension set, and for 0.5 <= p < 1 applies EvaluateLowSampleCountPercentile when the period holds fewer than 10/(1-p) samples (100 samples for p90, 1000 for p99) — NOT a flat 10 samples. With EvaluateLowSampleCountPercentile='ignore' and a sample count below that floor the alarm state is withheld (it stays in its current state, StateReason unchanged, and no ALARM item is appended to DescribeAlarmHistory); at or above the floor, and for 'evaluate' at any sample count, the alarm transitions normally and DescribeAlarms reports StateValue=ALARM with StateReasonData carrying statistic=<the requested percentile> and one evaluatedDatapoints entry whose sampleCount is the period's sample count and whose value is the computed percentile. The percentile the alarm withholds is still computable on the query path: GetMetricData with Stat='p99' returns StatusCode='Complete' with the value. Docs: https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/AlarmThatSendsEmail.html#percentiles-with-low-samples
Emulator: no percentile alarm ever leaves its creation state. The check publishes, once per minute, statistic sets of 99 / 100 / 200 samples (Values=[100.0], Counts=[n], Unit=Milliseconds) and then polls DescribeAlarms for the three CONTROL alarms (p90 at the exact 100-sample floor with 'ignore', p90 at 99 samples with 'evaluate', p99 at 200 samples with 'evaluate') to reach ALARM, with its own 420 s budget and a re-publish on every iteration. None of them transitions; the check is killed at the 60 s replay cap while still polling, so the withheld-vs-evaluated distinction, the StateReasonData.evaluatedDatapoints/sampleCount/statistic content, the no-ALARM-in-history requirement for the withheld pair, and the GetMetricData p99/SampleCount read are all never reached.
Why: a client that alarms on a latency percentile waits forever — the alarm looks permanently un-evaluated. Worse, because the floor policy is inert a client cannot distinguish 'CloudWatch deliberately withheld this percentile alarm because the period is under-sampled' from 'this alarm is broken', which is exactly the decision EvaluateLowSampleCountPercentile exists to make; paging and autoscaling built on p90/p99 alarms silently never fire.
Fix: the alarm evaluation engine in the cloudwatch service — the periodic evaluator that joins stored datapoints to registered alarms must compute percentile statistics over the stored statistic sets (the same extended-statistic computation GetMetricStatistics/GetMetricData already share), apply the 10/(1-p) sample-count floor against the alarm's EvaluateLowSampleCountPercentile before deciding whether to move the state, write StateValue/StateReason/StateReasonData (statistic, evaluatedDatapoints[].sampleCount/value) and append history items. Belongs with the PutMetricAlarm handler's alarm state and the evaluator, not in the read handlers.
Traces: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p.

## Evidence
Failing check: [mismatch] cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p — "Error: Check timed out after 60.0s".

Expected (AWS): after creating five alarms over the same per-minute series — p90/99-samples/'ignore', p90/99/'evaluate', p90/100/'ignore', p99/200/'ignore', p99/200/'evaluate' — the three controls reach StateValue='ALARM' (Threshold 10.0, GreaterThanThreshold, every sample 100.0) with StateReasonData.statistic equal to the requested percentile and exactly one evaluatedDatapoints entry (sampleCount 100.0 / 99.0 / 200.0, value 100.0), while the two 'ignore' alarms below the floor stay INSUFFICIENT_DATA with an unchanged StateReason and no ALARM item in DescribeAlarmHistory.
Actual (emulator): all five alarms are created and read back with the right EvaluateLowSampleCountPercentile and ExtendedStatistic, then no state ever changes. The poll
    _poll_until(lambda: {k: _describe(cw, names[k]) for k in controls},
               lambda d: all(a["StateValue"] == "ALARM" for a in d.values()),
               timeout_s=420.0, interval_s=15.0, on_iteration=_tick,
               description="the controls [...] reached ALARM over a constant 100.0 series")
never resolves and the check is killed at the 60 s cap. No non-2xx and no wrong field value is involved: the alarms simply are never evaluated.

Related already-open issue (does NOT carry this replay key): 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md.

## Steps to Reproduce
1. PutMetricData(Namespace=NS, MetricData=[{MetricName:'P90AtFloor', Timestamp:<this minute>, Values:[100.0], Counts:[100.0], Unit:'Milliseconds'}, {MetricName:'P90Below', Values:[100.0], Counts:[99.0], ...}, {MetricName:'P99Below', Values:[100.0], Counts:[200.0], ...}]) and repeat once per minute.
2. PutMetricAlarm(AlarmName=a1, Namespace=NS, MetricName='P90AtFloor', ExtendedStatistic='p90', Unit='Milliseconds', Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator='GreaterThanThreshold', TreatMissingData='missing', EvaluateLowSampleCountPercentile='ignore', ActionsEnabled=false). Repeat for ('P90Below','p90','evaluate') and ('P99Below','p99','evaluate') as controls, and for ('P90Below','p90','ignore') and ('P99Below','p99','ignore') as the under-sampled pair.
3. DescribeAlarms(AlarmNames=[a1..a5]) — every alarm reads back the requested ExtendedStatistic/EvaluateLowSampleCountPercentile and StateValue='INSUFFICIENT_DATA'.
4. Keep publishing and poll DescribeAlarms for the three controls to reach 'ALARM': they never do (observed for the whole 60 s replay window; the check's own budget is 420 s).
5. Consequently never reached: StateReasonData for a fired control (statistic + one evaluatedDatapoints entry with the published sampleCount and value 100.0); the 120 s hold proving the two 'ignore' alarms below the 10/(1-p) floor do NOT move and append no ALARM history item; and GetMetricData(Stat='p99') / Stat='SampleCount' over 'P99Below' returning Complete with 100.0 and 200.0.
