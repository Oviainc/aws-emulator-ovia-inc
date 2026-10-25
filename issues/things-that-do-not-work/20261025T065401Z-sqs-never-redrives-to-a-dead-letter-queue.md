Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.e2e-dlq-redrive-full-cycle sqs.deploy-cfn-dlq-redrive-behaves sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source sqs.redrive-allow-policy-denyall-does-not-block-start-message-move-task sqs.start-message-move-task-destination-arn-to-third-queue-bypasses-original-source sqs.start-message-move-task-self-referential-destination-rejected sqs.message-move-task-redrives-dlq-back-to-source sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source sqs.dead-lettered-message-keeps-its-identity-and-names-its-source-queue sqs.redrive-allow-policy-by-queue-rejects-unlisted-source sqs.redrive-allow-policy-deny-all-rejects-dlq-association-rejected sqs.redrive-policy-dlq-type-mismatch-rejected sqs.redrive-policy-max-receive-count-out-of-range-rejected sqs.redrive-policy-missing-target-or-count-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.e2e-dlq-redrive-full-cycle|run#15|AssertionError|assert dlq_message is not None, "message was never moved to the DLQ"
Issue-Key: sqs.deploy-cfn-dlq-redrive-behaves|run#18|AssertionError|assert dlq_message is not None, "message was never redriven to the DLQ"
Issue-Key: sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source|run#27|AssertionError|assert moved, f"after restoring the allow policy, {body!r} should have redirected to the DLQ"
Issue-Key: sqs.redrive-allow-policy-denyall-does-not-block-start-message-move-task|run#15|AssertionError|assert landed == "1", (
Issue-Key: sqs.start-message-move-task-destination-arn-to-third-queue-bypasses-original-source|run#15|AssertionError|assert landed == "1", (
Issue-Key: sqs.start-message-move-task-self-referential-destination-rejected|run#13|AssertionError|assert landed == "1", (
Issue-Key: sqs.message-move-task-redrives-dlq-back-to-source|_poll_receive_one#14|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=1, WaitTimeSeconds=3)
Issue-Key: sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source|run#20|Timeout|for m in sqs.receive_message(QueueUrl=src_url, MaxNumberOfMessages=1, WaitTimeSeconds=1, VisibilityTimeout=30).get("Messages", []):
Issue-Key: sqs.dead-lettered-message-keeps-its-identity-and-names-its-source-queue|run#9|AssertionError|assert stored.get("RedrivePolicy") == expected_policy, (
Issue-Key: sqs.redrive-allow-policy-by-queue-rejects-unlisted-source|_err#14|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sqs.redrive-allow-policy-deny-all-rejects-dlq-association-rejected|_err#10|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sqs.redrive-policy-dlq-type-mismatch-rejected|_err#15|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sqs.redrive-policy-max-receive-count-out-of-range-rejected|_err#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.redrive-policy-missing-target-or-count-rejected|_err#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS never redrives to a dead-letter queue: maxReceiveCount is stored but not counted, RedrivePolicy/RedriveAllowPolicy are not validated, and RedrivePolicy reads back malformed (4 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
The RedrivePolicy and RedriveAllowPolicy attributes round-trip as strings, but nothing in the emulator acts on them or checks them.

Cause 1 of 4: a message that exceeds maxReceiveCount is never moved to the DLQ. AWS: once a message's receive count exceeds the source queue's maxReceiveCount, SQS moves it to deadLetterTargetArn, where it becomes visible and ApproximateNumberOfMessages on the DLQ rises. Emulator: the message stays on the source queue forever — the DLQ's ApproximateNumberOfMessages reads '0' and the dependent checks either assert 'message was never redriven to the DLQ' or run out the 60 s harness cap. Why: every dead-letter design — poison-message isolation, DLQ alarms, redrive-after-fix — is unobservable, and a consumer loop that AWS would break out of spins forever. Fix: a receive-count/redrive step in the ReceiveMessage path of the SQS crate's queue state, driven by the parsed RedrivePolicy. Traces: sqs.e2e-dlq-redrive-full-cycle, sqs.deploy-cfn-dlq-redrive-behaves, sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source, sqs.redrive-allow-policy-denyall-does-not-block-start-message-move-task, sqs.start-message-move-task-destination-arn-to-third-queue-bypasses-original-source, sqs.start-message-move-task-self-referential-destination-rejected, sqs.message-move-task-redrives-dlq-back-to-source, sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source. (The last four reach their assertion only as a precondition — they need a dead-lettered message before the move task can be exercised — so they also gate the StartMessageMoveTask semantics behind this cause.)

Cause 2 of 4: RedrivePolicy does not read back as AWS serializes it. AWS: GetQueueAttributes returns the policy as compact JSON with a NUMERIC maxReceiveCount, e.g. {"deadLetterTargetArn":"arn:aws:sqs:...","maxReceiveCount":1}. Emulator: the value read back carries maxReceiveCount as a quoted string ('"maxReceiveCount": "1"') and is not compact. Why: a client that json-loads the attribute and compares the count numerically, or that re-submits the read value, breaks. Fix: the RedrivePolicy serializer in the queue attribute projection. Traces: sqs.dead-lettered-message-keeps-its-identity-and-names-its-source-queue.

Cause 3 of 4: RedriveAllowPolicy is not enforced on association. AWS: a DLQ whose RedriveAllowPolicy is redrivePermission=denyAll, or byQueue with a list that does not include the source, refuses the SetQueueAttributes that tries to wire that source's RedrivePolicy to it, at 400. Emulator: the association call returns 200. Why: an intentionally locked-down DLQ accepts arbitrary sources. Fix: evaluate the target queue's RedriveAllowPolicy in the SetQueueAttributes handler when RedrivePolicy is written. Traces: sqs.redrive-allow-policy-by-queue-rejects-unlisted-source, sqs.redrive-allow-policy-deny-all-rejects-dlq-association-rejected.

Cause 4 of 4: RedrivePolicy contents are not validated. AWS: wiring a standard queue as the DLQ of a FIFO queue (or vice versa), a maxReceiveCount outside 1..1000, and a policy missing deadLetterTargetArn or maxReceiveCount are each refused at 400 with the attribute unchanged. Emulator: all three return 200 and the bad policy is stored. Why: a queue is left with a redrive configuration AWS would never have accepted, and (per cause 1) nothing ever exercises it. Fix: RedrivePolicy parsing/validation in the SetQueueAttributes and CreateQueue handlers. Traces: sqs.redrive-policy-dlq-type-mismatch-rejected, sqs.redrive-policy-max-receive-count-out-of-range-rejected, sqs.redrive-policy-missing-target-or-count-rejected.

## Evidence
Expected (AWS): after maxReceiveCount+1 receives the message is on the DLQ and the DLQ's ApproximateNumberOfMessages is non-zero; RedrivePolicy reads back as compact JSON with a numeric maxReceiveCount; a denyAll/byQueue RedriveAllowPolicy and a malformed or type-mismatched RedrivePolicy are refused at 400.
Actual (emulator replay): 'AssertionError: message was never moved to the DLQ'; 'message was never redriven to the DLQ'; "precondition: '8ae5e6ba-retro' must have redriven into the DLQ before the move task is started; the DLQ's ApproximateNumberOfMessages read '0'"; 'Check timed out after 60.0s' (message-move-task-redrives-dlq-back-to-source, deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source); 'RedrivePolicy must read back as compact JSON with a numeric count, got s-eadd7df37abb", "maxReceiveCount": "1"}'; 'expected a rejection, but the call SUCCEEDED and returned {HTTPStatusCode: 200}' (byQueue unlisted source, denyAll association, DLQ type mismatch, out-of-range maxReceiveCount, missing target/count).

## Steps to Reproduce
1. CreateQueue DLQ and source; SetQueueAttributes on the source with RedrivePolicy {deadLetterTargetArn: <dlq arn>, maxReceiveCount: 1}.
2. SendMessage to the source, then ReceiveMessage twice without deleting, letting the visibility timeout lapse between receives.
3. GetQueueAttributes(['ApproximateNumberOfMessages']) on the DLQ -> '0'; the message is still on the source. AWS has it on the DLQ.
4. GetQueueAttributes(['RedrivePolicy']) on the source -> maxReceiveCount is the string "1", not the number 1.
5. SetQueueAttributes on the DLQ with RedriveAllowPolicy {redrivePermission: denyAll}, then wire a new source's RedrivePolicy to it -> 200.
6. SetQueueAttributes with RedrivePolicy maxReceiveCount=0, and with the deadLetterTargetArn of a FIFO queue on a standard source -> 200.
7. Re-replay the check ids listed in trace_ids.
