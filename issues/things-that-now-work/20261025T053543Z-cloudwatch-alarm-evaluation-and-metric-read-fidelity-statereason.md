Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-evaluation-engine-greater-than-operators-split-at-equal-threshold cloudwatch.alarm-evaluation-engine-less-than-operators-split-at-equal-threshold cloudwatch.alarm-evaluation-engine-statistic-maximum-alarms-where-average-does-not cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it cloudwatch.alarm-missing-data-and-m-of-n-treat-missing-data-four-way-over-one-hole cloudwatch.alarm-composite-and-actions-ok-and-insufficient-data-functions-are-first-class cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m cloudwatch.alarm-unit-and-percentile-traps-standard-resolution-accepts-sub-minute-period-but-buckets-by-minute cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p cloudwatch.cw-treat-missing-data-breaching-fires-with-no-datapoints-while-notbreaching-stays-ok cloudwatch.metrics-math-and-emf-get-metric-data-pagination-reassembles-every-query-in-scan-order cloudwatch.metrics-math-and-emf-period-buckets-anchor-at-minute-rounded-start-time cloudwatch.get-metric-data-invalid-next-token-rejected cloudwatch.fixing-the-comparison-operator-in-place-resolves-the-alarm
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-evaluation-engine-greater-than-operators-split-at-equal-threshold|_reason#14|AssertionError|assert m, f"StateReason {text!r} is not a 'Threshold Crossed: k out of the last n datapoints' reason"
Issue-Key: cloudwatch.alarm-evaluation-engine-less-than-operators-split-at-equal-threshold|_reason#14|AssertionError|assert m, f"StateReason {text!r} is not a 'Threshold Crossed: k out of the last n datapoints' reason"
Issue-Key: cloudwatch.alarm-evaluation-engine-statistic-maximum-alarms-where-average-does-not|_reason#13|AssertionError|assert m, f"StateReason {text!r} is not a 'Threshold Crossed: k out of the last n datapoints' reason"
Issue-Key: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it|run#18|AssertionError|assert baseline["StateReason"].startswith("Threshold Crossed"), (
Issue-Key: cloudwatch.alarm-missing-data-and-m-of-n-treat-missing-data-four-way-over-one-hole|run#19|AssertionError|assert a["StateReason"].startswith("Threshold Crossed: 1 out of the last 1 datapoints ["
Issue-Key: cloudwatch.alarm-composite-and-actions-ok-and-insufficient-data-functions-are-first-class|run#16|AssertionError|assert f"[{value}" in observed_child["StateReason"], (
Issue-Key: cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m|run#19|KeyError|window = srd_ok["recentDatapoints"]
Issue-Key: cloudwatch.alarm-unit-and-percentile-traps-standard-resolution-accepts-sub-minute-period-but-buckets-by-minute|run#13|KeyError|stamps = [_ts(d["timestamp"]) for d in srd.get("evaluatedDatapoints", [])]
Issue-Key: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p|run#15|AssertionError|assert srd.get("statistic") == specs[key][1], (
Issue-Key: cloudwatch.cw-treat-missing-data-breaching-fires-with-no-datapoints-while-notbreaching-stays-ok|run#22|AssertionError|assert "no datapoints were received" in reasons["b"].lower(), (
Issue-Key: cloudwatch.metrics-math-and-emf-get-metric-data-pagination-reassembles-every-query-in-scan-order|run#9|AssertionError|assert seen[-1] == "Complete" and all(s == "PartialData" for s in seen[:-1]), (
Issue-Key: cloudwatch.metrics-math-and-emf-period-buckets-anchor-at-minute-rounded-start-time|run#9|AssertionError|assert gms == want, (
Issue-Key: cloudwatch.get-metric-data-invalid-next-token-rejected|run#8|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: cloudwatch.fixing-the-comparison-operator-in-place-resolves-the-alarm|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# CloudWatch alarm evaluation and metric-read fidelity: StateReason/StateReasonData are not AWS's shape, TreatMissingData is never applied, GetMetricData pagination status and period anchoring are wrong (7 causes)

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Seven distinct net-new root causes in the CloudWatch emulator's alarm evaluator and metric read path. All seven are reached only AFTER an evaluation runs, so they are not covered by the open issue 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md (which reports that no evaluation happens at all): in this run alarms DO transition from published data, and what diverges is the content the evaluator emits. 20261025T024908Z-cloudwatch-four-scaffolded-operations-answer-notimplemented-so-getmetricdata.md and 20261025T020208Z-cloudwatch-getmetricdata-stub.md cover GetMetricData answering NotImplemented; causes 4-6 below are reached on calls that DID answer 2xx (or answered the right error code), so they are separate work.

=== Cause 1 of 7: the evaluated StateReason sentence is not CloudWatch's documented reason format ===
AWS: Smithy: MetricAlarm.StateReason -> StateReason @length 0..1023, documented as "An explanation for the alarm state, in text format." The model does not fix the wording, so this is recorded behavior only, measured by these checks on real AWS (us-west-2): every reason produced by an EVALUATION is prefixed "Threshold Crossed: " (for OK as well as ALARM - there is no "Threshold not Crossed" prefix), names k out of the last n datapoints, then lists the datapoints it graded in square brackets as `value (dd/mm/yy HH:MM:SS)` pairs, then the relation and threshold, then a trailing `(minimum <m> datapoints for <OK|ALARM> -> <OK|ALARM> transition).` The requirement is therefore: an evaluated reason is prefixed "Threshold Crossed", cites each datapoint value and timestamp the evaluator actually read, and states the m-of-n transition minimum.
Emulator: emits a different sentence with NO datapoint list and NO transition-minimum clause, and uses a "Threshold not Crossed" prefix for an evaluated OK. Observed verbatim: 'Threshold Crossed: 2 out of the last 2 datapoints satisfied GreaterThanOrEqualToThreshold with the threshold (50); 2 datapoints must for the alarm to be in ALARM state.' and 'Threshold not Crossed: 0 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (50), fewer than the 1 datapoints required for the alarm to be in ALARM state.' (the second is also ungrammatical - "2 datapoints must for the alarm").
Why: a client cannot tell WHICH datapoints moved the alarm, nor whether an OK was produced by a real evaluation or by SetAlarmState, because the only distinguishing signal (the "Threshold Crossed" prefix plus the cited datapoint list) is absent. Operators and runbooks that parse StateReason to attribute a page to a value get nothing.
Fix: the alarm evaluator's StateReason renderer in the cloudwatch service crate (the component that writes MetricAlarm.StateReason when an evaluation changes or confirms a state).
Traces: cloudwatch.alarm-evaluation-engine-greater-than-operators-split-at-equal-threshold, cloudwatch.alarm-evaluation-engine-less-than-operators-split-at-equal-threshold, cloudwatch.alarm-evaluation-engine-statistic-maximum-alarms-where-average-does-not, cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it, cloudwatch.alarm-missing-data-and-m-of-n-treat-missing-data-four-way-over-one-hole, cloudwatch.alarm-composite-and-actions-ok-and-insufficient-data-functions-are-first-class

=== Cause 2 of 7: StateReasonData omits recentDatapoints, evaluatedDatapoints[].timestamp and statistic ===
AWS: Smithy: MetricAlarm.StateReasonData -> StateReasonData @length 0..4000, "An explanation for the alarm state, in JSON format." Recorded behavior: the JSON object carries version "1.0", statistic (the Statistic or the ExtendedStatistic such as "p90"), period, threshold, a recentDatapoints array of the values in the evaluation window, and an evaluatedDatapoints array whose entries each carry timestamp, value and sampleCount.
Emulator: the object exists but keys are missing - `KeyError: 'recentDatapoints'`; `evaluatedDatapoints` entries have no `timestamp` key (`KeyError: 'timestamp'`); and for a percentile alarm `statistic` is absent (read back as None where "p90" was configured).
Why: anything that reconstructs an alarm's decision from the machine-readable reason - dashboards, incident enrichment, the aws CLI's own alarm views - gets a truncated object and either crashes on the missing key or silently reports no evaluated data.
Fix: the StateReasonData serializer in the alarm evaluator (same component as cause 1, JSON side).
Traces: cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m, cloudwatch.alarm-unit-and-percentile-traps-standard-resolution-accepts-sub-minute-period-but-buckets-by-minute, cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p

=== Cause 3 of 7: TreatMissingData is never applied - a hole is reported as a graded datapoint ===
AWS: Smithy: MetricAlarm.TreatMissingData documents the four arms breaching / notBreaching / ignore / missing. Recorded behavior: over a metric that has NEVER been written, the evaluator reports that it received no data and names the arm it applied, e.g. 'Threshold Crossed: no datapoints were received for 1 period and 1 missing datapoint was treated as [Breaching].' The requirement is that a missing period is reported as missing and the configured arm decides the state.
Emulator: an alarm on a metric with no datapoints at all still reports a graded datapoint: 'Threshold Crossed: 1 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (1); 1 datapoints must for the alarm to be in ALARM state.' - the evaluator fabricated a datapoint instead of recognising a hole, so the TreatMissingData arm is not what decided the state.
Why: the four TreatMissingData arms are indistinguishable, so a client cannot build the standard "alarm on absence" pattern (breaching on no data) or its inverse, and cannot tell a real breach from an outage of the publisher.
Fix: the missing-datapoint handling in the alarm evaluator's window assembly (where periods with no data are classified before comparison).
Traces: cloudwatch.cw-treat-missing-data-breaching-fires-with-no-datapoints-while-notbreaching-stays-ok

=== Cause 4 of 7: GetMetricData reports StatusCode=Complete on every page of a paginated read ===
AWS: Smithy: MetricDataResult.StatusCode -> StatusCode enum [Complete, InternalError, PartialData, Forbidden], documented "Complete indicates that all data points in the requested time range were returned. PartialData means that an incomplete set of data points were returned. You can use the NextToken value that was returned and repeat your request to get more data points." So a non-final page of a MaxDatapoints-limited read must report PartialData and only the final page Complete.
Emulator: with 30 datapoints read at MaxDatapoints=7, the StatusCode sequence across the five pages was ['Complete', 'Complete', 'Complete', 'Complete', 'Complete'].
Why: a client that stops paginating when it sees Complete (the documented meaning of the field) truncates the series on the first page and silently reports 7 of 30 datapoints.
Fix: the GetMetricData pagination/serialization path that sets MetricDataResult.StatusCode per result per page.
Traces: cloudwatch.metrics-math-and-emf-get-metric-data-pagination-reassembles-every-query-in-scan-order

=== Cause 5 of 7: period buckets are anchored at the raw StartTime instead of StartTime rounded down to the whole minute ===
AWS: recorded behavior only (documented for data under 15 days old in the CloudWatch User Guide): CloudWatch rounds the requested StartTime DOWN to the whole minute and lays Period-wide buckets from there. With one datapoint per minute, StartTime at B+2m30s and Period=300, the returned buckets are stamped on the minute (B+2m, B+7m) and aggregate the points inside those minute-anchored windows.
Emulator: buckets are stamped at the sub-minute StartTime and aggregate a shifted window - GetMetricStatistics Period=300 from B+2m30s returned [('...T05:02:30+00:00', 30.0), ('...T05:07:30+00:00', 19.0)] where the minute-rounded anchor gives [('...T05:02:00+00:00', 25.0), ('...T05:07:00+00:00', 27.0)]. Both the timestamps and the aggregated values are wrong.
Why: every read whose StartTime is not on a whole minute returns datapoints at timestamps AWS never emits and sums/averages that include or exclude the wrong datapoint, so client-side joins on timestamp and any threshold computed off the read disagree with AWS.
Fix: the period-bucketing component shared by GetMetricStatistics and GetMetricData (StartTime normalisation before bucket layout).
Traces: cloudwatch.metrics-math-and-emf-period-buckets-anchor-at-minute-rounded-start-time

=== Cause 6 of 7: GetMetricData's InvalidNextToken refusal carries a message where AWS sends none ===
AWS: recorded behavior only. A garbled NextToken is refused with error code InvalidNextToken and NO message text; the rule is that this code is returned message-less on GetMetricData. The positive control (the identical query with no NextToken) succeeds, so the refusal is specific to the token.
Emulator: the code InvalidNextToken is correct, but the Message member is populated: 'NextToken is not a token this operation minted'. (The recorded empty message is the requirement here; the emulator's text is reported only as what was observed.)
Why: a client that keys on message emptiness, or that surfaces Message verbatim, shows text AWS never sends for this refusal.
Fix: the GetMetricData NextToken validation path's error construction.
Traces: cloudwatch.get-metric-data-invalid-next-token-rejected

=== Cause 7 of 7: an in-place PutMetricAlarm update is not followed by a re-evaluation ===
AWS: recorded behavior only. Re-putting an existing alarm name with only ComparisonOperator changed is an in-place update (one alarm, a ConfigurationUpdate history item, the new operator stored), after which the SAME already-published series is re-graded and the alarm resolves to OK; a later breaching value fires the updated alarm.
Emulator: the check never observes the post-update resolve and hangs to the 60 s harness cap ('Check timed out after 60.0s'). Sibling evaluation checks in this same suite do complete inside that cap and show that evaluation itself runs, so the update path specifically does not re-grade the alarm. Hypothesis: the updated configuration is stored but the evaluator keeps (or never re-derives) the pre-update state, so no transition is ever produced.
Why: the standard remediation loop - fix a mis-configured alarm in place and watch it clear - never clears, so an operator cannot distinguish a repaired alarm from a still-broken one.
Fix: the PutMetricAlarm update path's interaction with the evaluator (re-arm / re-evaluate on configuration change).
Traces: cloudwatch.fixing-the-comparison-operator-in-place-resolves-the-alarm

## Evidence
Replayed behavioral suite (simparity-behavioral-ga-prod-v1), 85/409 passed.

Cause 1 - expected an evaluated reason matching ^Threshold Crossed: (k) out of the last (n) datapoints \[<value> (dd/mm/yy HH:MM:SS), ...\] (was|were) <relation> the threshold (<thr>) (minimum <m> datapoints for <OK|ALARM> -> <OK|ALARM> transition).$
  got: 'Threshold Crossed: 2 out of the last 2 datapoints satisfied GreaterThanOrEqualToThreshold with the threshold (50); 2 datapoints must for the alarm to be in ALARM state.'
  got: 'Threshold Crossed: 2 out of the last 2 datapoints satisfied LessThanOrEqualToThreshold with the threshold (50); 2 datapoints must for the alarm to be in ALARM state.'
  got (evaluated OK): 'Threshold not Crossed: 0 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (10), fewer than the 1 datapoints required for the alarm to be in ALARM state.'
  AssertionError: "child's StateReason does not cite datapoint 7.0"

Cause 2 - KeyError: 'recentDatapoints' (StateReasonData); KeyError: 'timestamp' (StateReasonData.evaluatedDatapoints[*]); AssertionError: "'p90-100-ignore' StateReasonData statistic=None, expected 'p90'"

Cause 3 - metric never written; expected a reason saying no datapoints were received and naming the TreatMissingData arm; got 'Threshold Crossed: 1 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (1); 1 datapoints must for the alarm to be in ALARM state.'

Cause 4 - AssertionError: TimestampDescending: asum StatusCode sequence across pages was ['Complete', 'Complete', 'Complete', 'Complete', 'Complete']; expected PartialData on pages 1..n-1 and Complete on the last.

Cause 5 - AssertionError: GetMetricStatistics Period=300 from B+2m30s: buckets [('2026-10-25T05:02:30+00:00', 30.0), ('2026-10-25T05:07:30+00:00', 19.0)]; expected [('2026-10-25T05:02:00+00:00', 25.0), ('2026-10-25T05:07:00+00:00', 27.0)].

Cause 6 - error code InvalidNextToken correct; AssertionError: expected an empty message, got 'NextToken is not a token this operation minted'.

Cause 7 - Error: Check timed out after 60.0s (no assertion reached).

## Steps to Reproduce
1. Clone the repo and boot the emulator, then replay the behavioral suite.
2. Cause 1/2: PutMetricData a known value, PutMetricAlarm over it (Period=10, EvaluationPeriods=2, Statistic=Maximum or ExtendedStatistic=p90), wait for the state to move, then DescribeAlarms and read StateReason / json.loads(StateReasonData).
3. Cause 3: PutMetricAlarm over a metric name that was never published, once with TreatMissingData=breaching and once with notBreaching; wait, then DescribeAlarms and read StateReason.
4. Cause 4: PutMetricData 30 datapoints across 3 series, GetMetricData with 6 queries and MaxDatapoints=7, and follow NextToken; read MetricDataResults[*].StatusCode on each page.
5. Cause 5: publish one datapoint per minute from a whole-minute base B, then GetMetricStatistics with StartTime=B+2m30s and Period=300; compare returned bucket timestamps and values.
6. Cause 6: GetMetricData with NextToken="this-is-not-a-real-next-token-!!garbled=="; inspect Error.Code and Error.Message.
7. Cause 7: PutMetricAlarm GreaterThanThreshold over a healthy series until it fires, re-PutMetricAlarm the same name with LessThanThreshold only, then poll DescribeAlarms for OK.
8. Re-replay the check ids in trace_ids to confirm.
