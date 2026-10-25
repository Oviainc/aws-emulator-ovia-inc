Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.cli-alarms-for-metric-query-matches-the-sdk cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup cloudwatch.metrics-math-and-emf-metric-math-non-finite-points-dropped-with-arithmetic-error cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.cli-alarms-for-metric-query-matches-the-sdk|run#15|AssertionError|assert cli([cid], "StateValue") == "ALARM", "the cluster-wide alarm should read ALARM"
Issue-Key: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it|run#28|AssertionError|assert "1 out of the last 1 datapoints" in reclaimed["StateReason"], (
Issue-Key: cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.metrics-math-and-emf-metric-math-non-finite-points-dropped-with-arithmetic-error|run#18|AssertionError|assert gap["messages"] == [{"Code": "ArithmeticError", "Value": _ARITH_DROPPED}], (
Issue-Key: cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm|run#36|AssertionError|assert dead_message == "", (
Issue-Key: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action|_drain_queue#27|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# CloudWatch: 6 net-new root causes — a SetAlarmState override is discarded within seconds, the reclaiming StateReason numerator is the breaching count, composite alarms never derive state (2 checks hang), the metric-math ArithmeticError text is reworded, SetAlarmState's ResourceNotFound carries a message AWS leaves empty, and the DisableAlarmActions suppression check never completes

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Six net-new root causes in the CloudWatch emulator, all reached AFTER the alarm evaluator and the metric store already work for the simple cases (every one of these checks gets past alarm creation, metric publication and, where relevant, an evaluated transition). None of them is marked owned or already-filed by the replay report.

=== Cause 1 of 6: a SetAlarmState override does not survive to the next read ===
Failing check id: cloudwatch.cli-alarms-for-metric-query-matches-the-sdk.
AWS: Smithy models SetAlarmState with AlarmName/StateValue/StateReason required and errors InvalidFormatFault (400) and ResourceNotFound (404); the operation's own documentation says the state it writes is "temporarily" in force and that "the alarm is then evaluated normally" at its next evaluation, i.e. the override is owned by the alarm record until the evaluator next runs, which for Period=300 is minutes away. Recorded behavior on real AWS: a client forces StateValue=ALARM on an alarm over AWS/Redshift HealthStatus (a metric nothing in the account writes), then reads the alarm back twice through DescribeAlarmsForMetric — once via the CLI, once via the SDK — and both reads report ALARM.
Emulator: the forced ALARM is gone by the time of those reads, a few seconds later. The CLI and the SDK agree with each other on all four read fields (AlarmName, StateValue, ComparisonOperator, Threshold all matched), so this is not a front-end divergence: the stored StateValue itself is no longer ALARM. AssertionError: "the cluster-wide alarm should read ALARM" on `cli([cid], "StateValue") == "ALARM"` (check body line 69), with the companion leg asserting the untouched twin reads INSUFFICIENT_DATA. The override is therefore discarded (re-graded to the data-less baseline) essentially immediately instead of holding until the alarm's own next scheduled evaluation.
Why: SetAlarmState is the standard way to drive alarm-dependent machinery in tests and runbooks (118 corpus files use it as their act step). A client that forces ALARM and then reads, polls or hands the alarm to a downstream consumer sees the state it just set vanish, so nothing downstream can be driven deterministically — and nothing distinguishes "my SetAlarmState call was ignored" from "the evaluator re-graded it".
Fix: the lifetime of the forced-state override in the cloudwatch crate's alarm state/evaluator component — the override must be the alarm's state until the evaluator's next scheduled pass for that alarm's Period, not until the next read. Note the INVERSE of this is on file and still open as 20261025T084321Z-cloudwatch-setalarmstate-s-forced-alarm-state-is-never-reclaimed.md (forced state never reclaimed at all); the behavior has moved past that issue to the opposite extreme, so neither that issue nor the resolved 20261025T174025Z-cloudwatch-setalarmstate-never-invokes-the-alarm-s-alarmactions-so.md covers this.
Traces: cloudwatch.cli-alarms-for-metric-query-matches-the-sdk.

=== Cause 2 of 6: the StateReason written when the evaluator reclaims a forced state reports the breaching count and the ALARM-direction requirement ===
Failing check id: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it.
AWS: Smithy models MetricAlarm.StateReason as StateReason, string @length 0..1023, "An explanation for the alarm state, in text format" — the wording itself is recorded behavior only. Recorded on real AWS: when the evaluator reclaims a forced ALARM on an alarm configured EvaluationPeriods=1 / DatapointsToAlarm=1 / Threshold=10.0 / GreaterThanThreshold whose single datapoint is 1.0, the reason reports the datapoints that satisfied the direction being reported — "1 out of the last 1 datapoints" — not the count that breached. The numerator is the number of datapoints satisfying the reported direction, the denominator is EvaluationPeriods, and the trailing clause states the ALARM -> OK transition minimum (EvaluationPeriods - DatapointsToAlarm + 1). All of those are values taken from the request, never literals.
Emulator: the reclaim now happens, but the reason is rendered from the ALARM direction only: 'Threshold Crossed: 0 out of the last 1 datapoints [1.0 (<timestamp>)] satisfied GreaterThanThreshold with the threshold (10.0), fewer than the 1 datapoints required for the alarm to be in ALARM state.' The fixed text "Threshold Crossed: ", " out of the last ", " datapoints " is right; the numerator is the breaching count (0) where AWS reports the satisfying count (1), and the "minimum <N-M+1> datapoints for ALARM -> OK transition" clause is absent. The bracketed datapoint value/timestamp and the threshold are request/state-derived and correctly substituted.
Why: a client cannot tell which direction was evaluated, nor recover DatapointsToAlarm/EvaluationPeriods from the reason the way AWS permits; incident tooling that parses StateReason to attribute a verdict to a datapoint count reads the complement of AWS's number (0 vs 1).
Fix: the alarm evaluator's StateReason renderer in the cloudwatch crate (direction-aware numerator plus the transition-minimum clause). The same renderer defect is already described for two OTHER check ids by the open 20261025T100455Z-cloudwatch-alarm-evaluator-a-period-10-high-resolution.md (cause 2); this check id appears in neither that issue's trace ids nor 20261025T053543Z-cloudwatch-alarm-evaluation-and-metric-read-fidelity-statereason.md, and the report did not mark it owned, so it is recorded here so the reclaim path is not closed unfixed.
Traces: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it.

=== Cause 3 of 6: composite alarms never derive a state from their children, so both composite-alarm checks hang to the harness cap ===
Failing check ids: cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected, cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup.
AWS: PutCompositeAlarm's AlarmRule is a boolean expression over existing alarms (ALARM()/OK() functions ANDed together); recorded behavior on real AWS (us-west-2) is that the composite evaluates that rule against its children's current states and transitions accordingly, that a rule naming an alarm which does not exist (including itself) is refused with the message "Could not save the composite alarm as alarms [<arn>] in the alarm rule do not exist" (the ARN is request-derived), that a cycle between two existing composites is ACCEPTED with 200 and both settle on OK, and that a composite in ALARM invokes its AlarmActions (observed as an SNS-to-SQS delivery).
Emulator: both checks consume the whole 60 s harness budget and are reported as "Error: Check timed out after 60.0s" — each has a bounded poll for the composite's derived state (the e2e check waits for two children forced into ALARM via SetAlarmState to drive the ANDed composite into ALARM and deliver one SQS message; the rule-validation check waits for the mixed ALARM()/OK() rule to settle and for the cycle pair to stay inert). Neither poll is ever satisfied, so the composite never derives a state from its children and its action never fires.
Why: a composite alarm is the only way to express an AND/OR over alarms; one that never leaves its initial state means every aggregated-health signal, and every notification hanging off it, is silently dead. Clients see an alarm that exists, round-trips and never fires.
Fix: the composite-alarm evaluation component in the cloudwatch crate — rule parsing/evaluation against child alarm states on each pass, and the action dispatch for the derived transition. Related but not covering: the resolved 20261025T020208Z-cloudwatch-sns-sqs-cloudwatch-alarm-actions-never-fire-to-sns-so.md mentioned composite AND-rule waiting as collateral, and 20261025T020208Z-cloudwatch-alarm-delete-conflict-semantics-deletealarms-is-not.md covers the delete-conflict leg only.
Traces: cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected, cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup.

=== Cause 4 of 6: the metric-math ArithmeticError message is reworded ===
Failing check id: cloudwatch.metrics-math-and-emf-metric-math-non-finite-points-dropped-with-arithmetic-error.
AWS: Smithy models GetMetricData output member Messages as MetricDataResultMessages of MessageData{Code: MessageDataCode, Value: MessageDataValue} — "A message returned by the GetMetricData API, including a code and a description"; the text is recorded behavior only. Measured on real AWS (us-west-2, 2026-09-22) for `m1/(m2-1)` where m2-1 is zero at exactly one timestamp: StatusCode Complete, the offending timestamp dropped from both Timestamps and Values, and exactly one Messages entry {Code: "ArithmeticError", Value: "One or more data-points have been dropped due to non-numeric values (NaN, -Infinite, +Infinite)"}. The whole Value is fixed text — it interpolates nothing from the request.
Emulator: the Code, the per-point drop and the StatusCode are all correct, but the Value is 'One or more data points have been dropped due to non-numeric values (NaN, -Infinite, +Infinite)' — "data points" where AWS writes "data-points". AssertionError at check body line 69 comparing the single-entry Messages list.
Why: the message is the only signal that a point was silently dropped, so clients and dashboards match on its text; a reworded fixed string breaks exact-match handling and any log-based detection of arithmetic drops, while looking correct to a human reader.
Fix: the GetMetricData metric-math evaluator's message constant in the cloudwatch crate (the ArithmeticError text emitted when non-finite points are dropped).
Traces: cloudwatch.metrics-math-and-emf-metric-math-non-finite-points-dropped-with-arithmetic-error.

=== Cause 5 of 6: SetAlarmState on a deleted alarm answers ResourceNotFound with an invented message ===
Failing check id: cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm.
AWS: Smithy models SetAlarmState's error ResourceNotFound, errorKind client, HTTP 404, awsQueryError code ResourceNotFound, with a single member `message` (ErrorMessage, @length 1..255) carrying NO documentation. Recorded behavior on real AWS for a mutation against an alarm that was just deleted: the refusal is ResourceNotFound and its Message is EMPTY — AWS sends no explanatory text for this error class, so the error code is the entire contract.
Emulator: the code is right (ResourceNotFound) but the message is invented: 'alarm sim2real-search-control-<suffix> does not exist' — an emulator-authored sentence whose only variable part is the alarm name from the request. AssertionError at check body line 274: "expected the exact ResourceNotFound message AWS returns, got 'alarm ... does not exist'". Everything else in this check passes, including the server-side SEARCH() discovery legs and the alarm-side refusal of a search expression.
Why: a client that keys off the message (or asserts the absence of one, as AWS's own SDK error shapes allow) sees text AWS never sends; any parity harness or error-equality comparison diverges on an otherwise correct refusal.
Fix: the SetAlarmState handler's not-found refusal in the cloudwatch crate — return ResourceNotFound with no message body for a missing alarm. The same family on a different operation is already open as 20261025T020208Z-cloudwatch-firehose-s3-getmetricstream-s-not-found-error-carries-a-message.md (GetMetricStream); this is the SetAlarmState instance and is not covered by it.
Traces: cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm.

=== Cause 6 of 6: the DisableAlarmActions / EnableAlarmActions suppression check never completes ===
Failing check id: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action.
AWS: recorded behavior only — DisableAlarmActions suppresses action delivery for a state change driven by SetAlarmState, and EnableAlarmActions restores it, observed as an SNS-to-SQS message that arrives in the enabled leg and does not arrive in the disabled leg (the absence window is calibrated from the run's own delivery latency, so a merely slow target is not read as a blocking one).
Emulator: the check consumes the whole 60 s harness budget — "Error: Check timed out after 60.0s" — so the delivery it waits for after re-enabling actions never arrives and the calibration the negative leg depends on can never be established. The suppression semantics are therefore unobservable in either direction.
Why: ActionsEnabled is the switch every maintenance window and every test harness uses to stop an alarm from paging; an emulator where the enabled leg never delivers makes both the suppression and its release untestable, and a client cannot distinguish "suppressed" from "nothing is wired up".
Fix: the SetAlarmState-driven action dispatch path in the cloudwatch crate and its ActionsEnabled gate (DisableAlarmActions/EnableAlarmActions must flip delivery for state changes, not merely record a flag). The resolved 20261025T174025Z-cloudwatch-setalarmstate-never-invokes-the-alarm-s-alarmactions-so.md claimed this dispatch; it does not cover this check, which still cannot observe one delivery. Related and open for a different service token: 20261025T174025Z-cloudwatch-sns-sqs-breaching-datapoints-move-neither-the.md.
Traces: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the built emulator: 108/412 passed (26.2%); cloudwatch 102/254. All seven check ids below are in the failing list and NONE is marked "ALREADY FILED" or "OWNED by".

Cause 1 — cloudwatch.cli-alarms-for-metric-query-matches-the-sdk [mismatch].
Passed first: both PutMetricAlarm calls (AWS/Redshift HealthStatus, one dimensioned [ClusterIdentifier], one [ClusterIdentifier, NodeID=Leader]); SetAlarmState StateValue=ALARM StateReason='sim2real: forced' returned 200; DescribeAlarmsForMetric matched exactly one alarm for each dimension set; and the CLI's shorthand --dimensions form read the SAME four fields (AlarmName, StateValue, ComparisonOperator, Threshold) as the SDK for both alarms.
Expected: `aws cloudwatch describe-alarms-for-metric --query MetricAlarms[0].StateValue` == "ALARM" for the alarm just forced into ALARM (and INSUFFICIENT_DATA for the untouched twin).
Actual: AssertionError: "the cluster-wide alarm should read ALARM" (check body line 69) — the forced state was already gone a few seconds after the 200, on both front ends.

Cause 2 — cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it [mismatch].
Passed first: the forced ALARM was taken back by the evaluator (the premise of the check) and the reason string carries AWS's 'Threshold Crossed: ' prefix and the bracketed datapoint.
Expected: the reclaiming StateReason contains "1 out of the last 1 datapoints" for EvaluationPeriods=1 / DatapointsToAlarm=1.
Actual: AssertionError (check body line 183): StateReason was 'Threshold Crossed: 0 out of the last 1 datapoints [1.0 (2026-10-25T21:39:54.25Z)] satisfied GreaterThanThreshold with the threshold (10.0), fewer than the 1 datapoints required for the alarm to be in ALARM state.' — numerator is the breaching count, and no "minimum 1 datapoint for ALARM -> OK transition" clause.

Cause 3 — cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected [mismatch] and cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup [mismatch].
Expected: the composite's ANDed rule over its children derives ALARM/OK within the checks' own bounded polls, the e2e composite's SNS action delivers exactly one SQS message, and the rule-validation legs settle (self-reference refused with "Could not save the composite alarm as alarms [<arn>] in the alarm rule do not exist"; an accepted 2-cycle staying inert).
Actual: both reported "Error: Check timed out after 60.0s" — the bounded polls for the derived composite state are never satisfied, so the harness cap is hit before any later leg runs.

Cause 4 — cloudwatch.metrics-math-and-emf-metric-math-non-finite-points-dropped-with-arithmetic-error [mismatch].
Passed first: PutMetricData of the two 5-point series, and the control `m1/m2` with all five points and no message.
Expected: the `m1/(m2-1)` result's Messages == [{"Code": "ArithmeticError", "Value": "One or more data-points have been dropped due to non-numeric values (NaN, -Infinite, +Infinite)"}].
Actual: AssertionError (check body line 69): Messages were [{'Code': 'ArithmeticError', 'Value': 'One or more data points have been dropped due to non-numeric values (NaN, -Infinite, +Infinite)'}] — "data points" instead of "data-points".

Cause 5 — cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm [mismatch].
Passed first: three SEARCH()-discovered series all returned under the literal query Id with per-series Labels; the decoy metric excluded; a SEARCH expression inside PutMetricAlarm refused; DeleteAlarms left nothing behind; and the post-delete SetAlarmState raised code ResourceNotFound.
Expected: that ResourceNotFound's Message is the empty string.
Actual: AssertionError (check body line 274): Message was 'alarm sim2real-search-control-a41dd80f598a does not exist'.

Cause 6 — cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action [mismatch].
Expected: with actions enabled, a SetAlarmState-driven transition delivers one SNS-to-SQS message (which also calibrates the absence window); with DisableAlarmActions in force, no message arrives over that calibrated window.
Actual: "Error: Check timed out after 60.0s" — the enabled-leg delivery never arrives, so neither leg can be graded.

## Steps to Reproduce
Cause 1 (forced state lifetime):
1. cloudwatch:PutMetricAlarm AlarmName=A, Namespace=AWS/Redshift, MetricName=HealthStatus, Dimensions=[{ClusterIdentifier,<c>}], Statistic=Average, Period=300, EvaluationPeriods=1, Threshold=0.5, ComparisonOperator=GreaterThanThreshold, ActionsEnabled=false. Add a second alarm B with Dimensions=[{ClusterIdentifier,<c>},{NodeID,Leader}], Period=1800, Threshold=1.0, ComparisonOperator=LessThanThreshold.
2. cloudwatch:SetAlarmState AlarmName=A, StateValue=ALARM, StateReason='forced'.
3. Immediately cloudwatch:DescribeAlarmsForMetric Namespace=AWS/Redshift MetricName=HealthStatus Dimensions=[{ClusterIdentifier,<c>}] (and the same through `aws cloudwatch describe-alarms-for-metric --dimensions Name=ClusterIdentifier,Value=<c> --query MetricAlarms[0].StateValue --output text`).
4. Expected StateValue ALARM for A (and INSUFFICIENT_DATA for B). Observed: A no longer reads ALARM on either front end.

Cause 2 (reclaiming StateReason):
1. cloudwatch:PutMetricAlarm over a fresh custom namespace, Statistic=Maximum, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, ActionsEnabled=false.
2. cloudwatch:SetAlarmState StateValue=ALARM StateReason='sim2real: forced'.
3. cloudwatch:PutMetricData one datapoint of 1.0 (below threshold) and poll cloudwatch:DescribeAlarms until the evaluator reclaims the state.
4. Expected StateReason to contain "1 out of the last 1 datapoints" (the count satisfying the reported direction) and the "minimum 1 datapoint for ALARM -> OK transition" clause. Observed numerator 0 and the ALARM-direction clause only.

Cause 3 (composite alarms):
1. Create two metric alarms C1, C2 over a fresh namespace with ActionsEnabled=false; cloudwatch:SetAlarmState both to ALARM.
2. sns:CreateTopic + sqs:CreateQueue + queue policy allowing sns.amazonaws.com + sns:Subscribe (protocol sqs).
3. cloudwatch:PutCompositeAlarm AlarmName=P, AlarmRule="ALARM(C1) AND ALARM(C2)", AlarmActions=[topic].
4. Poll cloudwatch:DescribeAlarms AlarmTypes=[CompositeAlarm] for P reaching ALARM and sqs:ReceiveMessage for the notification. Observed: neither happens; the check hits the 60 s cap.
5. Separately, PutCompositeAlarm with AlarmRule naming a non-existent alarm (and itself) must be refused "Could not save the composite alarm as alarms [<arn>] in the alarm rule do not exist", a 2-cycle between two EXISTING composites must be accepted 200 and stay inert, and no alarm referenced by a live rule may be deleted.

Cause 4 (ArithmeticError text):
1. cloudwatch:PutMetricData into a fresh namespace: metric A = [1,3,6,10,15] and metric B = [5,1,4,2,3] at five consecutive minute timestamps.
2. cloudwatch:GetMetricData with MetricStat queries m1=A, m2=B (ReturnData=false) plus expressions "m1/(m2-1)", "m1/0" and the control "m1/m2".
3. Expected for m1/(m2-1): StatusCode Complete, the index-1 timestamp dropped from BOTH Timestamps and Values, and exactly one Messages entry {Code: ArithmeticError, Value: "One or more data-points have been dropped due to non-numeric values (NaN, -Infinite, +Infinite)"}. Observed the same entry with "data points" instead of "data-points".

Cause 5 (ResourceNotFound message):
1. cloudwatch:PutMetricAlarm AlarmName=X over any namespace.
2. cloudwatch:DeleteAlarms AlarmNames=[X]; confirm DescribeAlarms returns nothing for X.
3. cloudwatch:SetAlarmState AlarmName=X StateValue=OK StateReason='probe'.
4. Expected HTTP 404 code ResourceNotFound with an EMPTY Message. Observed Message 'alarm X does not exist'.

Cause 6 (ActionsEnabled suppression):
1. sns:CreateTopic, sqs:CreateQueue, queue policy for sns.amazonaws.com conditioned on the topic ARN, sns:Subscribe protocol=sqs.
2. cloudwatch:PutMetricAlarm with AlarmActions=[topic], ActionsEnabled=true; cloudwatch:SetAlarmState to ALARM; sqs:ReceiveMessage must yield one notification (and its latency calibrates the absence window).
3. cloudwatch:DisableAlarmActions, SetAlarmState back to OK then to ALARM; no message may arrive over the calibrated window. cloudwatch:EnableAlarmActions and repeat: the message must arrive again.
4. Observed: step 2 never delivers, so the check runs to the 60 s cap.
5. Re-replay every check id in trace_ids to confirm the fixes.
