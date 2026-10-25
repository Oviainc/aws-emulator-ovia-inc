Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.fifo-receive-request-attempt-id-replays-batch sqs.receive-request-attempt-id-replay-resets-visibility-timeout sqs.receive-request-attempt-id-not-retryable-after-batch-modified-rejected sqs.fifo-dedup-id-reusable-after-dedup-window-elapses sqs.fifo-throughput-limit-per-message-group-requires-message-group-scope-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.fifo-receive-request-attempt-id-replays-batch|run#16|AssertionError|assert replay_ids == first_ids, (
Issue-Key: sqs.receive-request-attempt-id-replay-resets-visibility-timeout|run#20|AssertionError|assert [m["MessageId"] for m in replay] == [first[0]["MessageId"]], (
Issue-Key: sqs.receive-request-attempt-id-not-retryable-after-batch-modified-rejected|run#15|AssertionError|assert [m["MessageId"] for m in replay] == [m["MessageId"] for m in batch], (
Issue-Key: sqs.fifo-dedup-id-reusable-after-dedup-window-elapses|run#13|Timeout|time.sleep(_DEDUP_WINDOW_WAIT_S)
Issue-Key: sqs.fifo-throughput-limit-per-message-group-requires-message-group-scope-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS FIFO receive-side idempotency is absent: ReceiveRequestAttemptId never replays a batch, the deduplication window never elapses, and FifoThroughputLimit is not validated (3 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
Smithy: ReceiveMessage carries ReceiveRequestAttemptId (String) and CreateQueue/SetQueueAttributes carry the FIFO attributes (FifoQueue, ContentBasedDeduplication, DeduplicationScope, FifoThroughputLimit) in QueueAttributeMap. None of the FIFO receive-side semantics behind them are implemented.

Cause 1 of 3: ReceiveRequestAttemptId does not replay a batch. AWS: on a FIFO queue, a ReceiveMessage retried with the SAME ReceiveRequestAttemptId within the 5-minute deduplication window returns the SAME batch of messages and resets their visibility timeout; the retry is refused only once the batch has been modified (a message deleted, or its visibility changed). Emulator: the replay returns an empty Messages list, so the original batch is lost to the caller, and the 'modified batch' negative cannot even reach its assertion because its positive control (an unmodified replay) returns []. Why: a FIFO consumer that retries a receive after a network timeout — the exact failure ReceiveRequestAttemptId exists for — sees an empty poll and either loses the batch until the visibility timeout lapses or double-processes it. Fix: track attempt-id -> batch in the SQS crate's per-queue receive state and honour it in the ReceiveMessage handler. Traces: sqs.fifo-receive-request-attempt-id-replays-batch, sqs.receive-request-attempt-id-replay-resets-visibility-timeout, sqs.receive-request-attempt-id-not-retryable-after-batch-modified-rejected.

Cause 2 of 3: the FIFO deduplication window never elapses. AWS: a MessageDeduplicationId suppresses a duplicate send for 5 minutes and is reusable afterwards. Emulator: the check never completes inside the 60 s harness cap, so no window boundary is ever observable. Hypothesis: the dedup bookkeeping has no time component, so either the id is suppressed forever or the check's wait for the window to lapse never resolves. Fix: timestamp the dedup-id ledger in the SQS crate's FIFO state. Traces: sqs.fifo-dedup-id-reusable-after-dedup-window-elapses.

Cause 3 of 3: FifoThroughputLimit is not validated against DeduplicationScope. AWS: CreateQueue with FifoThroughputLimit='perMessageGroupId' and a DeduplicationScope that is not 'messageGroup' is refused at 400 and no queue is created. Emulator: the call returns 200 with a QueueUrl. Why: a high-throughput FIFO configuration AWS rejects is accepted, so the deployment's assumptions about ordering/throughput scope are never checked. Fix: cross-attribute validation in the CreateQueue/SetQueueAttributes handler. Traces: sqs.fifo-throughput-limit-per-message-group-requires-message-group-scope-rejected.

## Evidence
Expected (AWS): a FIFO ReceiveMessage replayed with the same ReceiveRequestAttemptId returns the identical batch and resets its visibility; the same replay is refused once the batch is modified; a MessageDeduplicationId is reusable after the 5-minute window; FifoThroughputLimit=perMessageGroupId without DeduplicationScope=messageGroup is refused at 400.
Actual (emulator replay): 'AssertionError: replay with the same ReceiveRequestAttemptId returned [], expected the original batch [bdc5979f-..., 3de472f1-..., 0df1267a-...]'; 'a retry with the same ReceiveRequestAttemptId inside the visibility window must return the same message: got []'; 'positive control: an unmodified batch must replay under its attempt id, got []'; 'Check timed out after 60.0s' (fifo-dedup-id-reusable-after-dedup-window-elapses); 'expected the call to be refused, but it succeeded: {QueueUrl: .../sim2real-behav-272497bba006-bad.fifo, HTTPStatusCode: 200}'.

## Steps to Reproduce
1. CreateQueue with FifoQueue=true; SendMessage three messages with MessageGroupId set.
2. ReceiveMessage(MaxNumberOfMessages=3, ReceiveRequestAttemptId='r1') -> the batch.
3. ReceiveMessage again with ReceiveRequestAttemptId='r1' inside the visibility window -> Messages is [], not the same batch.
4. CreateQueue with FifoQueue=true, FifoThroughputLimit='perMessageGroupId' and DeduplicationScope='queue' -> 200 with a QueueUrl.
5. Re-replay the check ids listed in trace_ids.
