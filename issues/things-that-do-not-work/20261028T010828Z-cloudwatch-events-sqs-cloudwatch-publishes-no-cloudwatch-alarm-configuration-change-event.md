Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.xsvc-cloudwatch-2-rejected-composite-alarm-emits-no-configuration-change-event
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-2-rejected-composite-alarm-emits-no-configuration-change-event|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 2882fa01447cddf34bfea2947358b39ec0b8885e

# CloudWatch publishes no 'CloudWatch Alarm Configuration Change' event for composite-alarm writes, so the rejected-composite leakage check never gets a live rule->queue path (1 cause)

> _Note: this issue was identified against the code base at commit `2882fa01447cddf34bfea2947358b39ec0b8885e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutCompositeAlarm

## Description
Cause 1 of 1 — a CloudWatch alarm write emits no `aws.cloudwatch` / "CloudWatch Alarm Configuration Change" event onto the default EventBridge bus, so nothing is ever delivered to a rule whose target is an SQS queue.

AWS: a PutMetricAlarm or PutCompositeAlarm that actually changes an alarm's configuration (and DeleteAlarms) emits a service event on the account's default event bus with `source` = "aws.cloudwatch" and `detail-type` = "CloudWatch Alarm Configuration Change". `resources` is exactly the alarm ARN (`arn:aws:cloudwatch:<region>:<account>:alarm:<alarmName>`); `detail.alarmName` is the alarm name from the request; `detail.operation` is "create", "update" or "delete"; `detail.configuration` carries the configuration in force (for a metric alarm the `threshold` just written; for a COMPOSITE alarm `alarmRule` and `actionsEnabled`, and no `metrics` member, because a composite evaluates a rule and not a metric); on an update `detail.previousConfiguration.alarmRule` is the pre-update rule, and a create carries no `previousConfiguration` at all. Smithy: the CloudWatch model (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) models no event emission at all — PutCompositeAlarm's only modeled error is LimitExceededFault (awsQueryError code `LimitExceeded`, HTTP 400) — so the event contract is recorded/documented behavior only, not a modeled shape.

Emulator: nothing is delivered. The check's setup all succeeds — CreateQueue, GetQueueAttributes, SetQueueAttributes with a policy admitting events.amazonaws.com for the rule ARN, PutRule(State=ENABLED) with a pattern on source `aws.cloudwatch` / detail-type "CloudWatch Alarm Configuration Change" / `detail.alarmName`, and PutTargets reporting FailedEntryCount 0 — and then the warm-up helper `_prove_path_live` issues PutMetricAlarm on the child alarm with threshold 1, 2, 3, ... and drains the queue with ReceiveMessage after each write, requiring five CONSECUTIVE writes each to deliver their own matching event. Not one event arrives, the warm-up loop never advances past 0 consecutive hits, and the replay cuts the check off: "Check timed out after 60.0s".

Why: a client that wires alarm-configuration drift detection or change auditing (EventBridge rule on "CloudWatch Alarm Configuration Change" -> queue/Lambda) sees alarms being created, re-thresholded and deleted in CloudWatch while the rule reports perfectly healthy and no event ever lands — there is no error anywhere to diagnose. It also means the emulator cannot be used to verify the inverse guarantee this check exists for: that a REJECTED PutCompositeAlarm leaks no event onto the bus, since a target that emits nothing at all is indistinguishable from one that correctly emits nothing on refusal.

Fix: the CloudWatch alarm-event publisher in services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801 — the PutMetricAlarm / PutCompositeAlarm / DeleteAlarms handlers must emit the configuration-change event (with operation, configuration and previousConfiguration populated from the alarm state before and after the write) — plus the EventBridge default-bus ingestion of `aws.cloudwatch` service events and its SQS target dispatch.

Related but NOT covering this: open issue 20261025T002357Z-cloudwatch-events-sqs-cloudwatch-emits-no-alarm-events-to-eventbridge-no.md carries the sibling checks (alarm STATE-change events, the tagging/no-op configuration-change negatives, and AWS/Events FailedInvocations) under its own replay keys; this check's key is not on it, and its composite-alarm legs — configuration.alarmRule, the absence of configuration.metrics, previousConfiguration.alarmRule on update — are graded by no check it owns.

Not yet graded, and therefore not claimed here as a separate defect: everything after the warm-up. Once events flow, the same check additionally requires that PutCompositeAlarm whose AlarmRule names an alarm that does not exist is refused with error code `ValidationError` and the message whose fixed text is "Could not save the composite alarm as alarms [" ... "] in the alarm rule do not exist", the bracketed part being the alarm ARNs built from the non-existent names in the request's AlarmRule (arn:aws:cloudwatch:<region>:<account>:alarm:<name>), that the refused write creates nothing (DescribeAlarms with AlarmTypes=["CompositeAlarm"] returns an empty CompositeAlarms list), and that exactly the operations ["create", "update", "delete"] reach the queue for the valid composite written under the same name.

Traces: cloudwatch__events__sqs.xsvc-cloudwatch-2-rejected-composite-alarm-emits-no-configuration-change-event.

## Evidence
Replay verdict: [mismatch] cloudwatch__events__sqs.xsvc-cloudwatch-2-rejected-composite-alarm-emits-no-configuration-change-event — "Error: Check timed out after 60.0s".

Expected (real AWS, us-west-2 as recorded by the check): after PutMetricAlarm(AlarmName=<child>, Namespace='Sim2Real/XsvcCw2', MetricName='CompositeChildProbe', Threshold=n), ReceiveMessage on the rule's queue returns a message whose JSON body has source='aws.cloudwatch', detail-type='CloudWatch Alarm Configuration Change', resources=[<child alarm ARN>], detail.alarmName=<child> and detail.configuration.threshold=float(n) — repeatably, five writes in a row.

Actual: ReceiveMessage(QueueUrl=<queue>, MaxNumberOfMessages=10, WaitTimeSeconds=2) returns no Messages on every poll for the whole 60 s budget, for every one of the warm-up writes. The preceding calls all returned 2xx: CreateQueue, GetQueueAttributes(QueueArn), SetQueueAttributes(Policy), PutRule, PutTargets (FailedEntryCount == 0), PutMetricAlarm. No error code or non-2xx status is produced anywhere — the divergence is a silently absent event, not a refusal.

## Steps to Reproduce
1. CreateQueue(QueueName=q) and read its QueueArn via GetQueueAttributes; SetQueueAttributes(q, Policy allowing Principal {"Service":"events.amazonaws.com"} Action sqs:SendMessage on that ARN with Condition ArnEquals aws:SourceArn = the rule ARN below).
2. PutRule(Name=r, State='ENABLED', EventPattern='{"source":["aws.cloudwatch"],"detail-type":["CloudWatch Alarm Configuration Change"],"detail":{"alarmName":["<child>","<composite>"]}}'); PutTargets(Rule=r, Targets=[{"Id":"q","Arn":<queue arn>}]) -> FailedEntryCount 0.
3. PutMetricAlarm(AlarmName=<child>, Namespace='Sim2Real/XsvcCw2', MetricName='CompositeChildProbe', Statistic='Sum', Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator='GreaterThanOrEqualToThreshold', ActionsEnabled=False, TreatMissingData='missing').
4. Poll ReceiveMessage(QueueUrl=q, MaxNumberOfMessages=10, WaitTimeSeconds=2) -> no message ever arrives. Repeat step 3 with Threshold=2.0, 3.0 ... -> still nothing; on AWS each write delivers its own 'CloudWatch Alarm Configuration Change' event carrying detail.configuration.threshold equal to the threshold just written.
5. PutCompositeAlarm(AlarmName=<composite>, AlarmRule='ALARM("<child>")', ActionsEnabled=False) then PutCompositeAlarm with AlarmRule='ALARM("<child>") OR INSUFFICIENT_DATA("<child>")' then DeleteAlarms(AlarmNames=[<composite>]) -> no create/update/delete events either.
