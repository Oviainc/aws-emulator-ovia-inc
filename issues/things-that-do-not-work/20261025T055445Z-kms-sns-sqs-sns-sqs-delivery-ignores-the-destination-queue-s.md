Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns
Issue-Key: kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns|run#21|AssertionError|assert leaked == {"ungranted": []}, (
Issue-Commit: 3b7a097804c74e5868f8202f9c831b57819d0d90

# SNS->SQS delivery ignores the destination queue's CMK key policy: a queue whose key does not grant sns.amazonaws.com receives every published message (1 cause)

> _Note: this issue was identified against the code base at commit `3b7a097804c74e5868f8202f9c831b57819d0d90`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: the SNS-to-SQS subscription delivery path performs no KMS authorization against the DESTINATION QUEUE's customer managed key, so delivery is never withheld when that key's policy does not permit the SNS service principal to use it. Failing check: kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns.

NOTE — relationship to already-open work: 20261024T102105Z-kms-sns-sqs-sns-never-delivers-to-sqs-subscribers-when-kms.md lists this check among its Issue-Trace-Ids, but its replay key binds the OLD symptom (the check timing out inside _await_bodies while nothing was ever enqueued). Delivery now happens, and the check gets past that point and fails on a DIFFERENT, later assertion — the suite did not mark this failure OWNED by that issue. The remaining defect is the key-policy gate, which that issue described only as a follow-on requirement; this issue carries it as the live replay key so it is not closed unrun. It is also distinct from 20261025T043441Z-cloudwatch-ecs-sns-sqs-... (destination queue's SQS RESOURCE policy, not its KMS key policy) and from 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-... (the TOPIC's policy and the TOPIC's key).

AWS: when an SQS queue is encrypted at rest with a customer managed key (queue attribute KmsMasterKeyId), SQS must call KMS on the queue owner's behalf to wrap each incoming message, and the caller on the delivery path is the SNS service principal. AWS therefore requires the CMK's key policy to permit sns.amazonaws.com to use the key (kms:GenerateDataKey* and kms:Decrypt); when it does not, the KMS call on the delivery path is refused and SNS CANNOT enqueue the notification, so the message is silently dropped at delivery rather than refused at Publish. Smithy (com-amazonaws-kms-trentservice): GenerateDataKey and Decrypt are the two operations on that path; both model the refusal classes a denied delivery hits (GenerateDataKey/Decrypt errors include KMSInvalidStateException 409, DisabledException 409, NotFoundException 404; an authorization refusal is the service-common AccessDeniedException 400). The model says nothing about SNS's delivery-path behavior itself — recorded behavior only: Publish still succeeds and returns a MessageId, the condition is observable ONLY as the absence of a delivery to that queue, and a twin queue whose CMK key policy does grant SNS receives every message in the same instant. After PutKeyPolicy adds the SNS grant, subsequently published messages do reach the same queue.

Emulator: both queues receive everything. The check subscribes two queues to one topic before any publish — one backed by a CMK whose policy grants sns.amazonaws.com, one backed by a CMK whose policy does not — then publishes three messages. The granted twin received all three (so the delivery path works), and during the following 15s window the UNGRANTED queue also received all three: ReceiveMessage returned {'ungranted': ['sim2real-xsvc-ssekmsq-m1-...', 'sim2real-xsvc-ssekmsq-m2-...', 'sim2real-xsvc-ssekmsq-m3-...']} where it must have returned []. The queue's KmsMasterKeyId is accepted and stored, but the key policy behind it is never consulted when SNS enqueues; the enqueue is not gated on a KMS call at all.

Why: the single most common real cause of "my SNS->SQS fan-out does not deliver" is exactly this missing key-policy grant. A client validating its encryption posture against the emulator sees delivery succeed, ships a CMK policy with no SNS grant, and in real AWS every notification to that queue is dropped with no error anywhere in the publish path. Conversely, no test can reproduce or regression-guard the failure mode, and a least-privilege key policy cannot be verified before it is deployed.

Fix: the SNS subscription delivery worker for sqs endpoints — it must, for a destination queue with KmsMasterKeyId set, perform the KMS envelope call as the sns.amazonaws.com service principal and evaluate that key's key policy, withholding (not enqueueing) the notification when the principal is not permitted, and resuming for messages published after PutKeyPolicy grants it. This spans the SNS delivery path, the SQS enqueue path's SSE-KMS handling, and KMS key-policy evaluation for service principals.

Traces: kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns (assertion at check line 68).

## Evidence
Failing check kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns [mismatch]:

  AssertionError: the queue whose key policy does not grant SNS must receive nothing from the topic; during a 15s window after its twin received all three it got {'ungranted': ['sim2real-xsvc-ssekmsq-m1-a89ed614753b', 'sim2real-xsvc-ssekmsq-m2-fb02f2369f10', 'sim2real-xsvc-ssekmsq-m3-c136bf4bbe78']}

Expected (AWS): the ungranted queue receives NOTHING from the topic — _drain returns {'ungranted': []} — while its twin, whose CMK key policy grants sns.amazonaws.com kms:GenerateDataKey*/kms:Decrypt, receives all three published bodies. Publish itself still returns 200 with a MessageId in both cases. After kms:PutKeyPolicy adds the SnsEncryptsIntoTheQueue statement, a newly published message does reach the previously ungranted queue.

Actual (emulator): the ungranted queue received all three published bodies within the 15s differential window, identically to the granted twin. The differential control passed (granted twin received all three) and the positive control passed (a direct sqs:SendMessage to the ungranted queue is received), so the queue and its key work and ONLY the SNS-principal authorization on the delivery path is absent. The generated marker names above are environment-specific examples; the requirement is the empty drain.

## Steps to Reproduce
1. sns:CreateTopic.
2. kms:CreateKey twice. Give key A a policy with an AccountAdministers statement plus a SnsEncryptsIntoTheQueue statement granting Principal {"Service": "sns.amazonaws.com"} kms:GenerateDataKey* and kms:Decrypt. Give key B a policy with the AccountAdministers statement ONLY (no SNS grant).
3. sqs:CreateQueue twice with Attributes {"KmsMasterKeyId": <key A>} and {"KmsMasterKeyId": <key B>}, each with a queue policy admitting the topic.
4. sns:Subscribe both queue ARNs with Attributes {"RawMessageDelivery": "true"} and confirm, BEFORE publishing anything.
5. Positive control: sqs:SendMessage directly to the key-B queue and receive it — confirms the queue and key work.
6. sns:Publish three distinct marker bodies to the topic.
7. Poll sqs:ReceiveMessage on the key-A queue until all three markers arrive (they do).
8. Poll sqs:ReceiveMessage on the key-B queue for a window of at least 15s. AWS: nothing arrives. Emulator: all three markers arrive — the divergence.
9. kms:PutKeyPolicy on key B adding the SNS grant, then sns:Publish a new marker and poll the key-B queue: it must now arrive, and kms:GetKeyPolicy must report exactly the Sids ["AccountAdministers", "SnsEncryptsIntoTheQueue"].
