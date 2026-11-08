Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt
Issue-Key: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt|_wait_message_held#19|TimeoutError|raise TimeoutError("message to become held (in flight, not visible): the run budget ended first; not graded")
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# kms__lambda__sqs: an event source mapping on an SSE-KMS queue never takes a message in flight, so a message the role cannot decrypt is never withheld-then-delivered (1 cause)

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: with an event source mapping polling an SQS queue encrypted with a customer KMS key, a sent message never becomes in flight (invisible but not deleted), so the withheld-until-decryptable behaviour the check grades is never reachable. Failing check: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt.

AWS: recorded behavior only — the polling and KMS-decrypt interaction of an SQS event source mapping is not described by a Smithy shape. An enabled Lambda event source mapping polls its SQS source continuously: a sent message is received by the poller, so ApproximateNumberOfMessagesNotVisible rises above zero while the mapping holds it (and ApproximateNumberOfMessages falls). When the mapping's execution role cannot kms:Decrypt the queue's CMK, the mapping cannot hand the payload to the function, so the message is held and redelivered rather than consumed or dropped; once the role can decrypt, the same message is delivered to the handler.

Emulator: the message is never taken in flight. The replay waits 90 s for the message to become held (in flight, not visible) and the wait never satisfies — `TimeoutError: message to become held (in flight, not visible): the run budget ended first; not graded`. Because the mapping never picks the message up, the KMS-gated hold/release behaviour is not merely wrong but unobservable. (The harness labels this step 'not graded'; the underlying cause is the emulator's poller, which is why it is reported here.)

Why: a client cannot model an SQS-triggered Lambda on an encrypted queue. Queue depth and in-flight counts stay wrong while a mapping is supposedly polling, and the security-relevant case — a mapping whose role lacks kms:Decrypt must withhold, not silently consume, the message — cannot be relied upon.

Fix: the SQS event source mapping poller (the component that receives from the source queue and makes messages in flight), and its KMS-decrypt precondition on an SSE-KMS source, which must hold and redeliver rather than consume when the role cannot decrypt.

Traces: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt.

## Evidence
Expected (AWS): shortly after SendMessage to the SSE-KMS queue with an enabled event source mapping, GetQueueAttributes shows ApproximateNumberOfMessagesNotVisible >= 1 (the mapping holds it); while the role cannot kms:Decrypt the CMK the message is not consumed, and after the grant it is delivered to the handler.

Actual (emulator): the in-flight state never appears within 90 s — `TimeoutError: message to become held (in flight, not visible): the run budget ended first; not graded`.

## Steps to Reproduce
1. Create a customer KMS key and an SQS queue with SSE-KMS using that key.
2. Create a Lambda whose execution role may receive from the queue but may NOT kms:Decrypt the key, and create an enabled event source mapping from the queue to the function.
3. SendMessage to the queue and poll GetQueueAttributes for ApproximateNumberOfMessagesNotVisible — it never rises, so the message is never taken in flight.
4. Replay kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt.
