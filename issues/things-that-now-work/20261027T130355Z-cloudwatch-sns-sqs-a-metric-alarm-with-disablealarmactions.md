Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked
Issue-Key: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked|_wait_for#35|Timeout|time.sleep(interval_s)
Issue-Commit: 1f1c94accd1d7961aa84a7b9995986ca278396b6

# cloudwatch__sns__sqs: a metric alarm with DisableAlarmActions never evaluates published datapoints, so no transition, no SNS->SQS notification and no Action/StateUpdate history (1 cause)

> _Note: this issue was identified against the code base at commit `1f1c94accd1d7961aa84a7b9995986ca278396b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: a metric alarm registered with PutMetricAlarm never advances its state from the datapoints PutMetricData ingested, so neither the ActionsEnabled=true twin nor the DisableAlarmActions-muted twin ever transitions, nothing is published to the SNS topic (and therefore nothing reaches the SQS queue subscribed to it), and DescribeAlarmHistory records neither StateUpdate nor Action items. The ActionsEnabled suppression semantics this check exists to measure are consequently unobservable: the emulator delivers nothing for either alarm, so a target that simply never evaluates is indistinguishable from one that correctly suppresses only the muted alarm.

AWS: Smithy PutMetricAlarm (PutMetricAlarmInput: AlarmName @length 1..255 required, Namespace @length 1..255 @pattern '^[^:]', MetricName, Statistic, Period @range min 1, EvaluationPeriods @range min 1, Threshold, ComparisonOperator, DatapointsToAlarm, TreatMissingData @length 1..255, AlarmActions @length 0..5; only modeled error LimitExceededFault, httpError 400, awsQueryError code LimitExceeded) registers an alarm that CloudWatch then evaluates every Period against the stored datapoints for that Namespace/MetricName/Dimensions set. The state machine moves INSUFFICIENT_DATA -> OK/ALARM; DescribeAlarms reports the new StateValue with a StateReason whose fixed text is the "Threshold Crossed: N datapoint(s) (<values>) was ... the threshold (<threshold>)." template, whose variable parts (the datapoint values and the threshold) come from the request and the ingested data; DescribeAlarmHistory (Smithy: HistoryItemType enum including StateUpdate and Action, AlarmHistoryItems paginated by NextToken/MaxRecords @range 1..100) records a StateUpdate item whose HistoryData newState.stateValue is the new state, plus an Action item once an action is dispatched; and each ARN in AlarmActions is invoked, so an SNS topic receives a notification whose Message JSON carries AlarmName, OldStateValue, NewStateValue and Trigger.Namespace/Trigger.MetricName taken from the alarm's own configuration. DisableAlarmActions (Smithy: DisableAlarmActionsInput.AlarmNames required @length 0..100, no modeled errors) clears ActionsEnabled and suppresses ONLY the action dispatch — evaluation and the StateUpdate history item still occur — and EnableAlarmActions restores dispatch from the next transition onward. Recorded behavior: the check publishes a breaching datapoint (value above the threshold) in a loop and polls DescribeAlarms for StateValue=ALARM on both twins, then drains the SQS queue for the enabled twin's notification.

Emulator: no evaluation happens at all. The check is killed at the harness 60 s cap inside its very first wait, `_wait_for("both alarms to reach ALARM from the same breaching datapoints", ...)`, while it keeps calling PutMetricData with the breaching value; neither alarm ever leaves its creation state, so the positive control (the enabled twin's ALARM notification arriving on the queue), the muted twin's StateUpdate-into-ALARM history item, the empty-Action-history assertion and the post-EnableAlarmActions OK->ALARM redelivery are all never reached.

Why: a client that creates an alarm over its own metric and waits for it to fire waits forever, and the SNS->SQS fan-out behind it is silent. Paging, auto-remediation and rollback flows wired to AlarmActions never trigger, DescribeAlarmHistory shows an alarm that looks permanently un-evaluated, and — specific to this cause — code that relies on DisableAlarmActions suppressing delivery while still recording the transition cannot be tested or trusted: the emulator's silence is total rather than selective.

Fix: the CloudWatch alarm evaluation engine — the periodic evaluator that joins stored metric datapoints to registered alarms, computes the m-of-n verdict over Period/EvaluationPeriods/DatapointsToAlarm, writes StateValue/StateReason/StateUpdatedTimestamp, appends StateUpdate and Action alarm-history items, and dispatches AlarmActions (the SNS publish path) while honouring the ActionsEnabled flag set by DisableAlarmActions/EnableAlarmActions — in the cloudwatch service's alarm state and its PutMetricAlarm / DisableAlarmActions / EnableAlarmActions / DescribeAlarmHistory handlers.

Related but NOT covering this: open issue 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md files the same symptom for the pure `cloudwatch` service token, and 20261024T030130Z-cloudwatch-rds-cloudwatch-metric-alarms-never-evaluate-or-fire-their.md / 20261027T020742Z-cloudwatch-rds-metric-alarms-never-evaluate-or-execute.md file it for cloudwatch__rds. None of them carries this check's replay key, and none of them exercises the DisableAlarmActions-vs-enabled-twin split across the SNS->SQS delivery path.

Traces: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked.

## Evidence
Failing check: [mismatch] cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked
Reported detail: "Error: Check timed out after 60.0s".

Expected (AWS): after repeated PutMetricData of a breaching value into the alarm's Namespace/MetricName/Dimensions, DescribeAlarms(AlarmNames=[enabled, muted], AlarmTypes=["CompositeAlarm","MetricAlarm"]) reports StateValue=ALARM for BOTH alarms with a StateReason citing the published datapoint; the enabled twin's SNS notification arrives on the subscribed SQS queue with TopicArn = the topic, Message.AlarmName = the enabled alarm, Message.OldStateValue=INSUFFICIENT_DATA, Message.NewStateValue=ALARM and Message.Trigger.Namespace/MetricName = the alarm's namespace/metric; the muted twin shows a StateUpdate history item with HistoryData newState.stateValue=ALARM, delivers nothing, and has an EMPTY Action history; after EnableAlarmActions the next OK->ALARM transition is delivered with Message.OldStateValue=OK.

Actual (emulator): both alarms stay in their creation state indefinitely. The check never gets past its first poll loop — `_wait_for("both alarms to reach ALARM from the same breaching datapoints", lambda: {n: _alarm(cw, n) for n in (enabled, muted)}, lambda d: all(a["StateValue"] == "ALARM" for a in d.values()), budget(), each=lambda: _put(cw, namespace, metric, dims, _BREACH))` — and is killed at the 60 s cap. DisableAlarmActions/EnableAlarmActions and DescribeAlarms return successfully (ActionsEnabled flips as expected), so the defect is the absent evaluation/dispatch, not the flag's storage. No message is ever observed on the SQS queue and no Action or StateUpdate history item is produced.

## Steps to Reproduce
1. CreateTopic (SNS) and CreateQueue (SQS); set a queue Policy allowing sns.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn = the topic ARN; Subscribe the queue ARN to the topic with Protocol=sqs.
2. PutMetricAlarm twice over the SAME namespace/metric/dimension set (one alarm named 'enabled', one named 'muted'): Statistic=Maximum, Period=10, EvaluationPeriods=1, Threshold=50, ComparisonOperator=GreaterThanThreshold, TreatMissingData=ignore, AlarmActions=[topic ARN].
3. DisableAlarmActions(AlarmNames=['muted']). DescribeAlarms confirms ActionsEnabled=false on 'muted' and true on 'enabled' — this part works.
4. Repeatedly PutMetricData(Namespace=NS, MetricData=[{MetricName=M, Dimensions=dims, Value=211.0, StorageResolution=1}]) and poll DescribeAlarms(AlarmNames=['enabled','muted']).
   Observed: both StateValue values never change; AWS moves both to ALARM with a StateReason citing the 211.0 datapoint.
5. ReceiveMessage on the queue: no notification for 'enabled' ever arrives (AWS delivers one with NewStateValue=ALARM, OldStateValue=INSUFFICIENT_DATA).
6. DescribeAlarmHistory(AlarmName='muted', HistoryItemType='StateUpdate', AlarmTypes=['MetricAlarm']): no item (AWS records the transition into ALARM); HistoryItemType='Action' is correctly empty only because nothing evaluated at all.
7. EnableAlarmActions(AlarmNames=['muted']), drive the metric to 2.0 then back to 211.0: the muted alarm never returns to OK and never re-enters ALARM, so the restored OK->ALARM delivery never happens.
