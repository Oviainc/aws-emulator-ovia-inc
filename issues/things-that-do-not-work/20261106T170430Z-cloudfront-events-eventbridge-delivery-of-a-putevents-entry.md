Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq
Issue-Key: cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq|_wait_for_log_message#45|Timeout|time.sleep(3)
Issue-Commit: 0e907dd5f11711da470907c28603d7f862f446bd

# cloudfront__events: EventBridge delivery of a PutEvents entry never completes — a permitted Lambda target is not invoked and a permission-denied delivery never reaches the target's DeadLetterConfig queue

> _Note: this issue was identified against the code base at commit `0e907dd5f11711da470907c28603d7f862f446bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause.

Cause 1 of 1: an EventBridge rule's target-delivery path produces no observable effect, in either the success or the failure direction, so the check never reaches a verdict inside the 60 s cap.
Failing check ids: cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq

AWS: PutEvents on the default bus delivers each entry to every ENABLED rule whose EventPattern matches it. For a matched rule with a Lambda target, EventBridge invokes that function ASYNCHRONOUSLY when the resource policy on the function allows events.amazonaws.com to call lambda:InvokeFunction for that rule ARN, and delivery is complete at the point the invocation is accepted — a handler that raises afterwards is therefore NOT a delivery failure and must NEVER appear in the rule's dead-letter queue. When the invocation attempt itself is refused because no such grant exists, that IS a delivery failure, and EventBridge sends the event to the queue named by the target's DeadLetterConfig. Smithy (services/service-com-amazonaws-eventbridge-awsevents/smithy-model): Target$DeadLetterConfig -> DeadLetterConfig$Arn ("The ARN of the SQS queue specified as the target for the dead-letter queue"), Target$RetryPolicy -> RetryPolicy{MaximumRetryAttempts range 0..185, MaximumEventAgeInSeconds range 60..86400}; the model carries the configuration but states nothing about the delivered record, so the DLQ record's shape is recorded behavior only. As recorded, the dead-lettered SQS message carries the original event as its body plus EventBridge's own SQS message attributes, whose names are fixed text and whose values come from the request and from state: RULE_ARN = the ARN of the rule that failed to deliver, TARGET_ARN = the ARN of the target it could not invoke, and ERROR_CODE = the fixed string "NO_PERMISSIONS" for an invocation refused for want of an invoke grant.

Emulator: nothing on this path is observable. The check is killed by the harness with "Check timed out after 60.0s" while waiting, in order, on (1) the positive control — the SQS queue that the PERMITTED Lambda target's handler writes to, which proves the matched rule invoked its target at all; (2) a CloudWatch Logs line from the deliberately-raising function, which proves that function was invoked and did raise; and (3) the twin control — the DLQ receiving the permission-denied delivery with the three attributes above. No verdict is ever produced, so none of the three is satisfied within the cap: a matched PutEvents entry yields no target invocation, and no refused delivery is dead-lettered. The suite's own control structure makes the absence meaningful: the check deliberately measures the DLQ latency from the permission-denied delivery before asserting that the raising handler's event is ABSENT from the DLQ, so an emulator that dead-letters a handler exception (rather than only a delivery failure) would also fail here, in the opposite direction.

Why: a client that wires CloudFront CreateInvalidation automation through EventBridge gets neither of the two things it depends on. The happy path silently does nothing — the function is never invoked, so no invalidation is ever created from an event — and the failure path is invisible: an operator who attaches a DeadLetterConfig to catch mis-permissioned targets sees an empty queue and cannot tell a correctly-delivered event from one EventBridge refused to deliver, and never sees ERROR_CODE=NO_PERMISSIONS pointing at the missing lambda:InvokeFunction grant. The same silence also erases the distinction AWS draws between an EventBridge delivery failure (dead-lettered) and a target's own exception (not dead-lettered), so retry/alarm logic built on the DLQ is unverifiable.

Fix: the EventBridge target-delivery component — the rule-matching and target-dispatch path behind PutEvents (invoke the Lambda target asynchronously when the function's resource policy grants events.amazonaws.com invoke for the rule ARN), plus the delivery-failure path that consults Target$DeadLetterConfig and Target$RetryPolicy and writes the dead-lettered record, with the RULE_ARN / TARGET_ARN / ERROR_CODE message attributes, into the configured SQS queue. The authorization decision (refuse the invocation when no matching lambda:add-permission statement exists) belongs with the dispatch, since it is what selects the DLQ path.

Traces: cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq (the suite reports it as a 60.0 s timeout, not an assertion, because the first wait never returns).

Not covered by an existing issue: the replay report marks this check neither ALREADY FILED nor OWNED. Open issues that look similar but are grounded in other services' checks and do not carry this check's replay key: 20261022T194351Z-events-firehose-s3-... (EventBridge failed target invocations never reach the DLQ), 20261105T031517Z-cloudwatch-events-sqs-a-rule-whose-lambda-target.md and 20261029T182228Z-events-sqs-eventbridge-puttargets-... (DLQ ERROR_CODE fidelity). Fixing those may well fix this one; this issue exists so the cloudfront__events check above is actually re-replayed.

## Evidence
Expected (real AWS, as recorded):
  1. PutEvents(Source="sim2real.cloudfront", DetailType="AWS API Call via CloudTrail", Detail={"eventName":"CreateInvalidation","invalidationMarker":<marker_ok>}) -> the matching ENABLED rule invokes its permitted Lambda target, whose handler forwards the event; the success SQS queue receives a message whose Body has detail.invalidationMarker == <marker_ok>.
  2. The entry carrying <marker_fail> invokes the raising function: its CloudWatch Logs stream contains "simulated CreateInvalidation failure", and NOTHING for that marker ever appears in the DLQ (a handler exception is not a delivery failure).
  3. The entry carrying <marker_permfail> targets a function with NO events.amazonaws.com invoke grant for that rule ARN: the DLQ receives that event with message attributes RULE_ARN = <rule_permfail_arn>, TARGET_ARN = <ok_fn_arn>, ERROR_CODE = "NO_PERMISSIONS" ("NO_PERMISSIONS" is fixed text; both ARNs are values taken from the request and from state).

Actual (emulator): the check is terminated by the harness — "Error: Check timed out after 60.0s" — with no assertion reached. The first blocking wait, _poll_for_message(success_url, timeout_s=60) for the positive control, never returns, so the permitted target was never invoked; neither the raising function's log line nor any dead-lettered record with RULE_ARN/TARGET_ARN/ERROR_CODE is observed either. No status code or error code is returned anywhere on this path: every control-plane call (PutRule, PutTargets with DeadLetterConfig/RetryPolicy, Lambda AddPermission, SQS policy grant, PutEvents) is accepted, and only the effect is missing.

## Steps to Reproduce
1. Create two SQS queues: a DLQ and a success queue; read both QueueArns via GetQueueAttributes.
2. Create three Lambda functions/roles as the check does: one handler that raises, one handler that forwards its event to the success queue (its role granted sqs:SendMessage on the success queue ARN).
3. events.PutRule three ENABLED rules with EventPattern {"source":["sim2real.cloudfront"],"detail":{"invalidationMarker":[<marker>]}} for markers marker_ok, marker_fail, marker_permfail.
4. lambda.AddPermission for events.amazonaws.com with SourceArn = the rule ARN for the marker_ok and marker_fail rules ONLY; grant nothing for the marker_permfail rule.
5. Grant EventBridge sqs:SendMessage on the DLQ for all three rule ARNs.
6. events.PutTargets on each rule with a single target carrying DeadLetterConfig={"Arn": <dlq_arn>} and RetryPolicy={"MaximumRetryAttempts":0,"MaximumEventAgeInSeconds":60}; the marker_permfail rule targets the forwarding function (which it may not invoke).
7. events.PutEvents with one entry per marker.
8. Observe: ReceiveMessage on the success queue never yields the marker_ok event; the raising function's log group never shows "simulated CreateInvalidation failure"; ReceiveMessage on the DLQ never yields the marker_permfail event with MessageAttributes RULE_ARN/TARGET_ARN/ERROR_CODE="NO_PERMISSIONS". The check hits the 60 s cap.
Replay: cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq
