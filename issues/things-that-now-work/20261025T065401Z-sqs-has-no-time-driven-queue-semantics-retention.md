Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.message-retention-period-expires-undeleted-message sqs.retention-expiry-discards-an-in-flight-message sqs.server-side-message-retention-auto-expunge sqs.queue-visibility-timeout-change-spares-in-flight-messages sqs.queue-delay-change-is-retroactive-for-fifo-but-not-standard sqs.standard-queue-concurrent-producers-and-consumers-lose-no-message
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.message-retention-period-expires-undeleted-message|run#22|Timeout|last_bodies = [m["Body"] for m in sqs.receive_message(
Issue-Key: sqs.retention-expiry-discards-an-in-flight-message|_receive#19|Timeout|return sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s,
Issue-Key: sqs.server-side-message-retention-auto-expunge|_poll_until_expunged#10|Timeout|time.sleep(interval_s)
Issue-Key: sqs.queue-visibility-timeout-change-spares-in-flight-messages|_receive#22|Timeout|return sqs.receive_message(
Issue-Key: sqs.queue-delay-change-is-retroactive-for-fifo-but-not-standard|_receive#18|Timeout|return sqs.receive_message(
Issue-Key: sqs.standard-queue-concurrent-producers-and-consumers-lose-no-message|run#13|Timeout|with concurrent.futures.ThreadPoolExecutor(max_workers=_PRODUCERS + _CONSUMERS) as ex:
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS has no time-driven queue semantics: retention never expires a message, and visibility/delay changes against live messages never converge (4 causes, all hitting the 60 s cap)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
Six checks that depend on SQS's time-driven queue behaviour all run out the 60 s harness cap with the awaited transition never happening. The common root is that the SQS crate's queue state has no clock-driven sweep: messages are only ever moved by an explicit API call. Recorded behavior only — no modelled error is involved; each check's awaited observable (a message disappearing, or reappearing) simply never occurs.

Cause 1 of 4: MessageRetentionPeriod never expires a message. AWS: an undeleted message older than MessageRetentionPeriod is discarded by the service; it stops being returned by ReceiveMessage and ApproximateNumberOfMessages drops. Emulator: the message stays forever and the check hangs. Why: retention-based cleanup — the backstop every queue design relies on — is absent, so a queue's depth never recovers and a test that asserts expiry can never pass. Fix: a retention sweep in the SQS crate's queue state, applied on read as well as on a timer. Traces: sqs.message-retention-period-expires-undeleted-message, sqs.server-side-message-retention-auto-expunge.

Cause 2 of 4: retention does not apply to an in-flight message. AWS: a message whose retention elapses while it is in flight (received but not deleted) is discarded and never becomes visible again. Emulator: the check hangs awaiting it. Fix: the same sweep, covering the in-flight set. Traces: sqs.retention-expiry-discards-an-in-flight-message.

Cause 3 of 4: a VisibilityTimeout or DelaySeconds change against live messages has no observable effect. AWS: lowering/raising the queue's VisibilityTimeout spares messages already in flight (they keep the timeout they were received under), and a DelaySeconds change is retroactive for FIFO queues but not for standard ones. Emulator: neither check ever reaches the transition it waits for. Fix: record the effective timeout/delay per message at receive/send time in the queue state, and honour the FIFO-vs-standard retroactivity rule in the SetQueueAttributes handler. Traces: sqs.queue-visibility-timeout-change-spares-in-flight-messages, sqs.queue-delay-change-is-retroactive-for-fifo-but-not-standard.

Cause 4 of 4: a standard queue under concurrent producers and consumers does not converge. AWS: with N producers and M consumers, every message is delivered at least once and none is lost. Emulator: the check hangs at the 60 s cap without accounting for all messages. Hypothesis: the same missing visibility/retention bookkeeping, or contention in the queue state, leaves messages neither visible nor in flight. Fix: the SQS crate's queue state concurrency and visibility accounting. Traces: sqs.standard-queue-concurrent-producers-and-consumers-lose-no-message.

## Evidence
Expected (AWS): an undeleted message older than MessageRetentionPeriod stops being returned and ApproximateNumberOfMessages drops; a message whose retention elapses in flight never reappears; a VisibilityTimeout change spares in-flight messages; a DelaySeconds change is retroactive on FIFO but not standard; concurrent producers/consumers lose no message.
Actual (emulator replay): all six checks end with 'Error: Check timed out after 60.0s' — sqs.message-retention-period-expires-undeleted-message, sqs.retention-expiry-discards-an-in-flight-message, sqs.server-side-message-retention-auto-expunge, sqs.queue-visibility-timeout-change-spares-in-flight-messages, sqs.queue-delay-change-is-retroactive-for-fifo-but-not-standard, sqs.standard-queue-concurrent-producers-and-consumers-lose-no-message. The awaited transition never occurs; no error is returned.

## Steps to Reproduce
1. CreateQueue with Attributes={'MessageRetentionPeriod':'60'}; SendMessage; wait past 60 s; ReceiveMessage -> the message is still returned and ApproximateNumberOfMessages is unchanged.
2. ReceiveMessage without deleting so the message is in flight, wait past the retention period -> it becomes visible again instead of being discarded.
3. ReceiveMessage with VisibilityTimeout=60, then SetQueueAttributes VisibilityTimeout=0 -> the in-flight message's effective timeout is not preserved.
4. Re-replay the check ids listed in trace_ids (each needs the full 60 s window).
