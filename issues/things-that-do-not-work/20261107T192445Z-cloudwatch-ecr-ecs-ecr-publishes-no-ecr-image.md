Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule cloudwatch__ecr__ecs.ecr-lifecycle-policy-expiry-event-not-emitted-without-policy
Issue-Key: cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule|run#10|AssertionError|assert delivered is not None, "no 'ECR Image Action' PUSH event delivered via EventBridge"
Issue-Key: cloudwatch__ecr__ecs.ecr-lifecycle-policy-expiry-event-not-emitted-without-policy|run#21|AssertionError|assert delivered is not None, "manual batch_delete_image never produced a DELETE event via EventBridge"
Issue-Commit: ab92470025dd8eeb501814706ada4aa2b617e26c

# cloudwatch__ecr__ecs: ECR publishes no 'ECR Image Action' event to EventBridge — neither a push nor a manual BatchDeleteImage reaches a wired rule target (2 causes)

> _Note: this issue was identified against the code base at commit `ab92470025dd8eeb501814706ada4aa2b617e26c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutImage

## Description
TWO net-new root causes, both in the ECR→EventBridge publication path. Related open issue that does NOT carry these replay keys (the suite report lists neither check as OWNED, and the earlier issue recorded these checks failing as harness TIMEOUTS, whereas they now run to completion and assert on an empty queue): 20261029T174654Z-cloudwatch-ecr-ecs-an-ecr-image-push-and.md.

Cause 1 of 2 — a successful image push publishes no 'ECR Image Action' event, so a matching EventBridge rule's SQS target receives nothing.
AWS: recorded behavior only (the Smithy model does not describe eventing). A successful push to an ECR repository publishes an event onto the account's default event bus with source 'aws.ecr', detail-type 'ECR Image Action' and a detail object carrying repository-name (the repository the push targeted), action-type 'PUSH', result 'SUCCESS', image-tag and image-digest; a rule whose event pattern matches that event delivers the envelope to its target (here an SQS queue whose queue policy allows events.amazonaws.com:SendMessage).
Emulator: after the push the queue stays empty for the check's full 90 s poll; the check then fails its own assertion "no 'ECR Image Action' PUSH event delivered via EventBridge". Nothing is refused anywhere — CreateRepository, the push, PutRule and PutTargets all answer 200.
Why: every push-triggered automation (image promotion, notification, scan kick-off, downstream deployments) is silently dead. A client sees a healthy rule, a healthy target and no event, with no error to diagnose, so it cannot distinguish a mis-wired rule from an event path that never fires.
Fix: ECR's push/PutImage handler must publish the 'ECR Image Action' (action-type PUSH) envelope onto the default event bus, and the bus's rule-match/target-delivery path must carry it to the target.
Traces: cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule.

Cause 2 of 2 — a manual BatchDeleteImage publishes no 'ECR Image Action' DELETE event, so the negative-delivery check's positive control never fires.
AWS: recorded behavior only. Deleting an image with BatchDeleteImage publishes source 'aws.ecr', detail-type 'ECR Image Action' with detail.action-type 'DELETE' and detail.repository-name naming the repository; this happens whether or not a lifecycle policy exists (the check first confirms GetLifecyclePolicy is refused LifecyclePolicyNotFoundException for the repository, i.e. no autonomous expiry can be the source of the event).
Emulator: the manual batch_delete_image produces no event at all — the check fails with "manual batch_delete_image never produced a DELETE event via EventBridge".
Why: the emulator appears to implement "no lifecycle policy ⇒ no DELETE event" correctly while in fact never delivering a DELETE event under any configuration, so a client cannot use the emulator to verify expiry/deletion automation, and a mis-configured lifecycle policy is indistinguishable from a correct one.
Fix: the BatchDeleteImage handler's event publication (action-type DELETE) plus the same bus rule-match/target-delivery path as cause 1.
Traces: cloudwatch__ecr__ecs.ecr-lifecycle-policy-expiry-event-not-emitted-without-policy.

## Evidence
[mismatch] cloudwatch__ecr__ecs.ecr-push-event-triggers-eventbridge-rule — AssertionError: "no 'ECR Image Action' PUSH event delivered via EventBridge" raised after a 90 s ReceiveMessage poll. Wiring: SQS queue + PutRule pattern {"source":["aws.ecr"],"detail-type":["ECR Image Action"],"detail":{"repository-name":[<repo>],"action-type":["PUSH"]}} + PutTargets(queue ARN); the image push itself succeeded. Expected one envelope with source 'aws.ecr' and detail.result 'SUCCESS'.
[mismatch] cloudwatch__ecr__ecs.ecr-lifecycle-policy-expiry-event-not-emitted-without-policy — AssertionError: "manual batch_delete_image never produced a DELETE event via EventBridge". Same wiring with detail.action-type ["DELETE"]; the repository was first confirmed to have no lifecycle policy (GetLifecyclePolicy refused LifecyclePolicyNotFoundException with message "Lifecycle policy does not exist for the repository with name '<repo>' in the registry with id '<account>'" — fixed text with the repository name and registry id taken from the request/state), two images were pushed and one was deleted with BatchDeleteImage.

## Steps to Reproduce
Cause 1: ecr CreateRepository(<repo>); sqs CreateQueue and set a queue policy allowing events.amazonaws.com to SendMessage; events PutRule with pattern {"source":["aws.ecr"],"detail-type":["ECR Image Action"],"detail":{"repository-name":["<repo>"],"action-type":["PUSH"]}} and PutTargets with the queue ARN; push a minimal image (InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload/PutImage); ReceiveMessage on the queue — nothing arrives.
Cause 2: same wiring with action-type ["DELETE"]; confirm GetLifecyclePolicy on the repository is refused LifecyclePolicyNotFoundException, push two tagged images, then BatchDeleteImage one of them; ReceiveMessage on the queue — nothing arrives.
