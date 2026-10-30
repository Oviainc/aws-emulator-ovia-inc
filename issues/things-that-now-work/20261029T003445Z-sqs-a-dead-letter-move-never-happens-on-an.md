Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo|_await_retention_boundary_split#29|Timeout|for msg in sqs.receive_message(
Issue-Commit: f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14

# A dead-letter move never happens on an SQS queue wired with maxReceiveCount, so the DLQ copy's retention clock (carried over on standard, reset on FIFO) is unobservable

> _Note: this issue was identified against the code base at commit `f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
Cause 1 of 1: a message that exceeds the source queue's maxReceiveCount is never moved to deadLetterTargetArn, so the DLQ copy — and in particular the enqueue-timestamp/retention semantics of that move — never materialises. Failing check id: sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo.

Related open issue: 20261025T065401Z-sqs-never-redrives-to-a-dead-letter-queue.md describes the missing receive-count/redrive step (its cause 1) but does not carry this check's replay key and says nothing about the enqueue-timestamp behaviour of a move, which is the part this check measures; the redrive step must be implemented with that semantics for this check to pass.

AWS: recorded behavior and AWS documentation (SQS Developer Guide, dead-letter queues) only — the Smithy model carries the surface this relies on but not the rule: com.amazonaws.sqs#QueueAttributeName includes RedrivePolicy and MessageRetentionPeriod (MessageRetentionPeriod minimum 60 seconds), and com.amazonaws.sqs#MessageSystemAttributeName includes SentTimestamp, ApproximateReceiveCount and DeadLetterQueueSourceArn. AWS behaviour: once a ReceiveMessage attempt would hand a message back for the (maxReceiveCount + 1)-th time, SQS moves that message to the RedrivePolicy's deadLetterTargetArn, where it becomes receivable. SentTimestamp is the immutable original-send instant and is identical on the source copy and the DLQ copy for both queue types. The RETENTION clock, however, differs by queue type: for a STANDARD queue the message keeps its original enqueue instant, so it expires under the DLQ's MessageRetentionPeriod measured from the original send (a message held ~40s on the source and moved into a DLQ whose MessageRetentionPeriod is the 60s minimum disappears within seconds-to-tens-of-seconds of arriving); for a FIFO queue the enqueue instant is RESET at the move, so the same copy survives the whole 60s retention window from the moment it lands in the DLQ. Both queue types answer ApproximateReceiveCount '1' on the first delivery from the source.

Emulator: no move ever occurs. After the first delivery from both a standard and a FIFO source wired with RedrivePolicy {deadLetterTargetArn: <same-type DLQ>, maxReceiveCount: 1}, and a subsequent ChangeMessageVisibility(VisibilityTimeout=0) that forces the second delivery attempt, the DLQ never returns the message: the check's bounded poll that alternately pokes the source and reads the DLQ never sees the token and the check runs out the harness budget (reported as 'Check timed out after 60.0s'). Because no copy ever lands in a DLQ, neither the standard carry-over expiry nor the FIFO reset survival can be exercised at all, and SentTimestamp on a DLQ copy is never produced.

Why: every dead-letter design breaks. A poison message stays on the source queue forever instead of being isolated, so a consumer loop AWS would break out of spins indefinitely. Clients that reason about how long a dead-lettered message still lives — DLQ retention alarms, reprocessing windows, 'is this message about to expire' logic — get the opposite answer per queue type, and anything reading SentTimestamp or DeadLetterQueueSourceArn off a DLQ copy sees nothing at all.

Fix: the SQS crate's ReceiveMessage path and queue state: count delivery attempts per message, and when the count exceeds the parsed RedrivePolicy's maxReceiveCount, move the message to the target queue — carrying SentTimestamp over unchanged on both queue types, keeping the original enqueue instant for retention on a STANDARD target and resetting the retention/enqueue instant on a FIFO target, and exposing DeadLetterQueueSourceArn on the moved copy.

Traces: sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo.

## Evidence
Expected (AWS): with maxReceiveCount=1 on both a standard and a FIFO source, the second delivery attempt moves each message to its same-type DLQ; the DLQ copy's body and SentTimestamp equal the source copy's (SentTimestamp is 13-digit epoch milliseconds and unchanged by the move); the standard DLQ copy then expires inside the DLQ's 60s MessageRetentionPeriod measured from the original send, while the FIFO DLQ copy stays receivable for the full window because its enqueue instant reset at the move.
Actual (emulator replay): 'Check timed out after 60.0s' — the message never arrives on either DLQ. The check's _await_dlq_move loop (ReceiveMessage on the source with ChangeMessageVisibility(0), then ReceiveMessage on the DLQ with MessageSystemAttributeNames=['All']) never observes the token, so the move, the DLQ-side SentTimestamp and the standard-vs-FIFO retention split are all unreachable; the assertions 'the standard DLQ copy ... was still receivable ... its enqueue timestamp did not carry over from the original send' and 'the FIFO DLQ copy ... expired as though its enqueue timestamp had NOT reset' are never even reached.

## Steps to Reproduce
1. CreateQueue a standard DLQ with MessageRetentionPeriod=60, and a standard source with VisibilityTimeout=100 and RedrivePolicy {deadLetterTargetArn: <dlq arn>, maxReceiveCount: 1}.
2. CreateQueue the same pair as FIFO queues (FifoQueue=true, ContentBasedDeduplication=true on both; DLQ MessageRetentionPeriod=60).
3. SendMessage a tokened body to each source (MessageGroupId on the FIFO one).
4. ReceiveMessage once from each source with MessageSystemAttributeNames=['All'] — note SentTimestamp and that ApproximateReceiveCount is '1'. Do not delete.
5. Wait ~40s without touching the queues, then ChangeMessageVisibility(VisibilityTimeout=0) on both receipt handles to force the second delivery attempt.
6. Poll the source (re-exposing anything it returns) and the DLQ: in AWS the tokened message appears on each DLQ; in the emulator it never does and the check exhausts its budget.
7. With the move working, release both DLQ copies with ChangeMessageVisibility(0) and poll both DLQs for ~45s: the standard copy must vanish (retention measured from the original send) while the FIFO copy remains receivable throughout, and SentTimestamp on each DLQ copy must equal the source copy's.
8. Re-replay sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo.
