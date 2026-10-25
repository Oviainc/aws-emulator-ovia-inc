Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.queue-arn-carries-the-creation-region sqs.same-name-queues-in-two-regions-hold-separate-messages sqs.queue-policy-naming-the-same-queue-name-in-another-region-grants-nothing sqs.queue-url-from-another-region-not-served-rejected sqs.dead-letter-wiring-to-another-region-rejected sqs.message-move-task-across-regions-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.queue-arn-carries-the-creation-region|run#10|AssertionError|assert parts[3] == expected_region, (
Issue-Key: sqs.same-name-queues-in-two-regions-hold-separate-messages|run#18|AssertionError|assert [(m["MessageId"], m["Body"]) for m in alt_got] == [(alt_id, alt_body)], (
Issue-Key: sqs.queue-policy-naming-the-same-queue-name-in-another-region-grants-nothing|run#12|AssertionError|assert home_arn != far_arn and home_arn.rsplit(":", 1)[-1] == far_arn.rsplit(":", 1)[-1], (
Issue-Key: sqs.queue-url-from-another-region-not-served-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.dead-letter-wiring-to-another-region-rejected|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.message-move-task-across-regions-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS has no per-region identity: QueueArn hardcodes one region, a queue URL from another region is served, and cross-region DLQ / move-task wiring is accepted (5 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueAttributes

## Description
Every SQS queue in the emulator lives in one global namespace with one hardcoded region, so region is neither reported, nor isolated, nor validated.

Cause 1 of 5: QueueArn does not name the region the queue was created in. AWS: GetQueueAttributes(['QueueArn']) returns arn:aws:sqs:<the region the CreateQueue call was made in>:<account>:<name>. Emulator: a queue created against us-east-1 reports 'arn:aws:sqs:us-west-2:000000000000:<name>' — one region is reported for every queue regardless of the endpoint addressed. Why: ARNs are the join key for DLQ wiring, EventBridge targets, SNS subscriptions and IAM Resource matching; a wrong region in the ARN silently binds the wrong resource. Fix: the ARN construction in the SQS crate's queue state/serialization, which must carry the request's region. Traces: sqs.queue-arn-carries-the-creation-region.

Cause 2 of 5: queues are not isolated per region. AWS: the same queue name created in two regions is two independent queues, each delivering only its own messages, and a queue policy naming the same name in another region grants nothing. Emulator: one queue backs both endpoints — the us-east-1 queue delivered all three messages, including the one sent to the alternate region. Why: cross-region tests and multi-region fan-out scenarios silently share state; a message is read by the wrong consumer. Fix: key the queue registry in the SQS crate's state by (region, account, name) rather than by name. Traces: sqs.same-name-queues-in-two-regions-hold-separate-messages, sqs.queue-policy-naming-the-same-queue-name-in-another-region-grants-nothing.

Cause 3 of 5: a queue URL belonging to another region is served. AWS: addressing a us-east-1 queue URL against the us-west-2 endpoint is refused (the queue does not exist there). Emulator: SendMessage against the foreign-region URL returns 200 with a MessageId. Why: a client misconfigured with the wrong region gets silent success instead of the error that would reveal it. Fix: region checking in the queue-URL resolver shared by the SQS handlers. Traces: sqs.queue-url-from-another-region-not-served-rejected.

Cause 4 of 5: cross-region RedrivePolicy is accepted. AWS: SetQueueAttributes with a deadLetterTargetArn in a different region is refused at 400 and the attribute is unchanged. Emulator: the call returns 200 and the wiring is stored. Why: a DLQ that can never receive is accepted, so redrive silently black-holes. Fix: the deadLetterTargetArn validation in the SetQueueAttributes/CreateQueue handler. Traces: sqs.dead-letter-wiring-to-another-region-rejected.

Cause 5 of 5: cross-region StartMessageMoveTask is accepted. AWS: a SourceArn/DestinationArn pair spanning two regions is refused. Emulator: it returns 200 with a TaskHandle. Why: the client believes a move is underway that AWS would never have started. Fix: the ARN region check in the StartMessageMoveTask handler. Traces: sqs.message-move-task-across-regions-rejected.

## Evidence
Expected (AWS): QueueArn carries the creating region; each region has its own queue namespace; a foreign-region queue URL is not served; cross-region DLQ wiring and cross-region move tasks are refused at 400.
Actual (emulator replay): 'AssertionError: [alt] QueueArn must name the region the queue was created in: expected us-east-1, got us-west-2 (full ARN arn:aws:sqs:us-west-2:000000000000:sim2real-behav-region-q-...)'; 'the us-east-1 queue must deliver exactly its own message; it delivered [home-0-..., home-1-..., alt-...]'; 'expected the call to be refused, but it succeeded: {MessageId: ..., HTTPStatusCode: 200}' (foreign-region URL); 'expected the call to be refused, but it succeeded: {HTTPStatusCode: 200}' (cross-region RedrivePolicy); 'expected the call to be refused, but it succeeded: {TaskHandle: ..., HTTPStatusCode: 200}' (cross-region move task).

## Steps to Reproduce
1. CreateQueue 'q' against the us-east-1 endpoint and GetQueueAttributes(['QueueArn']) -> the ARN names us-west-2.
2. CreateQueue the same name against us-west-2, SendMessage one distinct body to each, then ReceiveMessage from the us-east-1 queue -> it returns the other region's messages too.
3. SendMessage using the us-east-1 queue URL against the us-west-2 endpoint -> 200.
4. SetQueueAttributes RedrivePolicy with a deadLetterTargetArn in another region -> 200.
5. StartMessageMoveTask with SourceArn and DestinationArn in different regions -> 200 with a TaskHandle.
6. Re-replay the check ids listed in trace_ids.
