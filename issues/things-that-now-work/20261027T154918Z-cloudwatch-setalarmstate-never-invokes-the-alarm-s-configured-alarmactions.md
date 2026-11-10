Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action|_drain_queue#27|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 48b00a2ec8a92094223b124c2e7d5eacc8c9104d

# SetAlarmState never invokes the alarm's configured AlarmActions, so a forced ALARM transition delivers no SNS notification and DisableAlarmActions/EnableAlarmActions suppression is unobservable

> _Note: this issue was identified against the code base at commit `48b00a2ec8a92094223b124c2e7d5eacc8c9104d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: a metric alarm created by PutMetricAlarm with AlarmActions=[an SNS topic ARN] and the default ActionsEnabled=true never dispatches that action when SetAlarmState forces it into ALARM. The forced state change is accepted, but nothing is published to the topic, so nothing reaches an SQS queue subscribed to it. Because no delivery happens even in the positive-control leg (actions still enabled, untouched by DisableAlarmActions), the ActionsEnabled suppression semantics cannot be measured at all: a target that silently never dispatches is indistinguishable from one that correctly suppresses only the muted alarm.

AWS: Smithy SetAlarmState (SetAlarmStateInput: AlarmName required @length 1..255, StateValue required enum, StateReason required @length 0..1023, StateReasonData @length 0..4000; modeled errors InvalidFormatFault -> awsQueryError code InvalidFormat httpResponseCode 400, and ResourceNotFound -> code ResourceNotFound httpResponseCode 404) is documented in the model itself as: "Temporarily sets the state of an alarm for testing purposes. When the updated state differs from the previous value, the action configured for the appropriate state is invoked. For example, if your alarm is configured to send an Amazon SNS message when an alarm is triggered, temporarily changing the alarm state to ALARM sends an SNS message." So a SetAlarmState call that moves the alarm from its current state to ALARM must invoke every ARN in the alarm's AlarmActions. For an SNS topic action, the topic receives a notification whose Message is the standard CloudWatch alarm JSON: AlarmName = the alarm named in the request, OldStateValue = the state before the call, NewStateValue = the StateValue in the request, StateReason = the StateReason in the request, and Trigger.Namespace/Trigger.MetricName taken from the alarm's own configuration — all variable parts come from the request and from stored alarm state, none of them from any one run. DisableAlarmActions (DisableAlarmActionsInput.AlarmNames required) suppresses ONLY that dispatch; EnableAlarmActions restores it for the next differing transition.

Emulator: the alarm is created, DisableAlarmActions/EnableAlarmActions do flip ActionsEnabled as read back by DescribeAlarms, and SetAlarmState returns 200 — but no action is ever invoked. The check is killed at the harness 60 s cap while still inside its FIRST wait, the positive control `_wait_for_messages(ctx, queue["url"], expected_count=1, timeout_s=90.0)` that follows `cw.set_alarm_state(AlarmName=alarm_name, StateValue="ALARM", StateReason="behavioral-check-baseline")` on an alarm whose actions have never been touched. Zero messages arrive on the subscribed queue, so the latency calibration, the suppressed-window assertion and the post-EnableAlarmActions redelivery assertion are never reached.

Why: a client that uses SetAlarmState to test its alarm wiring — the documented purpose of the operation — observes total silence and concludes its SNS topic, subscription or queue policy is broken. Paging, auto-remediation and deployment-rollback flows wired to AlarmActions can never be exercised, and code that depends on DisableAlarmActions suppressing delivery while the transition still occurs cannot be validated: the emulator's silence is total rather than selective, which is the opposite of the contract.

Fix: the alarm action-dispatch path in the cloudwatch service — the SetAlarmState handler must, when the new StateValue differs from the stored one, write the new state/StateReason and then run the alarm's action dispatch (resolving each AlarmActions/OKActions/InsufficientDataActions ARN and performing the SNS publish) gated on the stored ActionsEnabled flag, rather than only mutating alarm state. This is dispatch-on-forced-transition, independent of the periodic evaluator.

Related but NOT covering this: open issue 20261027T130355Z-cloudwatch-sns-sqs-a-metric-alarm-with-disablealarmactions.md and 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md file the absent PERIODIC EVALUATION of published datapoints (no transition at all); this check never publishes a datapoint and never waits for evaluation — it forces the transition with SetAlarmState, which the emulator accepts, and still gets no action. Open issue 20261025T084321Z-cloudwatch-setalarmstate-s-forced-alarm-state-is-never-reclaimed.md covers the forced state not being reclaimed afterwards, not the missing action invocation. None of them carries this check's replay key.

Traces: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action.

## Evidence
Failing check: [mismatch] cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action
Reported detail: "Error: Check timed out after 60.0s".

Expected (AWS): after PutMetricAlarm(..., AlarmActions=[topic ARN]) and SetAlarmState(AlarmName=alarm, StateValue='ALARM', StateReason='behavioral-check-baseline'), the SNS topic is published to and the subscribed SQS queue yields exactly one message whose Body is the SNS envelope and whose Message JSON carries AlarmName = the alarm just created, NewStateValue = 'ALARM' and StateReason = the StateReason sent in the request. After DisableAlarmActions, a fresh OK->ALARM SetAlarmState delivers NOTHING for at least 3x the latency measured in the control leg. After EnableAlarmActions, the next OK->ALARM SetAlarmState delivers exactly one message again, with Message.NewStateValue='ALARM'.

Actual (emulator): SetAlarmState returns success and DescribeAlarms reflects the forced state, but no SNS publish occurs: the queue stays empty through the whole 60 s budget, and the check dies inside the positive-control wait (`_wait_for_messages(..., expected_count=1, timeout_s=90.0)` at the baseline step) before it can even disable actions. SQS SetQueueAttributes(Policy allowing sns.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn), SNS Subscribe(Protocol='sqs'), PutMetricAlarm, DisableAlarmActions, EnableAlarmActions and DescribeAlarms all succeed, so the missing piece is the action invocation on a forced state change, not the wiring or the ActionsEnabled flag's storage.

## Steps to Reproduce
1. SQS CreateQueue; GetQueueAttributes(AttributeNames=['QueueArn']) for its ARN.
2. SNS CreateTopic; SQS SetQueueAttributes with a Policy allowing Principal Service sns.amazonaws.com, Action sqs:SendMessage, Resource = the queue ARN, Condition ArnEquals aws:SourceArn = the topic ARN; SNS Subscribe(TopicArn=topic, Protocol='sqs', Endpoint=queue ARN).
3. CloudWatch PutMetricAlarm(AlarmName=A, Namespace=NS, MetricName='Errors', Statistic='Average', Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator='GreaterThanThreshold', AlarmActions=[topic ARN], TreatMissingData='missing'). Do NOT call DisableAlarmActions.
4. SetAlarmState(AlarmName=A, StateValue='ALARM', StateReason='behavioral-check-baseline').
5. ReceiveMessage(QueueUrl=queue, MaxNumberOfMessages=10, WaitTimeSeconds=5) in a loop.
   Expected (AWS): one message within seconds, Message.AlarmName = A, Message.NewStateValue = 'ALARM'.
   Observed (emulator): no message ever arrives; the loop runs until the harness 60 s cap.
6. (Never reached, but equally affected) SetAlarmState back to OK, DisableAlarmActions(AlarmNames=[A]), SetAlarmState to ALARM -> must deliver nothing; EnableAlarmActions(AlarmNames=[A]), SetAlarmState OK then ALARM -> must deliver exactly one message with NewStateValue='ALARM'.
