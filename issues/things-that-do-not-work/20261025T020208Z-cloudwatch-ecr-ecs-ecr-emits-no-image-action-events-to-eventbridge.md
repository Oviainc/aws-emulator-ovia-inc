Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecr-lifecycle-policy-expiry-event-not-emitted-without-policy
Issue-Key: cloudwatch__ecr__ecs.ecr-lifecycle-policy-expiry-event-not-emitted-without-policy|_poll_for_event#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# ECR emits no image-action events to EventBridge: a manual BatchDeleteImage never delivers a DELETE event (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteImage

## Description
One net-new root cause.

Cause 1 of 1: ECR image actions are not published to EventBridge, so the presence/absence differential for lifecycle-policy expiry cannot be observed.
AWS: an ECR repository with no lifecycle policy fires no autonomous expiry DELETE event (and GetLifecyclePolicy on it is refused LifecyclePolicyNotFoundException), but a manual BatchDeleteImage still delivers an `ECR Image Action` event to the default EventBridge bus whose detail carries repository-name=<repo> and result=SUCCESS, which a rule can route to SQS. Recorded behavior: the manual delete's event is delivered and the autonomous one is absent.
Emulator: the check polls the rule's SQS target for the manual delete's event and is killed at the harness 60 s cap — no ECR image-action event is ever delivered, so the "no policy means no autonomous DELETE" assertion has no positive control to stand on.
Why: any consumer that reacts to image deletions/expiries through EventBridge — cleanup automation, audit trails, cache invalidation — receives nothing, and a client cannot distinguish "no lifecycle policy, so no expiry event" from "no events at all".
Fix: EventBridge event emission for ECR image actions from the ECR image-mutation handlers (BatchDeleteImage and the lifecycle-policy evaluator).
Traces: cloudwatch__ecr__ecs.ecr-lifecycle-policy-expiry-event-not-emitted-without-policy.

## Evidence
"Error: Check timed out after 60.0s". The check's own assertions state the contract it never reaches: `assert delivered is not None, "manual batch_delete_image never produced a DELETE event via EventBridge"`, then `assert delivered["detail"]["repository-name"] == repo_name` and `assert delivered["detail"]["result"] == "SUCCESS"`; the negative arm is `assert no_auto_delete is None, "a DELETE event fired even though no lifecycle policy is configured"`.

## Steps to Reproduce
1. CreateRepository (no lifecycle policy) and push two tagged images ('keep', 'victim').
2. PutRule on the default bus matching source=aws.ecr detail-type='ECR Image Action', targeting an SQS queue.
3. BatchDeleteImage for the 'victim' tag.
4. Poll the queue for the ECR Image Action event -> nothing ever arrives; the check hangs to the 60 s cap.
