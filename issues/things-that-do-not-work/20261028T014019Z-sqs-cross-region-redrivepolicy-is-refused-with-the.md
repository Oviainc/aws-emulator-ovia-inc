Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.dead-letter-wiring-to-another-region-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.dead-letter-wiring-to-another-region-rejected|run#15|AssertionError|assert (err.get("Code"), err.get("Message")) == ("InvalidParameterValue", region_msg), (
Issue-Commit: f5d2818b51255ceb308b62fff342c02a5f2a7669

# SQS cross-region RedrivePolicy is refused with the wrong Reason text: the dead-letter-target region rule is reported with AWS's redrive-allow-policy wording (1 cause)

> _Note: this issue was identified against the code base at commit `f5d2818b51255ceb308b62fff342c02a5f2a7669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
Cause 1 of 1 — SetQueueAttributes now correctly refuses a RedrivePolicy whose deadLetterTargetArn lives in another region, but the refusal's message states a different rule than the one that was broken.

AWS: SetQueueAttributes with a RedrivePolicy whose deadLetterTargetArn is in a region other than the queue's own is refused at HTTP 400 with error code 'InvalidParameterValue' and the message template:
  Value <the RedrivePolicy document exactly as submitted> for parameter RedrivePolicy is invalid. Reason: Dead-letter target must be in same region as the source.
Fixed text: 'Value ', ' for parameter RedrivePolicy is invalid. Reason: Dead-letter target must be in same region as the source.' Variable part: the policy document, echoed back verbatim from the request (its deadLetterTargetArn and maxReceiveCount are the caller's values, not part of the template). The same code and the same Reason sentence are used when the policy is supplied on CreateQueue, and in both cases the attribute is not stored.
Smithy: sqs#SetQueueAttributes models the attribute-value refusal as InvalidAttributeValue (httpError 400, defaulted) alongside InvalidAttributeName / QueueDoesNotExist (awsQueryError AWS.SimpleQueueService.NonExistentQueue) / UnsupportedOperation; sqs#CreateQueue models the same InvalidAttributeValue. The model carries no message text, so the wording above is recorded behavior only, taken from the recorded 400 response.

Emulator: the code is right and the Reason is wrong. The same call answers code 'InvalidParameterValue' with message 'Value {"deadLetterTargetArn": "<far-region dlq arn>", "maxReceiveCount": "3"} for parameter RedrivePolicy is invalid. Reason: Only source queues in this Amazon Web Services account in the same Region can specify this queue as the dead-letter queue.' That trailing sentence is AWS's wording for a different rule entirely — the rule about which SOURCE queues a dead-letter queue will admit (the RedriveAllowPolicy / same-account-same-region source constraint) — not the rule about the dead-letter TARGET being in the source's region. The 'Value ... for parameter RedrivePolicy is invalid. Reason: ' prefix is correct, so only the Reason clause diverges.

Why: the Reason clause is the only thing in an SQS InvalidParameterValue that tells a caller WHICH constraint it violated; both of these sentences are real AWS sentences, so a client (or an operator reading a log) that branches on the Reason is sent to diagnose the dead-letter queue's RedriveAllowPolicy when the actual defect is the region of the deadLetterTargetArn it submitted. Retries after 'fixing' the allow policy keep failing with no new information.

Fix: the message template of the cross-region deadLetterTargetArn check in the SQS crate's RedrivePolicy validator, shared by the SetQueueAttributes and CreateQueue handlers — it must emit the dead-letter-target Reason, and keep the distinct allow-policy Reason for the RedriveAllowPolicy path (whose cross-region refusal AWS words as 'Value <allow document> for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the redrive allow policy. All source queue ARNs must be in the same region as the current SQS queue.').

Context, not a reopen: the refusal itself is present — it was introduced by the already-fixed issue 20261025T065401Z-sqs-has-no-per-region-identity-queuearn-hardcodes.md (its cause 4, where this same check previously failed with 'expected the call to be refused, but it succeeded'). The call is no longer wrongly accepted; what remains is the refusal's wording, which that issue never specified. The open issue 20261025T065401Z-sqs-never-redrives-to-a-dead-letter-queue.md covers RedrivePolicy enforcement, serialization and the type-mismatch / out-of-range / missing-member validations, and says nothing about the cross-region Reason text.

Traces: sqs.dead-letter-wiring-to-another-region-rejected.

## Evidence
Replay verdict: [mismatch] sqs.dead-letter-wiring-to-another-region-rejected — AssertionError at check_sqs_dead_letter_wiring_to_another_region_rejected.py line 71.

Expected (AWS): code 'InvalidParameterValue', message 'Value {"deadLetterTargetArn": "<arn of the DLQ created in the other region>", "maxReceiveCount": "3"} for parameter RedrivePolicy is invalid. Reason: Dead-letter target must be in same region as the source.'

Actual (emulator): code 'InvalidParameterValue' (correct), message 'Value {"deadLetterTargetArn": "arn:aws:sqs:us-east-1:000000000000:...-dlq", "maxReceiveCount": "3"} for parameter RedrivePolicy is invalid. Reason: Only source queues in this Amazon Web Services account in the same Region can specify this queue as the dead-letter queue.' Only the Reason clause after 'is invalid. Reason: ' differs; the echoed policy document and the prefix match.

The check stops at this first assertion, so the rest of its contract produced no observation and is NOT evidenced here (a fixer should confirm it while re-replaying): that the refused RedrivePolicy is absent from GetQueueAttributes(['All']); that CreateQueue carrying the same cross-region policy is refused with the same code and Reason and leaves no queue behind (GetQueueUrl -> AWS.SimpleQueueService.NonExistentQueue); that a RedriveAllowPolicy listing a source queue in another region is refused 'InvalidParameterValue' with 'Value <allow document> for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the redrive allow policy. All source queue ARNs must be in the same region as the current SQS queue.' and is not stored; and that the equivalent same-region documents are accepted, read back with deadLetterTargetArn intact, and ListDeadLetterSourceQueues reports exactly the one source queue.

## Steps to Reproduce
1. CreateQueue '<base>-src' and '<base>-dlq' against the home region; CreateQueue '<base>-dlq' against a second region. GetQueueAttributes(['QueueArn']) on each.
2. SetQueueAttributes(QueueUrl=<home src url>, Attributes={'RedrivePolicy': '{"deadLetterTargetArn": "<other-region dlq arn>", "maxReceiveCount": "3"}'}).
3. Read the error: the emulator answers InvalidParameterValue with Reason 'Only source queues in this Amazon Web Services account in the same Region can specify this queue as the dead-letter queue.' AWS answers the same code with Reason 'Dead-letter target must be in same region as the source.'
4. Also exercise CreateQueue with that same RedrivePolicy, and SetQueueAttributes on the home DLQ with RedriveAllowPolicy '{"redrivePermission": "byQueue", "sourceQueueArns": ["<other-region arn>"]}' -> the latter must be refused with the RedriveAllowPolicy Reason quoted above.
5. Re-replay the check id listed in trace_ids.
