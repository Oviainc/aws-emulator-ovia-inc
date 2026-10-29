Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule cloudwatch__ecr__ecs.ecr-scan-finding-without-rule-target-not-delivered
Issue-Key: cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule|_poll_for_event#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Key: cloudwatch__ecr__ecs.ecr-scan-finding-without-rule-target-not-delivered|_poll_for_event#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Commit: b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7

# cloudwatch__ecr__ecs: an ECR image push and an ECR image scan deliver no EventBridge event to a wired rule target (2 causes)

> _Note: this issue was identified against the code base at commit `b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutImage

## Description
TWO net-new root causes. The already-open 20261025T020208Z-cloudwatch-ecr-ecs-ecr-emits-no-image-action-events-to-eventbridge.md reports the same absence for a manual BatchDeleteImage DELETE event and owns that check's key only; neither replay key below is carried by it or by any other open issue, per the suite report.

Cause 1 of 2 — an image push emits no "ECR Image Action" event, so a matching EventBridge rule's SQS target receives nothing.
AWS: recorded behavior — a successful push to an ECR repository emits an EventBridge event with source "aws.ecr", detail-type "ECR Image Action" and detail carrying repository-name, action-type "PUSH", image-tag and image-digest; a rule whose pattern matches that event delivers it to its target (here an SQS queue with a matching queue policy).
Emulator: after the push, nothing is ever delivered to the target queue; the check polls to the 60 s replay cap and is killed. Hypothesis: the ECR push path publishes no event to the bus at all.
Why: every push-triggered automation (image-promotion pipelines, scanners, notifications) is silently dead; a client sees a healthy rule and a healthy queue and no events, with no error anywhere to diagnose.
Fix: ECR's push/PutImage path must publish the ECR Image Action event onto the default event bus, and the bus's rule-matching/target-delivery path must carry it to the target.
Traces: cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule.

Cause 2 of 2 — an "ECR Image Scan" event is never delivered even after a target is attached, so the targetless-rule negative cannot be distinguished from a dead event path.
AWS: recorded behavior — for a repository with imageScanningConfiguration.scanOnPush=true, a push produces an "ECR Image Scan" event (source aws.ecr); a matching rule with NO target delivers nothing, and attaching a target to that same rule makes the next matching event arrive. The check's whole point is that second leg (the positive control).
Emulator: the check reaches the 60 s replay cap. Hypothesis: no ECR Image Scan event is ever emitted (consistent with cause 1 and with scan-on-push producing no scan at all — see the ECR issue filed in this same sweep for ecr.registry-scanning-configuration-enforced-on-push), so the "wiring a target fixes delivery" leg never converges.
Why: the emulator appears to implement "a rule with no target delivers nothing" correctly while in fact delivering nothing under any configuration, so a user cannot rely on the emulator to tell a mis-wired rule from a working one.
Fix: the ECR scan path's event publication, plus EventBridge rule-target delivery for source aws.ecr / detail-type "ECR Image Scan".
Traces: cloudwatch__ecr__ecs.ecr-scan-finding-without-rule-target-not-delivered.

## Evidence
[mismatch] cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule — "Check timed out after 60.0s". The check creates a repository, an SQS queue + rule with pattern {source:["aws.ecr"], detail-type:["ECR Image Action"], detail:{repository-name:[<repo>], action-type:["PUSH"]}}, pushes a minimal image, then polls the queue for the matching event; none arrived.
[mismatch] cloudwatch__ecr__ecs.ecr-scan-finding-without-rule-target-not-delivered — "Check timed out after 60.0s". The check creates a repository with imageScanningConfiguration.scanOnPush=true, a queue, and a rule with pattern {source:["aws.ecr"], detail-type:["ECR Image Scan"], detail:{repository-name:[<repo>]}} first WITHOUT a target and then WITH one; no event was ever delivered on the positive leg.

## Steps to Reproduce
Cause 1: ecr CreateRepository(<repo>); sqs CreateQueue and allow events.amazonaws.com to SendMessage; events PutRule with pattern {"source":["aws.ecr"],"detail-type":["ECR Image Action"],"detail":{"repository-name":["<repo>"],"action-type":["PUSH"]}} and PutTargets with the queue ARN; push an image to <repo>; ReceiveMessage on the queue — nothing arrives.
Cause 2: same wiring with detail-type "ECR Image Scan" against a repository created with imageScanningConfiguration={scanOnPush:true}; first with no target (expect nothing), then PutTargets with the queue and push again — nothing arrives on the second leg either.
