Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.message-move-task-completes-and-drains-dlq
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.message-move-task-completes-and-drains-dlq|run#14|Timeout|dlq_depth = sqs.get_queue_attributes(
Issue-Commit: 06adb3e2aae322591dd1176b14ac77cd8d29e903

# A COMPLETED SQS message move task is unreachable: maxReceiveCount is never counted so the DLQ stays empty, and ListMessageMoveTasks' moved/to-move counters are never exercised

> _Note: this issue was identified against the code base at commit `06adb3e2aae322591dd1176b14ac77cd8d29e903`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
One root cause.

Cause 1 of 1: with a source queue wired to a dead-letter queue by RedrivePolicy {deadLetterTargetArn, maxReceiveCount: 1} and VisibilityTimeout 1, repeated ReceiveMessage calls never move the messages to the DLQ, so the DLQ's ApproximateNumberOfMessages stays '0' and StartMessageMoveTask/ListMessageMoveTasks can never be driven to a COMPLETED task over a non-empty DLQ.

AWS: the redrive step is not described by the Smithy model (recorded behavior only) — once a message's receive count passes the source queue's maxReceiveCount, SQS moves it off the source onto deadLetterTargetArn, where it becomes visible and the DLQ's ApproximateNumberOfMessages rises while the source's falls to 0. The move task that drains it back IS modelled: Smithy (sqs) StartMessageMoveTask input SourceArn (String, @required), DestinationArn, MaxNumberOfMessagesPerSecond (NullableInteger), output TaskHandle; ListMessageMoveTasks input SourceArn (@required), MaxResults, output Results (list of ListMessageMoveTasksResultEntry with TaskHandle, Status — 'RUNNING, COMPLETED, CANCELLING, CANCELLED, and FAILED' — SourceArn, DestinationArn, MaxNumberOfMessagesPerSecond, ApproximateNumberOfMessagesMoved (Long, default 0, 'the approximate number of messages already moved to the destination queue'), ApproximateNumberOfMessagesToMove (NullableLong, obtained at the time of starting the task), FailureReason (only when FAILED) and StartedTimestamp). So a task over a two-message DLQ must settle at Status='COMPLETED' with ApproximateNumberOfMessagesToMove = the DLQ depth it sized at start and ApproximateNumberOfMessagesMoved >= that, re-deliver those messages on the destination queue, and leave the DLQ at ApproximateNumberOfMessages '0' and delivering nothing.

Emulator: the precondition never holds. Messages received past maxReceiveCount stay on the source queue, the DLQ reports ApproximateNumberOfMessages '0' and delivers nothing, so the loop that waits for the DLQ to fill never exits and the check is cut off with 'Check timed out after 60.0s' before StartMessageMoveTask is ever called. The move-task counters and the COMPLETED/drained-DLQ end state are therefore wholly unverified in this emulator.

Why: a client that relies on dead-lettering plus an operator redrive — the standard poison-message recovery path — gets a source queue that redelivers forever, a DLQ that looks permanently empty, and a redrive tool with nothing to move. Depth-based alarms on the DLQ never fire and an operator cannot tell a healthy queue from one stuck on a poison message.

Fix: the receive-count/redrive step in the SQS crate's ReceiveMessage path over the parsed RedrivePolicy (move the message to deadLetterTargetArn once its receive count passes maxReceiveCount), and then the StartMessageMoveTask/ListMessageMoveTasks task state so a task over a non-empty DLQ reaches Status='COMPLETED' and reports ApproximateNumberOfMessagesToMove and ApproximateNumberOfMessagesMoved rather than a COMPLETED task that moved nothing. Related open issue 20261025T065401Z-sqs-never-redrives-to-a-dead-letter-queue.md describes the same missing redrive step in its Cause 1, but does not carry this check id in its trace ids, and this check additionally grades the ListMessageMoveTasks counter and drained-DLQ end state that issue's traces never reach.

Traces: sqs.message-move-task-completes-and-drains-dlq.

## Evidence
Expected (AWS; move-task fields per the Smithy model, the redrive step from recorded behavior): after sending 2 messages to a source queue with VisibilityTimeout=1 and RedrivePolicy maxReceiveCount=1 and receiving each twice, the source's ApproximateNumberOfMessages is '0' and both messages are visible on the DLQ; StartMessageMoveTask(SourceArn=<dlq arn>, DestinationArn=<source arn>) returns a TaskHandle; ListMessageMoveTasks(SourceArn=<dlq arn>) settles at Status='COMPLETED' with DestinationArn=<source arn>, ApproximateNumberOfMessagesToMove=2 and ApproximateNumberOfMessagesMoved>=2; both bodies are then receivable on the destination and the DLQ reports ApproximateNumberOfMessages '0' and delivers nothing. The status token 'COMPLETED', the member names and the counter semantics (to-move = the DLQ depth sized at start, moved = messages already moved) are the requirement; the queue names, ARNs, message bodies and timestamps of any one run are values from the request and the emulator's state.

Actual (emulator replay): the DLQ's ApproximateNumberOfMessages never leaves '0' — the messages are redelivered on the source queue indefinitely instead of being dead-lettered — so the loop waiting for the DLQ to fill never completes and the replay reports 'Check timed out after 60.0s' for sqs.message-move-task-completes-and-drains-dlq, with StartMessageMoveTask and ListMessageMoveTasks never reached.

## Steps to Reproduce
1. CreateQueue a DLQ; GetQueueAttributes(['QueueArn']) on it.
2. CreateQueue a source queue with Attributes {VisibilityTimeout: '1', RedrivePolicy: '{"deadLetterTargetArn":"<dlq arn>","maxReceiveCount":1}'}; GetQueueAttributes(['QueueArn']) on it.
3. SendMessage two distinct bodies to the source queue.
4. Loop: ReceiveMessage(MaxNumberOfMessages=10, WaitTimeSeconds=1, AttributeNames=['ApproximateReceiveCount']) on the source and ChangeMessageVisibility(VisibilityTimeout=0) for each message, then GetQueueAttributes(['ApproximateNumberOfMessages']) on the DLQ. Observe the DLQ depth stays '0' and the source keeps redelivering; on AWS both messages land on the DLQ after the receive that takes them past maxReceiveCount.
5. (Unreachable in the emulator) StartMessageMoveTask(SourceArn=<dlq arn>, DestinationArn=<source arn>) and poll ListMessageMoveTasks(SourceArn=<dlq arn>) for Status='COMPLETED' with ApproximateNumberOfMessagesToMove=2 and ApproximateNumberOfMessagesMoved>=2, then confirm both bodies are receivable on the source and the DLQ is empty.
6. Re-replay the check id listed in trace_ids.
