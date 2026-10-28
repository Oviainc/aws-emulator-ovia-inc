Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.message-move-task-completes-and-drains-dlq
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.message-move-task-completes-and-drains-dlq|run#14|Timeout|for msg in sqs.receive_message(
Issue-Commit: 48f5d55163f51ee89d5b03c70a361001e80d86fa

# SQS StartMessageMoveTask never completes a DLQ drain: no COMPLETED task with ApproximateNumberOfMessagesMoved/ToMove, and the automatic redrive that feeds it never happens (1 cause)

> _Note: this issue was identified against the code base at commit `48f5d55163f51ee89d5b03c70a361001e80d86fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMessageMoveTask

## Description
Cause 1 of 1: a message-move task over a dead-letter queue never becomes an observable, completed drain, and its precondition — automatic redrive at maxReceiveCount — never fires either, so the whole StartMessageMoveTask lifecycle is unreachable.

AWS: Smithy — StartMessageMoveTask(SourceArn required, DestinationArn, MaxNumberOfMessagesPerSecond) returns TaskHandle; ListMessageMoveTasks(SourceArn required) returns Results of ListMessageMoveTasksResultEntry. The model documents the entry's fields: Status is one of RUNNING, COMPLETED, CANCELLING, CANCELLED, FAILED; TaskHandle "is only populated for tasks that are in RUNNING status"; ApproximateNumberOfMessagesMoved is "the approximate number of messages already moved to the destination queue" (long, default 0); ApproximateNumberOfMessagesToMove is "the number of messages to be moved from the source queue ... obtained at the time of starting the message movement task"; DestinationArn echoes the request's destination; FailureReason is "only included if the task status is FAILED". So on AWS: a source queue wired with RedrivePolicy {deadLetterTargetArn, maxReceiveCount} hands a message up to its DLQ once the receive count exceeds maxReceiveCount; StartMessageMoveTask on that DLQ ARN with a DestinationArn then transitions through RUNNING to Status=COMPLETED, reports ApproximateNumberOfMessagesToMove equal to the DLQ depth it sized at start and ApproximateNumberOfMessagesMoved at least that many, redelivers every moved message on the destination queue, and leaves the DLQ at ApproximateNumberOfMessages=0 and delivering nothing.

Emulator: the check runs out the 60 s harness cap before it can even start a move task. Its first stage — receive both messages on a VisibilityTimeout=1 / maxReceiveCount=1 source, re-expose each with ChangeMessageVisibility(0), and poll the DLQ's ApproximateNumberOfMessages until it reaches 2 — never sees the DLQ depth rise, so that loop (150 s budget) is still spinning when the cap hits. Nothing past it is reached: no TaskHandle is ever obtained, no ListMessageMoveTasks entry is ever inspected, and the COMPLETED-status / ApproximateNumberOfMessagesMoved / ApproximateNumberOfMessagesToMove / DestinationArn / destination-redelivery / DLQ-drained assertions are all unexercised. Verdict in the replay report: [mismatch] sqs.message-move-task-completes-and-drains-dlq, "Error: Check timed out after 60.0s".

Why: a client that uses the documented recovery path — let poison messages fall to a DLQ, fix the consumer, then StartMessageMoveTask the DLQ back to the source and poll ListMessageMoveTasks for COMPLETED — gets a task that never terminates and counters that never account for anything, with the messages still sitting on the source queue. Every DLQ-based operational runbook (poison-message isolation, backlog replay, DLQ depth alarms) is inert, and a poller written against AWS waits forever on a Status that never changes.

Fix: two components in the SQS crate. (a) the receive-count/redrive step in the ReceiveMessage path of the queue state, driven by the parsed RedrivePolicy, so a message past maxReceiveCount is actually moved to deadLetterTargetArn; (b) the StartMessageMoveTask/ListMessageMoveTasks handlers and their task state, so a task is sized at start (ApproximateNumberOfMessagesToMove), advances RUNNING -> COMPLETED, drops TaskHandle once it leaves RUNNING, accumulates ApproximateNumberOfMessagesMoved as it moves messages, echoes DestinationArn, and actually transfers the messages so the source of the move is drained.

Related but NOT covering this check: open issue 20261025T065401Z-sqs-never-redrives-to-a-dead-letter-queue.md describes the same missing automatic-redrive step and bundles four redrive causes, but it does not carry this check's replay key and its causes stop at the redrive/validation layer — the COMPLETED-task status and the ApproximateNumberOfMessagesMoved / ApproximateNumberOfMessagesToMove reporting asserted here are not among them.

## Evidence
Expected (AWS): with a 2-message DLQ, StartMessageMoveTask(SourceArn=<dlq arn>, DestinationArn=<src arn>) returns a TaskHandle; ListMessageMoveTasks(SourceArn=<dlq arn>) then shows one entry reaching Status='COMPLETED' with DestinationArn=<src arn>, ApproximateNumberOfMessagesToMove=2 and ApproximateNumberOfMessagesMoved>=2; both bodies are receivable on the destination queue; the DLQ reports ApproximateNumberOfMessages='0' and delivers nothing on a further peek.

Actual (emulator replay): the check never reaches StartMessageMoveTask. The DLQ-fill stage polls GetQueueAttributes(['ApproximateNumberOfMessages']) on the DLQ after repeated ReceiveMessage/ChangeMessageVisibility(0) cycles on a maxReceiveCount=1 source and the depth never leaves '0', so the check is still inside that loop when the harness kills it: 'Error: Check timed out after 60.0s'. No TaskHandle, no ListMessageMoveTasks entry, no Status, and no ApproximateNumberOfMessagesMoved/ToMove value is ever produced.

## Steps to Reproduce
1. CreateQueue a DLQ; read its QueueArn via GetQueueAttributes(['QueueArn']).
2. CreateQueue a source queue with Attributes {VisibilityTimeout: '1', RedrivePolicy: '{"deadLetterTargetArn":"<dlq arn>","maxReceiveCount":1}'}; read its QueueArn.
3. SendMessage two distinct bodies to the source queue.
4. Loop: ReceiveMessage(QueueUrl=<source>, MaxNumberOfMessages=10, WaitTimeSeconds=1, AttributeNames=['ApproximateReceiveCount']) and ChangeMessageVisibility(VisibilityTimeout=0) on each handle; after each pass read GetQueueAttributes(QueueUrl=<dlq>, ['ApproximateNumberOfMessages']). Observed: it stays '0'. AWS: it reaches '2' once each message's receive count exceeds maxReceiveCount=1, and the source queue's ApproximateNumberOfMessages falls to '0'.
5. StartMessageMoveTask(SourceArn=<dlq arn>, DestinationArn=<source arn>) -> expect a TaskHandle.
6. Poll ListMessageMoveTasks(SourceArn=<dlq arn>, MaxResults=10) for Status in (COMPLETED, FAILED, CANCELLED). AWS settles on 'COMPLETED' with DestinationArn=<source arn>, ApproximateNumberOfMessagesToMove=2 and ApproximateNumberOfMessagesMoved>=2.
7. ReceiveMessage on the source queue -> both bodies; GetQueueAttributes(QueueUrl=<dlq>, ['ApproximateNumberOfMessages']) -> '0', and a further peek delivers nothing.
8. Re-replay sqs.message-move-task-completes-and-drains-dlq.
