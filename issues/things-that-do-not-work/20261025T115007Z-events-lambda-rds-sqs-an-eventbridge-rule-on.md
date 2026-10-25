Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event
Issue-Key: events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event|_trigger_until#23|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Commit: 1f7e5adccf9e411825240a72188af4d50262df19

# events__lambda__rds__sqs: an EventBridge rule on the aws.rds CloudTrail event never fires, so a Lambda target lacking lambda:InvokeFunction is never dead-lettered with NO_PERMISSIONS

> _Note: this issue was identified against the code base at commit `1f7e5adccf9e411825240a72188af4d50262df19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: no aws.rds CloudTrail-sourced event is ever routed to a matching EventBridge rule, so the rule's resource-policy enforcement at delivery time and its dead-lettering of a denied Lambda target are both unobservable; the check never completes.

Context on what is already filed: the open issue 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md covers CloudTrail-sourced events not reaching rules for other services' checks, and 20261024T190039Z-events-iam-sfn-sqs-eventbridge-dead-letter-error-code-misclassifies-target-delivery.md covers dead-letter error-code classification for sfn targets. Neither names this check and the replay report marks it neither OWNED nor ALREADY FILED, so it is filed here; a fixer should expect overlap with the CloudTrail-ingest half of the first issue.

AWS: recorded behavior only (measured on real AWS, us-west-2, 2026-10-01, and recorded in the check). With a single-region CloudTrail trail in the account, a ModifyDBParameterGroup call produces an `AWS API Call via CloudTrail` event with source `aws.rds` that reaches a matching EventBridge rule in roughly 5 s, and the event's detail.requestID is the RequestId RDS returned for the call. When that rule's Lambda target has no resource policy granting lambda:InvokeFunction to events.amazonaws.com, EventBridge does NOT invoke the function; it dead-letters the event to the target's SQS DLQ with message attribute ERROR_CODE = `NO_PERMISSIONS`, an ERROR_MESSAGE quoting Lambda's AccessDeniedException for lambda:InvokeFunction, and RULE_ARN / TARGET_ARN naming the failed delivery. The message BODY is the original event envelope (same `id`), EventBridge adds no event-id attribute, and there are NO RETRY_ATTEMPTS / EXHAUSTED_RETRY_CONDITION attributes even though the target declares RetryPolicy.MaximumRetryAttempts=2 — a permission denial is not retried. Adding the lambda:InvokeFunction permission on the SAME rule and target makes the next ModifyDBParameterGroup invoke the function.
Emulator: `Check timed out after 60.0s` — neither leg is reached. No event arrives at the rule, so neither the DLQ message nor (on the positive leg) the function invocation is ever observed.
Why: this is the over-permissive-vs-silent failure boundary for every EventBridge->Lambda wiring. A client cannot tell a denied delivery from a never-attempted one, cannot verify that EventBridge enforces the Lambda resource policy at delivery time rather than invoking regardless, and gets no dead letter to alarm on — so an outage in which every event is being dropped looks identical to a healthy, idle rule.
Fix: the CloudTrail-sourced event emission for RDS management calls (the aws.rds `AWS API Call via CloudTrail` event feeding the bus) and the eventbridge target dispatcher's Lambda branch — its lambda resource-policy check at delivery time and the dead-letter path that stamps ERROR_CODE/ERROR_MESSAGE/RULE_ARN/TARGET_ARN and does NOT retry a NO_PERMISSIONS failure.
Traces: events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event.

## Evidence
events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event [mismatch]: `Error: Check timed out after 60.0s`.

Expected (recorded on real AWS, us-west-2, 2026-10-01): within ~5 s of ModifyDBParameterGroup, with a single-region trail present, an `AWS API Call via CloudTrail` / source `aws.rds` event reaches the matching rule; the Lambda target with no events.amazonaws.com invoke permission is dead-lettered to its SQS DLQ with ERROR_CODE='NO_PERMISSIONS', an ERROR_MESSAGE quoting Lambda's AccessDeniedException for lambda:InvokeFunction, RULE_ARN and TARGET_ARN of the failed delivery, the original event envelope as the body (detail.requestID == the RequestId RDS returned), no event-id attribute, and NO RETRY_ATTEMPTS / EXHAUSTED_RETRY_CONDITION attributes despite RetryPolicy={'MaximumRetryAttempts': 2, 'MaximumEventAgeInSeconds': 300}. After granting lambda:InvokeFunction on the same rule/target, the next ModifyDBParameterGroup invokes the function.
Actual: nothing arrives — no DLQ message on the negative leg and no invocation on the positive leg; the check does not complete.

What the timeout does and does not establish: the harness caps each check at 60 s and the check's own delivery budget is 240 s with a 20 s re-trigger, so the cap does not localise the stall to one call. It does establish that no aws.rds CloudTrail-sourced event and no dead letter is observable on this emulator inside the cap, against a real-AWS latency of ~5 s for the same wiring. The over-permissive failure mode (invoking the function with no permission) is also not observed, so the gap is upstream: the event never reaches the rule.

## Steps to Reproduce
1. Create a single-region CloudTrail trail in the account (a delivery prerequisite: with no trail, AWS emits no aws.rds event of any detail-type).
2. Create an RDS DB parameter group P (unattached), a Lambda function F with a sink, and an SQS queue D as the dead-letter queue.
3. events.create_event_bus / use the default bus; put_rule with a pattern matching {'source': ['aws.rds'], 'detail-type': ['AWS API Call via CloudTrail'], 'detail': {'eventName': ['ModifyDBParameterGroup']}}; put_targets with F as the target, DeadLetterConfig={'Arn': <D's arn>} and RetryPolicy={'MaximumRetryAttempts': 2, 'MaximumEventAgeInSeconds': 300}, and give F NO lambda:add_permission for events.amazonaws.com.
4. rds.modify_db_parameter_group(P, ...) and poll D. Expected: a dead letter with ERROR_CODE='NO_PERMISSIONS', ERROR_MESSAGE, RULE_ARN, TARGET_ARN, body == the original event, and no RETRY_ATTEMPTS attribute; F's sink stays empty. Observed: D stays empty and the check times out.
5. lambda.add_permission granting lambda:InvokeFunction to events.amazonaws.com for the rule ARN, then modify_db_parameter_group again. Expected: F is invoked and its sink records the event. Observed: never reached.

Replay events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event to confirm.
