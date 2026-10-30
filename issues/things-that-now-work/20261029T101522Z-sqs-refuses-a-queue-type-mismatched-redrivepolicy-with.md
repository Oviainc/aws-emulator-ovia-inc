Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.redrive-policy-dlq-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.redrive-policy-dlq-type-mismatch-rejected|run#15|AssertionError|assert message == MISMATCH_MESSAGE.format(policy=_redrive(dlq_arn)), (
Issue-Commit: 992ff7064f71a6c13a404188e850cf82f701572d

# SQS refuses a queue-type-mismatched RedrivePolicy with the wrong Reason text (1 cause)

> _Note: this issue was identified against the code base at commit `992ff7064f71a6c13a404188e850cf82f701572d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
Cause 1 of 1 — SetQueueAttributes correctly refuses a RedrivePolicy that pairs a standard source queue with a FIFO dead-letter queue (and the reverse), and it correctly uses error code InvalidParameterValue, but the refusal's Reason clause states the rule in wording that is not AWS's.

AWS: SetQueueAttributes with a RedrivePolicy whose deadLetterTargetArn names a queue of the other queue type than the source is refused at HTTP 400 with error code 'InvalidParameterValue' and this message template:
  Value <the RedrivePolicy document exactly as submitted> for parameter RedrivePolicy is invalid. Reason: Dead-letter queue must be same type of queue as the source..
Fixed text: the literal prefix 'Value ' and the literal suffix ' for parameter RedrivePolicy is invalid. Reason: Dead-letter queue must be same type of queue as the source..' — including the DOUBLED terminating period, which is what AWS emits (the Reason sentence ends in a period and the message's own period follows it). Variable part: the policy document, echoed back verbatim from the request; its deadLetterTargetArn and maxReceiveCount are the caller's values and are not part of the template. The same code and the same Reason sentence apply in both directions (standard source -> FIFO DLQ and FIFO source -> standard DLQ), and in both cases the attribute is NOT stored, so a subsequent GetQueueAttributes(['RedrivePolicy']) omits the Attributes key entirely.
Smithy: com.amazonaws.sqs#SetQueueAttributes models the attribute-value refusal as InvalidAttributeValue (httpError 400, defaulted) alongside InvalidAttributeName, QueueDoesNotExist (awsQueryError AWS.SimpleQueueService.NonExistentQueue) and UnsupportedOperation; com.amazonaws.sqs#CreateQueue models the same InvalidAttributeValue for a policy supplied at create time. The model carries no message text, so the exact wording above is recorded behavior only, taken from the recorded 400 response.

Emulator: the status and the code are right and the Reason clause is wrong. The same call answers 'InvalidParameterValue' with message 'Value <policy document> for parameter RedrivePolicy is invalid. Reason: The dead-letter queue of a FIFO queue must also be a FIFO queue. Similarly, the dead-letter queue of a standard queue must also be a standard queue.' That is two sentences of AWS *documentation* prose about the constraint, not the one-sentence Reason AWS returns on the wire. The 'Value ... for parameter RedrivePolicy is invalid. Reason: ' prefix and the echoed policy document are correct, so only the Reason clause diverges; the doubled period is also absent.

Why: in an SQS InvalidParameterValue the Reason clause is the only field that tells a caller WHICH constraint it violated, and it is routinely matched by SDK wrappers, infrastructure tools and operator runbooks. A client that string-matches AWS's 'Dead-letter queue must be same type of queue as the source.' sees no match and falls through to a generic 'invalid parameter' path, losing the type-mismatch diagnosis entirely; conversely an emulator-trained matcher will not work against real AWS. Because the code is identical for every RedrivePolicy defect, the Reason is the discriminator and it is unusable.

Fix: the message template of the queue-type-compatibility check in the SQS crate's RedrivePolicy validator, shared by the SetQueueAttributes and CreateQueue handlers. It must emit AWS's single-sentence Reason with the doubled terminating period, and keep the other RedrivePolicy Reason clauses distinct (the cross-region deadLetterTargetArn Reason is a separate, already-open defect — see 20261028T014019Z-sqs-cross-region-redrivepolicy-is-refused-with-the.md, which covers only the region rule's wording and says nothing about the queue-type rule; the open 20261029T003445Z-sqs-a-dead-letter-move-never-happens-on-an.md covers redrive ENFORCEMENT, not refusal wording). Neither of those issues covers this cause.

Traces: sqs.redrive-policy-dlq-type-mismatch-rejected.

## Evidence
Replay verdict: [mismatch] sqs.redrive-policy-dlq-type-mismatch-rejected — AssertionError at check_sqs_redrive_policy_dlq_type_mismatch_rejected.py line 95, on the first of the two directions the check exercises ('standard source -> FIFO DLQ').

Expected (AWS): HTTP 400, Error.Code 'InvalidParameterValue', Error.Message 'Value {"deadLetterTargetArn": "<arn of the FIFO DLQ just created>", "maxReceiveCount": 3} for parameter RedrivePolicy is invalid. Reason: Dead-letter queue must be same type of queue as the source..'

Actual (emulator): Error.Code 'InvalidParameterValue' (correct), Error.Message 'Value {"deadLetterTargetArn": "arn:aws:sqs:us-west-2:000000000000:sim2real-behav-48e60dd6224b-dlq.fifo", "maxReceiveCount": 3} for parameter RedrivePolicy is invalid. Reason: The dead-letter queue of a FIFO queue must also be a FIFO queue. Similarly, the dead-letter queue of a standard queue must also be a standard queue.'

In that actual message the ARN, the queue name fragment and the account id are per-run values from the request and are NOT part of the requirement; the divergence is entirely in the fixed Reason text after 'is invalid. Reason: '. The required fixed text is 'Dead-letter queue must be same type of queue as the source..'; the emulator emits the two-sentence documentation prose instead.

The check aborts at this first assertion, so the remainder of its contract produced no observation and is NOT evidenced here — a fixer should confirm it while re-replaying: (a) that the refused RedrivePolicy was not stored at all, i.e. GetQueueAttributes(QueueUrl=<std source>, AttributeNames=['RedrivePolicy']) returns a response with NO 'Attributes' key; (b) that the reverse direction (FIFO source -> standard DLQ) is refused with the SAME code and the SAME Reason sentence and likewise stores nothing; and (c) the positive controls, that same-type pairings (standard->standard, FIFO->FIFO) are accepted and read back with deadLetterTargetArn exactly as submitted and maxReceiveCount 3.

## Steps to Reproduce
1. CreateQueue '<base>-std-src' and '<base>-std-dlq' (no attributes); CreateQueue '<base>-src.fifo' and '<base>-dlq.fifo' with Attributes={'FifoQueue': 'true'}.
2. GetQueueAttributes(AttributeNames=['QueueArn']) on '<base>-std-dlq' and '<base>-dlq.fifo' to get their ARNs.
3. Confirm the standard source has no RedrivePolicy yet: GetQueueAttributes(QueueUrl=<std-src>, AttributeNames=['RedrivePolicy']) returns no 'Attributes' key.
4. SetQueueAttributes(QueueUrl=<std-src url>, Attributes={'RedrivePolicy': '{"deadLetterTargetArn": "<fifo dlq arn>", "maxReceiveCount": 3}'}).
5. Read Error.Code and Error.Message. Code is InvalidParameterValue (correct). The Message's Reason clause is 'The dead-letter queue of a FIFO queue must also be a FIFO queue. Similarly, the dead-letter queue of a standard queue must also be a standard queue.'; AWS returns 'Dead-letter queue must be same type of queue as the source..' (note the doubled period), with the submitted policy document echoed verbatim in the 'Value ...' prefix.
6. Repeat step 4 in the reverse direction: SetQueueAttributes(QueueUrl=<fifo-src url>, Attributes={'RedrivePolicy': '{"deadLetterTargetArn": "<standard dlq arn>", "maxReceiveCount": 3}'}) — same code and same Reason sentence are required.
7. After each refusal, GetQueueAttributes(AttributeNames=['RedrivePolicy']) on the source must return no 'Attributes' key (nothing stored, not even partially).
8. Control: set the same policy with a same-type DLQ on each source; both must be accepted and read back with deadLetterTargetArn intact and maxReceiveCount 3.
9. Re-replay the check id listed in trace_ids.
