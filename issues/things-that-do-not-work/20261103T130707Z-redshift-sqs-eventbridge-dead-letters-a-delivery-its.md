Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cfn-deploy-redshift-sqs-queue-policy-negative redshift__sqs.sqs-dead-letter-queue-captures-failed-eventbridge-delivery-positive
Issue-Key: redshift__sqs.cfn-deploy-redshift-sqs-queue-policy-negative|run#13|AssertionError|assert error_code == "NO_PERMISSIONS", f"unexpected DLQ error code: {error_code!r}"
Issue-Key: redshift__sqs.sqs-dead-letter-queue-captures-failed-eventbridge-delivery-positive|run#17|AssertionError|assert error_code == "NO_PERMISSIONS", (
Issue-Commit: 30be2c6aa9a038e82cb0ad1355d2adc7b9e702f5

# redshift__sqs: EventBridge dead-letters a delivery its SQS target's resource policy denied with ERROR_CODE=NO_RESOURCE instead of NO_PERMISSIONS (1 cause)

> _Note: this issue was identified against the code base at commit `30be2c6aa9a038e82cb0ad1355d2adc7b9e702f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One net-new root cause. Both failing checks reach the dead-letter queue now (the event IS captured, with the right body and the right RULE_ARN), and they diverge only on the ERROR_CODE message attribute that classifies WHY the delivery failed.

Related open issue that does NOT carry these replay keys: 20261029T193114Z-redshift-sqs-3-net-new-causes-eventbridge-sqs.md — its cause 2 is "the dead-letter queue never receives the event at all" (those keys are Timeout keys on the DLQ poll). That behaviour has changed: the event is now dead-lettered, so what remains is the misclassification described below, which that issue does not describe. Also related but not covering: 20261024T190039Z-events-iam-sfn-sqs-eventbridge-dead-letter-error-code-misclassifies-target-delivery.md (status fixed) covered ERROR_CODE misclassification for a target ROLE — trust failure and permission-policy failure on a role-assuming target; this cause is the roleless SQS-target case, where authorization comes from the destination QUEUE's resource policy, and it is a different branch of the same classifier.

Cause 1 of 1: a rule's SQS target whose queue EXISTS but whose resource policy does not authorise the delivery is dead-lettered with ERROR_CODE='NO_RESOURCE' instead of 'NO_PERMISSIONS'.
AWS: recorded behavior only. Smithy: events PutTargets.Target.DeadLetterConfig (DeadLetterConfig.Arn -> ResourceArn, @length 1..1600, "The ARN of the SQS queue specified as the target for the dead-letter queue"); the DLQ message attributes themselves are not modelled, so the requirement is the recording's. EventBridge assumes no role for an SQS target: the delivery is authorised solely by the destination queue's resource policy. When that queue exists but has no policy, or has a policy whose aws:SourceArn condition names a different rule, EventBridge dead-letters the event onto the target's DeadLetterConfig.Arn with message attributes RULE_ARN (the rule's ARN), TARGET_ARN, ERROR_MESSAGE and ERROR_CODE='NO_PERMISSIONS'. 'NO_PERMISSIONS' is the fixed value for the authorization-failure class; 'NO_RESOURCE' is the value for the different class where the target resource itself is absent (e.g. the queue was deleted). The two are distinct classes and must not be collapsed.
Emulator: the event is dead-lettered correctly (body carries the event detail, RULE_ARN matches the rule), but the ERROR_CODE message attribute carries the string 'NO_RESOURCE'. The queue is present and was found — only its policy denied the send — so the emulator is reporting "target missing" for a target that exists.
Why: a client that routes or alarms on the EventBridge DLQ taxonomy is mis-triaged in the worst direction: NO_RESOURCE says "the target is gone, re-create or re-point it", while the actual defect is a missing/mis-scoped sqs:SendMessage grant for events.amazonaws.com. Operators chasing NO_RESOURCE delete and re-create a healthy queue and the events keep failing; every consumer branch keyed on NO_PERMISSIONS is dead code against this emulator, and alarms on it never fire.
Fix: the EventBridge target-delivery failure classifier that maps a failed SQS-target delivery onto the DLQ ERROR_CODE attribute — the queue-resource-policy denial branch must emit NO_PERMISSIONS, and NO_RESOURCE must stay reserved for a target that does not exist.
Traces: redshift__sqs.cfn-deploy-redshift-sqs-queue-policy-negative, redshift__sqs.sqs-dead-letter-queue-captures-failed-eventbridge-delivery-positive

## Evidence
Check redshift__sqs.sqs-dead-letter-queue-captures-failed-eventbridge-delivery-positive (target queue intentionally left policy-less, DLQ granted to the rule, RetryPolicy MaximumRetryAttempts=0):
  AssertionError: expected ERROR_CODE=NO_PERMISSIONS (missing SQS resource policy), got 'NO_RESOURCE'
  (assertion at check_redshift__sqs_sqs_dead_letter_queue_captures_failed_eventbridge_delivery_positive.py line 61). The same message's Body and RULE_ARN attribute both passed their assertions, so capture and rule attribution are correct and only the classification is wrong.

Check redshift__sqs.cfn-deploy-redshift-sqs-queue-policy-negative (CFN stack with two rule/queue pairs; QueueBad's policy names only the other rule's SourceArn):
  AssertionError: unexpected DLQ error code: 'NO_RESOURCE'
  (assertion at check_redshift__sqs_cfn_deploy_redshift_sqs_queue_policy_negative.py line 55, which requires error_code == "NO_PERMISSIONS"). Exactly one DLQ message arrived, as AWS produces.

Expected vs actual, per the recorded DLQ message attribute:
  expected ERROR_CODE StringValue = "NO_PERMISSIONS"  (fixed text; the whole value is a fixed enumerated token, nothing in it comes from the request)
  actual   ERROR_CODE StringValue = "NO_RESOURCE"
The sibling attributes RULE_ARN/TARGET_ARN are per-run values (the rule and queue ARNs of that run) and are not part of the requirement beyond naming the rule and target of the failed delivery.

## Steps to Reproduce
1. CreateQueue twice: one queue to be the dead-letter queue, one to be the rule's target. Leave the TARGET queue with no resource policy (or give it a policy whose aws:SourceArn condition names a different rule's ARN).
2. SetQueueAttributes on the dead-letter queue with a Policy granting events.amazonaws.com sqs:SendMessage, scoped by aws:SourceArn to the rule's ARN.
3. PutRule with an event pattern, then PutTargets with that target queue's ARN plus Target.DeadLetterConfig.Arn = the DLQ ARN and Target.RetryPolicy {MaximumRetryAttempts: 0, MaximumEventAgeInSeconds: 60}.
4. PutEvents with an entry matching the pattern.
5. ReceiveMessage on the dead-letter queue with MessageAttributeNames=['All']. The event is captured; read the ERROR_CODE message attribute.
   Expected: 'NO_PERMISSIONS'. Emulator: 'NO_RESOURCE'.
6. Confirm the target queue received nothing, then SetQueueAttributes to grant the target queue the same events.amazonaws.com sqs:SendMessage policy and PutEvents again — delivery now succeeds, proving the queue existed all along and only authorization had failed.
