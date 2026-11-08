Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-read-only-analysis-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-read-only-analysis-paths
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|BatchGetFrameMetricData|2xx->2xx|$.endTimes[*]
Issue-Key: codeguru-profiler|BatchGetFrameMetricData|2xx->2xx|$.frameMetricData[*].values[*]
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# BatchGetFrameMetricData reports one time step too many and substitutes 0.0 for a step with no profile data

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetFrameMetricData

## Description
One root cause area in the BatchGetFrameMetricData series builder; the replayed step diverges in two linked ways on an otherwise matching 200 response.

Cause 1 of 2 — the series carries one time step more than the requested period holds.
AWS: Smithy models BatchGetFrameMetricData (POST /profilingGroups/{profilingGroupName}/frames/-/metrics, 200) with required output members startTime, endTime, resolution, endTimes (ListOfTimestamps of TimestampStructure{value}), unprocessedEndTimes and frameMetricData. endTimes' own doc fixes the count: "List of instances, or time steps, in the time series. For example, if the period is one day (PT24H), and the resolution is five minutes (PT5M), then there are 288 endTimes in the list that are each five minutes apart" — i.e. exactly period ÷ resolution time steps (86400/300 = 288), with no extra boundary step. The recorded response for the replayed window at PT5M resolution carries 12 end times (and 12 values per frame metric), matching period ÷ resolution.
Emulator: returns 13 end times for that same request — one trailing extra, and correspondingly a 13th value in frameMetricData[].values. The step walk aligns the first step's start DOWN to the enclosing aggregation window (crate::state::aggregation_window_start) and then advances while step_start < end, so a period whose bounds are not already window-aligned yields an extra partial step; the walk's own cap is deliberately (MAX_PERIOD / step) + 1, so nothing trims it back to period ÷ resolution.
Why: a client plotting the series gets one bucket too many, and because endTimes and every frameMetricData[].values list are index-aligned by contract, every consumer that zips them against its own expected grid (period ÷ resolution) is off by one at the tail and charts a phantom final interval.
Fix: the time-step enumeration in the BatchGetFrameMetricData handler — emit exactly the period ÷ resolution aggregation windows the documented series has, rather than every window overlapping the period after aligning the start down.

Cause 2 of 2 — a step with no profile data is reported as 0.0 instead of no value.
AWS: the model says nothing about absent values (FrameMetricValues is a plain list of FrameMetricValue doubles, not sparse), so: recorded behavior only. The recording for a group whose steps hold no usable aggregated frame data carries a value of null at every reported index, not a number — AWS reports no value for a step it has no metric data for, and keeps the index so the list stays aligned with endTimes.
Emulator: fills those positions with the number 0.0 (the series builder's "no count at all" branch returns 0.0 whenever the whole-profile tally for the step is not greater than zero).
Why: 0.0 and "no data" are different facts for a relative-time metric. A client averaging, summing or thresholding the series treats absent steps as a genuine zero-utilisation sample, which drags averages down and hides gaps, and code that tests for null to skip a gap never skips anything.
Fix: the per-step value construction in the BatchGetFrameMetricData handler — report no value for a step with no metric data, instead of defaulting the quotient to 0.0.
Traces: codeguruprofiler-read-only-analysis-paths#task#3 (both causes are observed on that one step). Source: src/ops/batch_get_frame_metric_data.rs — time_steps (~:274-304) for cause 1, frame_metric_data's value map (~:460-472) for cause 2, and the endTimes projection (~:697-702).

## Evidence
codeguruprofiler-read-only-analysis-paths#task#3 (BatchGetFrameMetricData, status 200 expected and returned):
  $.endTimes[12]: extra — expected no 13th entry, got a 13th TimestampStructure (the trailing aggregation-window boundary); the recording holds 12 end times for the requested period at the reported PT5M resolution.
  $.frameMetricData[0].values[0..11]: type_mismatch — expected null (no value for a step with no metric data), got 0.0 at every index.
  $.frameMetricData[0].values[12]: extra — the 13th value that accompanies the extra end time.
  Everything else in the response (startTime, endTime, resolution, unprocessedEndTimes, frameMetric identity) matched.

Smithy (services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/smithy-model/com-amazonaws-codeguruprofiler-codeguruprofiler.json): BatchGetFrameMetricDataResponse.endTimes @required -> ListOfTimestamps (member TimestampStructure{value: Timestamp date-time @required}); endTimes doc: "if the period is one day (PT24H), and the resolution is five minutes (PT5M), then there are 288 endTimes in the list that are each five minutes apart"; frameMetricData @required -> list of FrameMetricDatum{frameMetric, values: FrameMetricValues @required}; FrameMetricValues = list of double, NOT sparse; resolution -> AggregationPeriod one of PT5M | PT1H | P1D.

## Steps to Reproduce
1. CreateProfilingGroup with a fresh name and PostAgentProfile a profile (or none) so the group exists.
2. BatchGetFrameMetricData(profilingGroupName=<name>, targetResolution=PT5M, frameMetrics=[{frameName, type: AggregatedRelativeTotalTime, threadStates: [...]}]) over a period that is an exact multiple of the resolution but whose bounds are not window-aligned (the replayed fixture's one-hour window).
3. Count $.endTimes: the emulator returns period ÷ resolution + 1 entries (13 for a one-hour window at PT5M) where AWS returns period ÷ resolution (12), and $.frameMetricData[0].values has the same extra trailing entry.
4. Inspect $.frameMetricData[0].values for steps the group has no metric data in: the emulator returns 0.0, AWS returns null.
5. Re-replay fixture codeguruprofiler-read-only-analysis-paths (step task#3) to confirm both causes.
