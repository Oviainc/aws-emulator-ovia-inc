Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cfn-deploy-missing-sqs-queue-policy-negative redshift__sqs.eventbridge-rule-missing-sqs-queue-policy-negative redshift__sqs.eventbridge-rule-missing-sqs-resource-policy-negative redshift__sqs.sqs-queue-policy-wrong-source-arn-condition-negative redshift__sqs.sqs-dead-letter-queue-captures-failed-eventbridge-delivery-positive redshift__sqs.sqs-dlq-for-redshift-event-rule-positive redshift__sqs.cfn-deploy-redshift-sqs-queue-policy-negative redshift__sqs.cfn-deploy-redshift-serverless-workgroup-sqs-notification-positive
Issue-Key: redshift__sqs.cfn-deploy-missing-sqs-queue-policy-negative|run#21|AssertionError|assert not leftover, f"unpolicied queue unexpectedly received a message: {leftover}"
Issue-Key: redshift__sqs.eventbridge-rule-missing-sqs-queue-policy-negative|run#32|AssertionError|assert not leaked, (
Issue-Key: redshift__sqs.eventbridge-rule-missing-sqs-resource-policy-negative|_assert_absent#14|AssertionError|raise AssertionError(f"expected no message on the policy-less queue, got {len(leaked)}")
Issue-Key: redshift__sqs.sqs-queue-policy-wrong-source-arn-condition-negative|_assert_absent#19|AssertionError|raise AssertionError(f"expected no message on the mis-scoped queue, got {len(leaked)}")
Issue-Key: redshift__sqs.sqs-dead-letter-queue-captures-failed-eventbridge-delivery-positive|_poll_messages#9|Timeout|resp = sqs.receive_message(
Issue-Key: redshift__sqs.sqs-dlq-for-redshift-event-rule-positive|_wait_for_matching_message#23|Timeout|resp = sqs.receive_message(
Issue-Key: redshift__sqs.cfn-deploy-redshift-sqs-queue-policy-negative|_poll_for_dlq_messages#10|Timeout|resp = sqs.receive_message(
Issue-Key: redshift__sqs|CreateStack|ValidationError
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# redshift__sqs: 3 net-new causes — EventBridge->SQS delivery ignores the queue policy, the EventBridge DLQ never captures a failed delivery, and AWS::RedshiftServerless resource types are unrecognized

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Three net-new root causes. Related open issue that does NOT carry these replay keys: 20261029T082153Z-redshift-sqs-a-data-api-statement-submitted-with.md.

Cause 1 of 4 … (see numbering below; three causes total)

Cause 1 of 3: an EventBridge rule's SQS target receives the event even when the destination queue's policy does not authorise it.
AWS: recorded behavior only. EventBridge assumes no role for an SQS target — delivery is authorised solely by the queue's resource policy. A queue with no policy at all, or whose policy's aws:SourceArn condition names a different rule, never receives the event (the attempt is counted as a failed invocation), while a sibling queue that does grant events.amazonaws.com sqs:SendMessage for that rule does.
Emulator: the unpolicied and mis-scoped queues both receive the full event envelope, and in one case the measured control latency was 0.0s, i.e. the policy is not consulted on the delivery path at all.
Why: this is over-permissive delivery — Redshift events leak to queues the account never authorised, and every negative test asserting "no policy, no delivery" passes vacuously, so an IaC regression that drops the queue policy or mis-scopes its SourceArn condition cannot be detected.
Fix: the EventBridge target-delivery path for SQS targets — evaluate the destination queue's resource policy, including its aws:SourceArn condition, before enqueuing.
Traces: redshift__sqs.cfn-deploy-missing-sqs-queue-policy-negative, redshift__sqs.eventbridge-rule-missing-sqs-queue-policy-negative, redshift__sqs.eventbridge-rule-missing-sqs-resource-policy-negative, redshift__sqs.sqs-queue-policy-wrong-source-arn-condition-negative

Cause 2 of 3: a target's DeadLetterConfig never receives the event whose delivery failed.
AWS: recorded behavior only; the model fixes the surface (events PutTargets accepts Target.DeadLetterConfig.Arn). When a target delivery fails permanently, EventBridge puts the event on the configured dead-letter queue with the failure metadata message attributes.
Emulator: nothing ever arrives on the dead-letter queue, so the three DLQ checks wait for it and are killed at the 60s harness budget. This is the mirror image of cause 1: deliveries that should fail succeed, and the failures that should be dead-lettered are never recorded.
Why: the dead-letter queue is the only durable record of events EventBridge could not deliver; with it empty, lost events are silently lost.
Fix: the EventBridge target-delivery failure path — on a permanent delivery failure, write the event to the target's DeadLetterConfig.Arn with the failure attributes.
Traces: redshift__sqs.sqs-dead-letter-queue-captures-failed-eventbridge-delivery-positive, redshift__sqs.sqs-dlq-for-redshift-event-rule-positive, redshift__sqs.cfn-deploy-redshift-sqs-queue-policy-negative

Cause 3 of 3: CloudFormation does not recognize the AWS::RedshiftServerless resource types.
AWS: recorded behavior only. CreateStack accepts a template declaring AWS::RedshiftServerless::Namespace and AWS::RedshiftServerless::Workgroup and provisions them.
Emulator: CreateStack is refused with HTTP 400 `ValidationError` and the message "Template format error: Unrecognized resource types: [AWS::RedshiftServerless::Namespace, AWS::RedshiftServerless::Workgroup]" (the fixed text is "Template format error: Unrecognized resource types: ", the bracketed list is taken from the template).
Why: the whole Redshift Serverless deployment path is unavailable, so no stack that uses it can be deployed or tested.
Fix: the CloudFormation resource-type registry and providers for AWS::RedshiftServerless::Namespace / ::Workgroup.
Traces: redshift__sqs.cfn-deploy-redshift-serverless-workgroup-sqs-notification-positive

## Evidence
Cause 1 — `AssertionError: unpolicied queue unexpectedly received a message: [{... 'Body': '{"version":"0",...,"detail-type":"Redshift Cluster Event Notification","source":"sim2real.redshift",...}'}]`; `AssertionError: queue without a policy received the event during the 20s absence window (measured control latency 0.0s): [...]`; `AssertionError: expected no message on the policy-less queue, got 1`; `AssertionError: expected no message on the mis-scoped queue, got 1`.
Cause 2 — `Error: Check timed out after 60.0s` on all three DLQ checks; nothing arrives on the DeadLetterConfig queue.
Cause 3 — `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::RedshiftServerless::Namespace, AWS::RedshiftServerless::Workgroup]`.

## Steps to Reproduce
Cause 1: PutRule on source 'aws.redshift' with two SQS targets — one queue granting events.amazonaws.com sqs:SendMessage for that rule, one with no policy (or with an aws:SourceArn condition naming a different rule). Emit a matching Redshift event; both queues receive it.
Cause 2: PutTargets with Target.DeadLetterConfig.Arn=<dlq> for a target whose delivery must fail; emit the event and poll the DLQ — nothing arrives.
Cause 3: CreateStack with a template declaring AWS::RedshiftServerless::Namespace and AWS::RedshiftServerless::Workgroup — ValidationError, unrecognized resource types.
