Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.sse-sqs-is-the-default-and-exclusive-with-sse-kms sqs.deploy-cfn-encryption-update-keeps-earlier-messages-readable sqs.clearing-kms-master-key-id-does-not-restore-sse-sqs sqs.kms-data-key-reuse-period-bounds-rejected sqs.sse-kms-invalid-key-rejected sqs.sse-kms-producer-needs-generate-data-key-on-a-customer-key sqs.kms-key-permission-gates-send-message sqs.sse-kms-consumer-without-decrypt-drives-message-to-dlq
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.sse-sqs-is-the-default-and-exclusive-with-sse-kms|run#4|AssertionError|assert _sse(sqs, url) == {"SqsManagedSseEnabled": "true"}, f"a new queue's encryption was {_sse(sqs, url)!r}"
Issue-Key: sqs.deploy-cfn-encryption-update-keeps-earlier-messages-readable|run#10|AssertionError|assert _sse(sqs, url) == {"SqsManagedSseEnabled": "true"}, f"deployed default encryption {_sse(sqs, url)!r}"
Issue-Key: sqs.clearing-kms-master-key-id-does-not-restore-sse-sqs|run#10|AssertionError|assert before.get("SqsManagedSseEnabled") == "false", f"an SSE-KMS queue read {before!r}"
Issue-Key: sqs.kms-data-key-reuse-period-bounds-rejected|_reuse#8|KeyError|return sqs.get_queue_attributes(QueueUrl=url, AttributeNames=["KmsDataKeyReusePeriodSeconds"])[
Issue-Key: sqs.sse-kms-invalid-key-rejected|_error_code#10|AssertionError|raise AssertionError(f"expected the send to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sqs.sse-kms-producer-needs-generate-data-key-on-a-customer-key|run#24|AssertionError|assert err["Code"] == "KMS.AccessDeniedException", f"send to the customer-key queue gave {err!r}"
Issue-Key: sqs.kms-key-permission-gates-send-message|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: sqs.sse-kms-consumer-without-decrypt-drives-message-to-dlq|run#22|AssertionError|assert got == [], f"a role without kms:Decrypt must never receive the body; it got {got!r}"
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS encryption is inert: a new queue reports no SSE at all, KmsDataKeyReusePeriodSeconds is absent, an invalid CMK is accepted, and no KMS grant gates produce or consume (6 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueAttributes

## Description
Server-side encryption on SQS queues is represented only as free-form attribute storage: the defaults are missing, the attributes are not mutually exclusive, and KMS is never consulted.

Cause 1 of 6: SSE-SQS is not the default. AWS: a queue created with no encryption attributes reports SqsManagedSseEnabled='true' from GetQueueAttributes, and SSE-SQS is mutually exclusive with SSE-KMS. Emulator: a brand-new queue's encryption attributes read back as an empty map, and the same is true of a CloudFormation-deployed queue. Why: a client asserting its queues are encrypted at rest — or diffing the deployed attribute set — reads 'no encryption configured' for a queue AWS reports as encrypted. Fix: default and exclusivity handling for SqsManagedSseEnabled/KmsMasterKeyId in the CreateQueue handler and the queue attribute projection. Traces: sqs.sse-sqs-is-the-default-and-exclusive-with-sse-kms, sqs.deploy-cfn-encryption-update-keeps-earlier-messages-readable.

Cause 2 of 6: clearing KmsMasterKeyId does not restore SSE-SQS. AWS: SetQueueAttributes with an empty KmsMasterKeyId takes the queue off the CMK and back to SSE-SQS. Emulator: the queue still reads back KmsMasterKeyId with the old key id. Fix: the KmsMasterKeyId clear path in the SetQueueAttributes handler. Traces: sqs.clearing-kms-master-key-id-does-not-restore-sse-sqs.

Cause 3 of 6: KmsDataKeyReusePeriodSeconds is not reported. AWS: it is a readable queue attribute (default 300, range 60..86400) and out-of-range values are refused. Emulator: the attribute is absent from GetQueueAttributes entirely, so the check dies on KeyError: 'KmsDataKeyReusePeriodSeconds' before it can test the bounds. Fix: include the attribute in the queue attribute projection and validate its range in SetQueueAttributes/CreateQueue. Traces: sqs.kms-data-key-reuse-period-bounds-rejected.

Cause 4 of 6: an unusable CMK is accepted on send. AWS: SendMessage to a queue whose KmsMasterKeyId does not resolve to a usable key is refused with a KMS-family error — Smithy models KmsNotFound/KmsDisabled/KmsInvalidState/KmsAccessDenied/KmsInvalidKeyUsage on SendMessage with awsQueryError codes KMS.NotFoundException, KMS.DisabledException, KMS.InvalidStateException, KMS.AccessDeniedException. Emulator: the send returns 200 with a MessageId. Why: a key misconfiguration that AWS surfaces at the first send is invisible. Fix: resolve the queue's CMK against the KMS crate in the SendMessage handler and map the failure to the modelled Kms* errors. Traces: sqs.sse-kms-invalid-key-rejected.

Cause 5 of 6: the producer's kms:GenerateDataKey grant is not required. AWS: a producer without GenerateDataKey on the queue's customer key is refused KmsAccessDenied (KMS.AccessDeniedException, 400). Emulator: the send is accepted ('{Code: <accepted>}'), and the companion check never completed inside the 60 s cap. Fix: the same KMS authorization step in the SendMessage handler. Traces: sqs.sse-kms-producer-needs-generate-data-key-on-a-customer-key, sqs.kms-key-permission-gates-send-message.

Cause 6 of 6: the consumer's kms:Decrypt grant is not required. AWS: ReceiveMessage by a role without Decrypt on the queue's CMK is refused KmsAccessDenied and the message is never handed over (and eventually redrives). Emulator: ReceiveMessage returns 200 with the plaintext body. Why: an encryption boundary that AWS enforces is absent, so a consumer reads data it must not. Fix: the same KMS authorization step in the ReceiveMessage handler. Traces: sqs.sse-kms-consumer-without-decrypt-drives-message-to-dlq.

## Evidence
Expected (AWS / Smithy): a new queue reports SqsManagedSseEnabled='true'; KmsDataKeyReusePeriodSeconds is readable; SendMessage/ReceiveMessage against an unusable or ungranted CMK are refused with the modelled Kms* errors (awsQueryError KMS.NotFoundException / KMS.AccessDeniedException, 400) and have no effect.
Actual (emulator replay): "AssertionError: a new queue's encryption was {}"; 'deployed default encryption {}'; "an SSE-KMS queue read {'KmsMasterKeyId': 'edaa2c53-176c-4dae-92bd-3de41097e145'}" after the id was cleared; "KeyError: 'KmsDataKeyReusePeriodSeconds'"; 'expected the send to be rejected, but it SUCCEEDED and returned {MessageId: ..., HTTPStatusCode: 200}'; "send to the customer-key queue gave {'Code': '<accepted>', 'Message': ''}"; 'a role without kms:Decrypt must never receive the body; it got [{... Body: poisoned-by-missing-decrypt-...}]'.

## Steps to Reproduce
1. CreateQueue with no attributes, GetQueueAttributes(['All']) -> no SqsManagedSseEnabled, no KmsDataKeyReusePeriodSeconds.
2. SetQueueAttributes KmsMasterKeyId=<key>, then SetQueueAttributes KmsMasterKeyId='' -> the old key id is still reported.
3. SetQueueAttributes KmsMasterKeyId to a key id that does not resolve, then SendMessage -> 200 instead of a KMS.* refusal.
4. Create a CMK whose policy omits kms:GenerateDataKey for the caller, attach it to a queue, SendMessage -> accepted.
5. With a role lacking kms:Decrypt, ReceiveMessage from that queue -> 200 with the plaintext body.
6. Re-replay the check ids listed in trace_ids.
