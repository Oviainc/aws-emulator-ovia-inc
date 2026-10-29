Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.cfn-deploy-eventbridge-rule-cloudfront-invalidation-lambda cloudfront__events.cloudfront-invalidation-created-by-lambda-reflects-paths cloudfront__events.eventbridge-cloudfront-custom-event-denied-without-putevents-permission cloudfront__events.eventbridge-cloudtrail-cloudfront-deletedistribution-payload-format cloudfront__events.eventbridge-invalidation-rule-pattern-mismatch-no-invalidation-created cloudfront__events.eventbridge-lambda-cloudfront-invalidation-blocked-without-target-permission cloudfront__events.eventbridge-put-events-custom-detail-triggers-cloudfront-cache-invalidation cloudfront__events.eventbridge-rule-missing-permission-blocks-cloudfront-invalidation-lambda cloudfront__events.iam-role-trust-scoped-with-external-condition-enforced cloudfront__events.xsvc-cloudfront-1-create-invalidation-cloudtrail-event-routes-to-sqs cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target cloudfront__events.cloudfront-event-sns-topic-policy-missing-blocks-delivery
Issue-Key: cloudfront__events.cfn-deploy-eventbridge-rule-cloudfront-invalidation-lambda|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__events.cloudfront-invalidation-created-by-lambda-reflects-paths|_find_matching_invalidation#18|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__events.eventbridge-cloudfront-custom-event-denied-without-putevents-permission|_poll_until_denied#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__events.eventbridge-cloudtrail-cloudfront-deletedistribution-payload-format|_wait_for_queue_message#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: cloudfront__events.eventbridge-invalidation-rule-pattern-mismatch-no-invalidation-created|_poll_for_twin#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__events.eventbridge-lambda-cloudfront-invalidation-blocked-without-target-permission|_find_matching_invalidation#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__events.eventbridge-put-events-custom-detail-triggers-cloudfront-cache-invalidation|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__events.eventbridge-rule-missing-permission-blocks-cloudfront-invalidation-lambda|_peek_labeled_message#21|Timeout|resp = sqs.receive_message(
Issue-Key: cloudfront__events.iam-role-trust-scoped-with-external-condition-enforced|_wait_for_dlq_message#35|Timeout|resp = sqs.receive_message(
Issue-Key: cloudfront__events.xsvc-cloudfront-1-create-invalidation-cloudtrail-event-routes-to-sqs|_drain#10|Timeout|msgs = sqs.receive_message(
Issue-Key: cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target|_deploy_cfn#3|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: cloudfront__events.cloudfront-event-sns-topic-policy-missing-blocks-delivery|run#17|AssertionError|assert not any(marker in m["Body"] for m in blocked_messages), (
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudfront__events: 13 more CloudFront/EventBridge checks fail — 10 never see the rule's target effect, 2 CFN stacks roll back on an events-trusted IAM role, and an SNS topic with no events.amazonaws.com statement is delivered to anyway

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
This issue bundles 3 net-new root causes in the cloudfront__events behavioral group. Open issues 20261023T140640Z-cloudfront-events-eventbridge-never-delivers-a-matched-event-to-any.md, 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md and 20261025T002357Z-cloudfront-events-cloudfront-read-only-calls-emit-no-events-and.md own neighbouring checks; the 13 replay keys below are not on any of them.

Cause 1 of 3: a matched EventBridge rule never produces its target effect, so every check that waits for one burns its whole budget.
AWS: PutEvents (and the CloudTrail/management event a CloudFront write produces) returns FailedEntryCount=0 and the bus then evaluates every enabled rule; a matching rule invokes its targets — the Lambda runs (and its CreateInvalidation appears in CloudFront), the SQS queue receives the message, the SNS topic notifies subscribers — within seconds. A rule whose pattern does NOT match, whose state is DISABLED, or whose target lacks a resource-based grant produces NO target effect, and the absence is observable within the same window. Smithy: PutEvents -> PutEventsResponse{FailedEntryCount, Entries}; the delivery semantics are not modelled, so: recorded behavior only.
Emulator: PutEvents is accepted but no target is ever invoked and no invalidation, SQS message or SNS notification appears. All 10 checks below — the positive ones AND the negative ones that need a positive control first — run to `Check timed out after 60.0s`.
Why: EventBridge is unusable as a trigger: an event-driven CloudFront invalidation pipeline never fires, and because the negative controls also hang a client cannot even confirm that a deliberately-blocked delivery was blocked for the right reason.
Fix: the EventBridge rule-matching and target-dispatch path (the bus's delivery loop) in the eventbridge service crate, including the resource-policy/role check it must apply before invoking.
Traces: cloudfront__events.cfn-deploy-eventbridge-rule-cloudfront-invalidation-lambda, .cloudfront-invalidation-created-by-lambda-reflects-paths, .eventbridge-cloudfront-custom-event-denied-without-putevents-permission, .eventbridge-cloudtrail-cloudfront-deletedistribution-payload-format, .eventbridge-invalidation-rule-pattern-mismatch-no-invalidation-created, .eventbridge-lambda-cloudfront-invalidation-blocked-without-target-permission, .eventbridge-put-events-custom-detail-triggers-cloudfront-cache-invalidation, .eventbridge-rule-missing-permission-blocks-cloudfront-invalidation-lambda, .iam-role-trust-scoped-with-external-condition-enforced, .xsvc-cloudfront-1-create-invalidation-cloudtrail-event-routes-to-sqs.

Cause 2 of 3: a CloudFormation stack declaring an events-trusted IAM role and an EventBridge rule targeting CloudFront rolls back instead of creating.
AWS: a template declaring AWS::IAM::Role (AssumeRolePolicyDocument trusting events.amazonaws.com), AWS::Events::Rule and its target reaches CREATE_COMPLETE; the role and rule exist afterwards. The ROLLBACK path is for a resource the service genuinely refuses.
Emulator: the stack ends ROLLBACK_COMPLETE. Nothing the template declared survives, so the checks cannot even set up the mistrust/trust comparison they exist to make.
Why: infrastructure-as-code users cannot stand up an EventBridge-to-CloudFront pipeline at all, and the rollback gives no attributable reason for which resource failed.
Fix: the CloudFormation resource providers for AWS::IAM::Role and AWS::Events::Rule (and the rule-target property) — the create must succeed for a valid template, and any refusal must name the failing resource.
Traces: cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target, cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target.

Cause 3 of 3: SNS delivers an EventBridge notification to a topic whose access policy grants events.amazonaws.com nothing.
AWS: an EventBridge rule targeting an SNS topic can publish only if the topic's resource policy has a statement allowing sns:Publish to Principal Service events.amazonaws.com on that topic ARN. With no such statement the publish is denied and NO notification reaches the topic's subscribers.
Emulator: the notification is delivered anyway — the check observes the subscriber receiving it from a topic with no events.amazonaws.com statement.
Why: the resource-policy boundary around SNS is not enforced, so a client gets no signal that its topic policy is missing and a policy mistake that AWS would block passes silently in the emulator — the opposite of fail-closed.
Fix: the SNS publish authorization check (topic resource-policy evaluation) on the EventBridge target-delivery path.
Traces: cloudfront__events.cloudfront-event-sns-topic-policy-missing-blocks-delivery.

## Evidence
Cause 1: `Error: Check timed out after 60.0s` on all 10 listed checks. In the same runs PutEvents itself answered 200 — e.g. the sibling owned check records `{'FailedEntryCount': 0, 'Entries': [{'EventId': '...'}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` — so the event was accepted and simply never delivered.
Expected: the target effect (a CloudFront invalidation with the event's paths, an SQS message, an SNS notification) within the poll window; for the negative checks, the positive control first and then a confirmed absence.
Actual: no target effect ever; the harness cap is reached.

Cause 2, verbatim:
  cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target — AssertionError: Stack sim2real-xsvc-stk-5c7c914060d6 ended in ROLLBACK_COMPLETE
  cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target — AssertionError: Stack sim2real-xsvc-stk-208099e9a349 ended in ROLLBACK_COMPLETE
Expected: CREATE_COMPLETE (the stack names are per-run values; the terminal status is the requirement).

Cause 3, verbatim:
  cloudfront__events.cloudfront-event-sns-topic-policy-missing-blocks-delivery — AssertionError: topic without an events.amazonaws.com policy statement received the notification anyway
Expected: no notification delivered; the publish denied by the topic policy.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the cloudfront__events behavioral checks listed in trace_ids.
2. Cause 1: create an EventBridge rule with a pattern matching a custom detail-type, add a Lambda (or SQS) target with the required grant, PutEvents a matching event, then poll for the target effect (the Lambda's CreateInvalidation / the SQS message). Observe that none arrives.
3. Cause 2: CreateStack with a template declaring an AWS::IAM::Role trusting events.amazonaws.com plus an AWS::Events::Rule with a target, and wait for the terminal status. Observe ROLLBACK_COMPLETE.
4. Cause 3: create an SNS topic, leave its access policy without any statement allowing sns:Publish to Service events.amazonaws.com, make it the target of a matching EventBridge rule, subscribe an SQS queue, then PutEvents a matching event. Observe the subscriber receives the notification.
