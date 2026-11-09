Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-mute-rule-active-window-boundary-inclusive cloudwatch.alarm-mute-rule-active-window-suppresses-actions-mid-ttl cloudwatch.alarm-mute-rule-suppresses-actions-during-window cloudwatch.e2e-composite-alarm-mute-rule-suppresses-actions-flow cloudwatch.e2e-composite-alarm-mute-suppresses-action-then-cleanup cloudwatch.deploy-cloudformation-alarm-behaves-on-metric-breach cloudwatch.deploy-cloudformation-alarm-update-changes-threshold-behavior cloudwatch.deploy-cloudformation-metric-alarm-behaves cloudwatch.deploy-cloudformation-metric-alarm-triggers-on-breach cloudwatch.alarm-evaluation-engine-fewer-breaches-than-evaluation-periods-withhold-alarm cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data cloudwatch.alarm-missing-data-and-m-of-n-period-window-slides-by-minute-and-never-evaluates-a-partial-period cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-mute-rule-active-window-boundary-inclusive|run#35|AssertionError|assert muted_messages == [], (
Issue-Key: cloudwatch.alarm-mute-rule-active-window-suppresses-actions-mid-ttl|run#28|AssertionError|assert muted_messages == [], f"alarm action fired while mute rule was active mid-window: {muted_messages}"
Issue-Key: cloudwatch.alarm-mute-rule-suppresses-actions-during-window|run#22|AssertionError|assert not muted_messages, f"expected no SNS delivery while muted, got {muted_messages}"
Issue-Key: cloudwatch.e2e-composite-alarm-mute-rule-suppresses-actions-flow|run#25|AssertionError|assert muted_messages == [], (
Issue-Key: cloudwatch.e2e-composite-alarm-mute-suppresses-action-then-cleanup|run#29|AssertionError|assert leaked == [], f"action fired while alarm was muted: {leaked}"
Issue-Key: cloudwatch.deploy-cloudformation-alarm-behaves-on-metric-breach|run#15|AssertionError|assert len(described) == 1, "stack claims CREATE_COMPLETE but alarm resource is absent"
Issue-Key: cloudwatch.deploy-cloudformation-alarm-update-changes-threshold-behavior|_poll_alarm_state#13|TimeoutError|raise TimeoutError(
Issue-Key: cloudwatch.deploy-cloudformation-metric-alarm-behaves|run#8|AssertionError|assert len(alarms) == 1, f"CloudFormation did not create the metric alarm {alarm_name!r}"
Issue-Key: cloudwatch.deploy-cloudformation-metric-alarm-triggers-on-breach|run#12|AssertionError|assert len(alarms) == 1, "CFN-created alarm not visible via DescribeAlarms"
Issue-Key: cloudwatch.alarm-evaluation-engine-fewer-breaches-than-evaluation-periods-withhold-alarm|run#24|AssertionError|assert value == _LOW and published.get(ts) == _LOW, (
Issue-Key: cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data|run#29|AssertionError|assert data["oldState"] == items[idx - 1][1]["newState"], (
Issue-Key: cloudwatch.alarm-missing-data-and-m-of-n-period-window-slides-by-minute-and-never-evaluates-a-partial-period|run#11|AssertionError|raise AssertionError(
Issue-Key: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120|run#41|AssertionError|assert sl_start < boundary, (
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch: 6 net-new causes in alarm evaluation and action dispatch — mute rules never suppress actions, CFN-deployed alarms are absent, the OK state reason cites breaching datapoints, the history chain breaks, period windows are skipped and the sliding window is period-aligned

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 6: an ACTIVE alarm mute rule does not suppress alarm actions.
AWS: while a mute rule whose MuteTargets name the alarm (or its composite) is in its active window, CloudWatch evaluates the alarm and records the transition but does NOT execute AlarmActions/OKActions, so no SNS notification is published for a transition inside the window; the suppression applies at the window boundaries inclusively and for the whole TTL. Smithy (cloudwatch 2010-08-01): PutAlarmMuteRule(Name, Rule, MuteTargets, StartDate, ExpireDate) is the write that arms this; the suppression effect is recorded behavior.
Emulator: the transition's notification is delivered to the SNS topic (and on to SQS) while the rule is active — at the inclusive boundary, mid-TTL, for a plain metric alarm and for a composite alarm. The emulator stores the mute rule and ignores it in the action dispatcher.
Why: a customer who mutes an alarm for a maintenance window is still paged. Mute rules are write-only decoration, so the entire suppression feature is unusable and on-call noise suppression silently fails.
Fix: the alarm action-dispatch component — before executing AlarmActions/OKActions/InsufficientDataActions it must consult the active alarm mute rules whose MuteTargets match the alarm and skip delivery (while still recording the state transition).
Failing checks: cloudwatch.alarm-mute-rule-active-window-boundary-inclusive, cloudwatch.alarm-mute-rule-active-window-suppresses-actions-mid-ttl, cloudwatch.alarm-mute-rule-suppresses-actions-during-window, cloudwatch.e2e-composite-alarm-mute-rule-suppresses-actions-flow, cloudwatch.e2e-composite-alarm-mute-suppresses-action-then-cleanup.

Cause 2 of 6: an AWS::CloudWatch::Alarm in a deployed stack is never created in CloudWatch.
AWS: CloudFormation provisions each alarm resource as a PutMetricAlarm, so after CREATE_COMPLETE DescribeAlarms(AlarmNames=[<deployed name>]) returns exactly one MetricAlarm, which then evaluates published datapoints and transitions on breach; an UpdateStack that changes Threshold re-puts it and the new threshold governs.
Emulator: the stack claims CREATE_COMPLETE but DescribeAlarms for the deployed name returns [] ('stack claims CREATE_COMPLETE but alarm resource is absent', 'CloudFormation did not create the metric alarm', 'CFN-created alarm not visible via DescribeAlarms'), and the update arm times out waiting for a state because the alarm does not exist ('did not reach [ALARM, OK] within 600.0s; last observed state: None').
Why: infrastructure-as-code monitoring silently does nothing: the stack reports success and no alarm exists, so no breach is ever detected and no action ever fires.
Fix: the CloudFormation AWS::CloudWatch::Alarm resource handler must write the alarm into CloudWatch state on create/update and remove it on delete. Same class as the open issue 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md, which does not carry these replay keys.
Failing checks: cloudwatch.deploy-cloudformation-alarm-behaves-on-metric-breach, cloudwatch.deploy-cloudformation-alarm-update-changes-threshold-behavior, cloudwatch.deploy-cloudformation-metric-alarm-behaves, cloudwatch.deploy-cloudformation-metric-alarm-triggers-on-breach.

Cause 3 of 6: the OK StateReason of an M-of-N alarm cites datapoints that DID breach the threshold.
AWS: the OK reason reads 'Threshold Crossed: <k> out of the last <n> datapoints [<value> (<ts>), ...] was not greater than the threshold (<threshold>) (minimum <m> datapoint for ALARM -> OK transition).' — the fixed text is that sentence; the variable parts (k, n, the cited value/timestamp pairs, the threshold, the minimum) come from the request and the evaluated window. Only datapoints that are NOT greater than the threshold may be cited in the 'was not greater than the threshold' list.
Emulator: for a 3-of-3 alarm whose window held two breaching values and one in-range value, the OK reason cites a BREACHING value (the recorded run shows the high value 90.0 against threshold 50.0). The recentDatapoints set is right; the citation list is drawn from the wrong subset.
Why: the state reason is the only explanation a caller gets for a decision. Citing a breaching datapoint as 'not greater than the threshold' makes the alarm's own account of itself self-contradictory, and any tooling that parses the reason to find the deciding datapoints reads the wrong ones.
Fix: the alarm evaluator's state-reason renderer — restrict the cited datapoint list to the datapoints on the side of the comparison the new state reflects.
Failing check: cloudwatch.alarm-evaluation-engine-fewer-breaches-than-evaluation-periods-withhold-alarm.

Cause 4 of 6: consecutive entries in DescribeAlarmHistory do not chain — a transition's oldState is not the previous transition's newState.
AWS: alarm history is a chain: each StateUpdate item's oldState equals the newState of the item before it (same stateValue, stateReason and stateReasonData), so a caller can replay the alarm's life from the history alone.
Emulator: transition 1's oldState carries a different stateReason/stateReasonData from the preceding transition's newState (the recorded run shows an oldState reason over a 1-of-3 window where the previous newState's reason was over a 3-of-3 window), so the chain is broken even though both say 'OK'.
Why: the history is the audit trail for why an alarm moved. A non-chaining history makes it impossible to reconstruct the sequence of evaluations, and tools that diff old/new states report phantom changes.
Fix: the alarm state-transition recorder — it must persist the exact prior state record as the new item's oldState rather than re-deriving it.
Failing check: cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data.

Cause 5 of 6: a Period=300 alarm evaluates fewer windows than the elapsed wall clock contains.
AWS: an alarm re-evaluates once per period; over an elapsed span of N periods it produces N evaluated windows, and it never grades a partial (unclosed) period.
Emulator: over 540 s (nine minutes, i.e. at least four closed 300 s windows after the first) the alarm recorded only 3 evaluated windows, so the evaluation tick is slower than the configured period.
Why: alarms react later than their period promises. DatapointsToAlarm/EvaluationPeriods arithmetic over a sliding window becomes unpredictable, and a breach can sit undetected for multiples of the period.
Fix: the alarm evaluator's scheduling component — tick each alarm once per its Period, advancing the window by one period each time.
Failing check: cloudwatch.alarm-missing-data-and-m-of-n-period-window-slides-by-minute-and-never-evaluates-a-partial-period.

Cause 6 of 6: the default (sliding) evaluation window is aligned to the period boundary instead of rolling to now.
AWS: without EvaluationWindow.WallClockWindow an alarm evaluates a ROLLING window that ends at the current evaluation instant and therefore reaches back across the previous period boundary; only a wall-clock window is anchored to aligned period boundaries.
Emulator: the sliding twin evaluated from the aligned boundary (the recorded run shows a window starting exactly at the top of the hour) rather than from a point before that boundary with the window ending at the evaluation instant. Note this is the mirror image of the already-fixed issue 20261028T085300Z-cloudwatch-ignores-evaluationwindow-wallclockwindow-a-wall-clock-alarm.md: the two window kinds are now swapped.
Why: the two window kinds are the whole point of EvaluationWindow. Collapsing sliding onto the aligned grid delays detection by up to a period and makes the wall-clock opt-in meaningless, and it also distorts the Period=120 rejection arm that rides on the same alarm.
Fix: the alarm evaluator's window-selection component — derive the sliding window end from the evaluation instant and only snap to aligned boundaries when WallClockWindow is set.
Failing check: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120.

## Evidence
Cause 1 — Expected: no SNS/SQS delivery for a transition that occurs while a matching mute rule is active. Actual: 'AssertionError: alarm action fired while mute rule was active mid-window: [...Type: Notification, Subject: ALARM: "<alarm>" in us-west-2...]', 'AssertionError: expected no SNS delivery while muted, got [...]', 'AssertionError: alarm action was NOT muted during the ACTIVE window (waited 15.1s, >= 3x the 5.0s positive-control delivery latency)', 'AssertionError: alarm action fired while mute rule was ACTIVE: [...]' (message ids, ARNs and timestamps are per-run).
Cause 2 — Expected: one MetricAlarm from DescribeAlarms after CREATE_COMPLETE. Actual: 'AssertionError: stack claims CREATE_COMPLETE but alarm resource is absent'; 'AssertionError: CloudFormation did not create the metric alarm'; 'AssertionError: CFN-created alarm not visible via DescribeAlarms'; 'TimeoutError: alarm <name> did not reach [\'ALARM\', \'OK\'] within 600.0s; last observed state: None'.
Cause 3 — Expected: the OK reason's cited datapoints are all on the non-breaching side of the threshold. Actual: 'AssertionError: the OK reason cites 90.0 at <ts>; published 90.0' where the threshold is 50.0 and the in-range value is 12.0.
Cause 4 — Expected: history item n's oldState == item n-1's newState. Actual: 'AssertionError: transition 1\'s oldState {...stateReason: \'Threshold Crossed: 1 out of the last 3 datapoints ...\'} is not the previous transition\'s newState {...stateReason: \'Threshold Crossed: 3 out of the last 3 datapoints ...\'}'.
Cause 5 — Expected: >= 4 evaluated windows for Period=300 over 540 s. Actual: 'AssertionError: a Period=300 alarm over a flapping per-minute series recorded 3 evaluated windows in 540s, expected at least 4'.
Cause 6 — Expected: a rolling window ending at the evaluation instant and reaching back before the aligned boundary. Actual: 'AssertionError: the sliding twin evaluated from <aligned boundary>, expected a range reaching back before the <aligned boundary> boundary (a rolling window ending at <now>)'.

## Steps to Reproduce
Cause 1: PutMetricAlarm with an SNS AlarmAction subscribed by SQS; PutAlarmMuteRule naming that alarm with a window covering now; SetAlarmState (or drive a real breach) to ALARM; observe the SNS notification is delivered to the queue. Repeat at the window's inclusive start/end and for a composite alarm.
Cause 2: CreateStack with an AWS::CloudWatch::Alarm resource; wait for CREATE_COMPLETE; DescribeAlarms(AlarmNames=[<deployed name>]) -> [].
Cause 3: PutMetricAlarm(Statistic=Maximum, Period=10, EvaluationPeriods=3, DatapointsToAlarm=3, Threshold=50, ComparisonOperator=GreaterThanThreshold); publish a repeating pattern of two breaching and one in-range datapoint per window; read DescribeAlarmHistory for the OK transition and inspect the datapoints cited in StateReason.
Cause 4: drive several transitions on one alarm and compare each history item's oldState with the previous item's newState.
Cause 5: PutMetricAlarm with Period=300 over a per-minute flapping series; count distinct evaluated windows in StateReasonData over 540 s.
Cause 6: PutMetricAlarm without EvaluationWindow, publish datapoints and inspect StateReasonData.startDate/queryDate relative to the period boundary and to the evaluation instant.
