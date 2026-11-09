Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.message-move-task-redrive-assigns-new-message-id-and-sent-timestamp sqs.redriven-message-restarts-its-receive-count-and-dead-letters-again-at-maxreceivecount sqs.fifo-dead-letter-queue-move-sets-dedup-id-to-original-message-id sqs.start-message-move-task-on-directly-sent-message-fails-could-not-determine-source sqs.fifo-batch-with-a-group-less-entry-rejected-whole-and-group-stream-unchanged sqs.message-attribute-type-length-bounds-rejected sqs.identity-policy-requesttag-condition-gates-create-queue sqs.identity-policy-resourcetag-condition-gates-send-message sqs.set-queue-attributes-policy-statement-limits-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.message-move-task-redrive-assigns-new-message-id-and-sent-timestamp|run#28|AssertionError|assert moved_msg["MessageId"] != dlq_message_id, (
Issue-Key: sqs.redriven-message-restarts-its-receive-count-and-dead-letters-again-at-maxreceivecount|_deliver_up_to_the_count#21|AssertionError|assert seen == list(range(1, len(seen) + 1)), (
Issue-Key: sqs.fifo-dead-letter-queue-move-sets-dedup-id-to-original-message-id|run#17|AssertionError|assert dlq_dedup_id == sent_id, (
Issue-Key: sqs.start-message-move-task-on-directly-sent-message-fails-could-not-determine-source|run#18|AssertionError|assert failed.get("Status") == "FAILED", (
Issue-Key: sqs.fifo-batch-with-a-group-less-entry-rejected-whole-and-group-stream-unchanged|run#10|AssertionError|assert got == ("InvalidParameterValue", MISSING_GROUP_MSG), (
Issue-Key: sqs.message-attribute-type-length-bounds-rejected|run#8|AssertionError|assert message == "Message (user) attribute type must be shorter than 256 bytes.", (
Issue-Key: sqs.identity-policy-requesttag-condition-gates-create-queue|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.identity-policy-resourcetag-condition-gates-send-message|_until_allowed#14|TimeoutError|raise TimeoutError(f"call never became allowed within {timeout_s}s (last={last!r})")
Issue-Key: sqs.set-queue-attributes-policy-statement-limits-rejected|_check_size_limit#7|AssertionError|assert code == "OverLimit", f"oversized Policy: expected OverLimit, got {code!r}"
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 7 net-new causes: redrive/move-task identity and receive counts, FIFO batch and attribute refusal codes, tag-condition authorization, and the Policy limit error

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMessageMoveTask

## Description
Seven further distinct root causes in the sqs suite (data plane, redrive and authorization), separate from the protocol issue filed alongside it. Each is listed with its own failing check ids; all seven must be closed.

Cause 1 of 7 — a redriven message keeps the dead-letter copy's MessageId instead of being assigned a new one. AWS: a message moved back by a message move task is a NEW message on the destination: it gets a fresh MessageId and a fresh SentTimestamp. Emulator: the redriven message still carries the DLQ copy's MessageId. Why: consumers deduplicating on MessageId drop the redriven message as already-seen, so a redrive silently delivers nothing. Fix: message identity assignment in the message-move/redrive path. Traces: sqs.message-move-task-redrive-assigns-new-message-id-and-sent-timestamp.

Cause 2 of 7 — a redriven message's ApproximateReceiveCount is not restarted. AWS: after a redrive the message's receive count starts again at 1 and climbs 1, 2, ... until it dead-letters again at maxReceiveCount. Emulator: the distinct values observed after the redrive were [4] — the count carried over from before the move. Why: the message dead-letters again immediately (or never), so maxReceiveCount means nothing after a redrive and poison-message handling is unpredictable. Fix: per-message receive-count state in the redrive path. Traces: sqs.redriven-message-restarts-its-receive-count-and-dead-letters-again-at-maxreceivecount.

Cause 3 of 7 — a FIFO message moved to its DLQ does not take the original MessageId as its MessageDeduplicationId. AWS: when SQS moves a FIFO message to a FIFO dead-letter queue it sets the copy's MessageDeduplicationId to the ORIGINAL message's MessageId. Emulator: the copy carries the producer's dedup id ('dedup-3f99d271'). Why: two different originals that shared a dedup id collapse into one on the DLQ (or a legitimate copy is deduplicated away), so dead letters are silently lost. Fix: the FIFO dead-letter move path's dedup-id assignment. Traces: sqs.fifo-dead-letter-queue-move-sets-dedup-id-to-original-message-id.

Cause 4 of 7 — a move task with no DestinationArn over a directly-sent message settles COMPLETED instead of FAILED. AWS: StartMessageMoveTask without DestinationArn redrives to each message's original source queue; a message that was sent DIRECTLY to the queue has no recorded source, so the task settles FAILED with a 'could not determine source' failure reason. Emulator: Status='COMPLETED'. Why: a client is told the redrive succeeded while the messages went nowhere; the one case where SQS refuses to guess is reported as success. Fix: source attribution and task failure in the StartMessageMoveTask execution path. Traces: sqs.start-message-move-task-on-directly-sent-message-fails-could-not-determine-source.

Cause 5 of 7 — a FIFO SendMessageBatch entry without MessageGroupId is refused with the wrong error code. AWS: the batch is refused whole with code 'InvalidParameterValue' and message 'The request must contain the parameter MessageGroupId.' (fixed text; the parameter name is the variable part); the group's stream is unchanged. Smithy: SendMessageBatch models neither MissingParameter nor InvalidParameterValue, so this is recorded behavior — the recorded code is InvalidParameterValue. Emulator: code 'MissingParameter' with the same message. Why: clients that branch on InvalidParameterValue (the code AWS returns for every other malformed-parameter refusal) do not recognise the failure. Fix: the error code chosen by the FIFO validation in the SendMessageBatch handler. Traces: sqs.fifo-batch-with-a-group-less-entry-rejected-whole-and-group-stream-unchanged.

Cause 6 of 7 — an over-long message-attribute DataType is refused with the wrong diagnosis. AWS: a DataType longer than the 256-character limit is refused as a length violation of that member. Emulator: the refusal says "The type of message (user) attribute 'N' is invalid. You must use only the following supported type prefixes: Binary, Number, String." — a type-prefix message for a value whose prefix is valid and whose only fault is length. Why: the client is told the attribute type is unsupported and goes looking for the wrong bug. Fix: message-attribute DataType validation (check length separately from the prefix) in the SendMessage handler. Traces: sqs.message-attribute-type-length-bounds-rejected.

Cause 7 of 7 — identity-policy tag conditions are not enforced (and resource-tag conditions never become true). AWS: an identity policy whose Allow is conditioned on aws:RequestTag/<key> refuses a CreateQueue that carries no such tag (AccessDenied), and an Allow conditioned on aws:ResourceTag/<key> permits SendMessage once the queue carries that tag. Emulator: the conditioned CreateQueue SUCCEEDS (HTTP 200, QueueUrl returned), and the resource-tag-gated SendMessage never becomes allowed within 54s (last='AccessDenied'). Why: tag-based authorization — the mechanism a tenant isolation model rests on — is simultaneously too permissive (request tags ignored) and too strict (resource tags never satisfied), so neither direction can be trusted. Fix: condition-key evaluation (aws:RequestTag, aws:ResourceTag) in the SQS authorization path. Traces: sqs.identity-policy-requesttag-condition-gates-create-queue, sqs.identity-policy-resourcetag-condition-gates-send-message.

One further message defect belongs to the same SetQueueAttributes validation component as Cause 6 and must be closed with it: an oversized Policy document is refused with code 'InvalidAttributeValue' where AWS returns 'OverLimit' (Smithy: SetQueueAttributes declares OverLimit with awsQueryError code 'OverLimit', HTTP 403, alongside InvalidAttributeValue) — a client cannot tell a malformed policy from one that exceeds the statement limits. Traces: sqs.set-queue-attributes-policy-statement-limits-rejected.

## Evidence
Cause 1: AssertionError: the redriven message kept the DLQ copy's MessageId '3b62f1f7-4c06-4aa4-bbb8-d29503103faa' instead of being assigned a new one
Cause 2: AssertionError: after redrive: distinct ApproximateReceiveCount values ran [4], expected 1, 2, ...
Cause 3: AssertionError: a message moved from a FIFO queue to its FIFO DLQ must carry the original MessageId '6eeeb377-de93-43e1-87d6-29e8579151b9' as its MessageDeduplicationId, got 'dedup-3f99d271'
Cause 4: AssertionError: a move task with no DestinationArn over a directly-sent message must settle FAILED, got Status='COMPLETED'
Cause 5: AssertionError: a FIFO batch with a group-less entry was refused with ('MissingParameter', 'The request must contain the parameter MessageGroupId.'), expected ('InvalidParameterValue', 'The request must contain the parameter MessageGroupId.')
Cause 6: AssertionError: 257-byte DataType: unexpected message "The type of message (user) attribute 'N' is invalid. You must use only the following supported type prefixes: Binary, Number, String."
Cause 7: AssertionError: expected the call to be refused, but it succeeded: {'QueueUrl': 'https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-behav-reqtag-notag-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}; TimeoutError: call never became allowed within 54.85930164799993s (last='AccessDenied')
Policy limit: AssertionError: oversized Policy: expected OverLimit, got 'InvalidAttributeValue'

## Steps to Reproduce
Cause 1/2: send to a queue with a DLQ redrive policy, let a message dead-letter, StartMessageMoveTask back, then read the redriven message's MessageId and ApproximateReceiveCount across receives.
Cause 3: let a FIFO message dead-letter to a FIFO DLQ and read the copy's MessageDeduplicationId.
Cause 4: send a message directly to a queue, then StartMessageMoveTask(SourceArn=<that queue>) with no DestinationArn and poll ListMessageMoveTasks.
Cause 5: SendMessageBatch to a FIFO queue with one entry lacking MessageGroupId.
Cause 6: SendMessage with a MessageAttribute whose DataType is 257 characters starting 'Number'.
Cause 7: assume a role whose Allow on sqs:CreateQueue is conditioned on aws:RequestTag/<key> and call CreateQueue without tags; separately condition sqs:SendMessage on aws:ResourceTag/<key> and call it against a queue carrying that tag.
Policy limit: SetQueueAttributes with a Policy exceeding the statement limits.
