Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.batch-total-size-exceeds-limit-rejected sqs.change-message-visibility-expired-handle-accepted sqs.change-message-visibility-batch-repeated-receipt-handle-last-entry-wins sqs.conc-limits-and-fanout-fifo-stale-owner-handle-rejected-after-handoff sqs.dead-letter-queue-redrive-after-max-receive-count sqs.dlq-redrive sqs.fifo-group-blocked-by-a-poison-message-resumes-after-the-dlq-move sqs.dlq-move-withheld-until-the-maxreceivecount-th-delivery sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo sqs.message-move-task-completes-and-drains-dlq sqs.sse-kms-consumer-without-decrypt-drives-message-to-dlq sqs.deploy-cfn-kms-queue-inline-policy-enforces-key-and-queue-grants sqs.queue-policy-action-names-outside-the-sqs-action-list-rejected sqs.op-visibility-timeout-out-of-range-rejected sqs.start-message-move-task-invalid-rate-rejected sqs.sender-id-and-sent-timestamp-derive-from-the-caller-and-the-live-clock sqs.fifo-twenty-group-head-of-line-isolation-at-volume
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.batch-total-size-exceeds-limit-rejected|run#14|AssertionError|assert code == ERR_BATCH_TOO_LONG, (
Issue-Key: sqs|ChangeMessageVisibility|AWS.SimpleQueueService.MessageNotInflight
Issue-Key: sqs.change-message-visibility-batch-repeated-receipt-handle-last-entry-wins|run#4|AssertionError|assert ok == {"extend", "release"} and not resp.get("Failed"), (
Issue-Key: sqs.conc-limits-and-fanout-fifo-stale-owner-handle-rejected-after-handoff|run#18|AssertionError|assert err.get('Code') == 'InvalidParameterValue', (
Issue-Key: sqs.dead-letter-queue-redrive-after-max-receive-count|run#15|AssertionError|assert not redelivered_from_src, (
Issue-Key: sqs.dlq-redrive|run#12|AssertionError|raise AssertionError("message never landed in the DLQ")
Issue-Key: sqs.fifo-group-blocked-by-a-poison-message-resumes-after-the-dlq-move|_drive_source_until_successor#23|AssertionError|raise AssertionError(
Issue-Key: sqs.dlq-move-withheld-until-the-maxreceivecount-th-delivery|_receive#14|Timeout|return sqs.receive_message(
Issue-Key: sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo|_await_dlq_move#19|Timeout|for msg in sqs.receive_message(
Issue-Key: sqs.message-move-task-completes-and-drains-dlq|run#14|Timeout|sqs.change_message_visibility(
Issue-Key: sqs.sse-kms-consumer-without-decrypt-drives-message-to-dlq|run#22|Timeout|time.sleep(1)
Issue-Key: sqs|SendMessage|AccessDenied
Issue-Key: sqs.queue-policy-action-names-outside-the-sqs-action-list-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.op-visibility-timeout-out-of-range-rejected|run#10|AssertionError|assert err.get("Message") == MSG_HIGH, f"got {err!r}"
Issue-Key: sqs.start-message-move-task-invalid-rate-rejected|run#10|AssertionError|assert message == (
Issue-Key: sqs.sender-id-and-sent-timestamp-derive-from-the-caller-and-the-live-clock|_system_attribute#18|AssertionError|assert name in attrs, (
Issue-Key: sqs.fifo-twenty-group-head-of-line-isolation-at-volume|_hold_group_head#34|AssertionError|raise AssertionError(f"never received a message for group {group!r} in 120 single receives")
Issue-Commit: 5d59434299be1c81c983c1f9331cdf2ffaedfa51

# sqs: 10 net-new behavioral root causes — BatchRequestTooLong served as a bare 413, receipt-handle lifetime and stale-handle refusals, no automatic DLQ redrive, queue-policy grants/validation ignored, two refusal-message templates, missing SenderId, and FIFO receive starvation

> _Note: this issue was identified against the code base at commit `5d59434299be1c81c983c1f9331cdf2ffaedfa51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessageBatch, ChangeMessageVisibility, ChangeMessageVisibilityBatch, DeleteMessage, ReceiveMessage, SetQueueAttributes, StartMessageMoveTask

## Description
Ten distinct, independent root causes in the SQS emulator, each grounded in its own failing behavioral check(s). They are numbered below and each carries its own failing check ids; all ten must be closed before this issue is submitted. None of these checks is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 10: SendMessageBatch does not raise BatchRequestTooLong for an over-limit aggregate payload; it answers a bare HTTP 413 with an empty body.
AWS: Smithy (com-amazonaws-sqs-amazonsqs.json) binds `BatchRequestTooLong` to SendMessageBatch as a client error with httpError 400 and aws.protocols#awsQueryError code `AWS.SimpleQueueService.BatchRequestTooLong`, documented "The length of all the messages put together is more than the limit." The bound is the queue's `MaximumMessageSize` attribute, and the refusal message is the fixed template `Batch requests cannot be longer than {MaximumMessageSize} bytes. You have sent {total request bytes} bytes.` — the two numbers are values read from the queue's attribute and from the request, the rest is fixed text. All entries are refused; nothing enqueues.
Emulator: for a batch of four individually-legal entries whose sum exceeds the queue's MaximumMessageSize, the response is HTTP status `413` with an empty body, so botocore surfaces Error.Code `'413'` and Error.Message `''` — no modeled error, no code, no message.
Why: a client that catches `BatchRequestTooLong` (or any modeled SQS error) never matches, because a bare 413 with no body is not a parsable SQS error document; SDK retry/classification logic treats it as an unknown transport failure instead of a sender fault, and the caller never learns which limit it exceeded.
Fix: the SendMessageBatch handler's aggregate-size validation and the error serializer for the awsQuery/awsJson1.0 SQS protocol — the limit must be checked before entry processing and rendered as the modeled BatchRequestTooLong error, not as a raw 413.
Traces: sqs.batch-total-size-exceeds-limit-rejected.

Cause 2 of 10: a receipt handle whose visibility window has already lapsed is rejected with MessageNotInflight; in AWS the handle stays valid for the lifetime of the message.
AWS: a receipt handle remains usable for as long as the message exists, not only while the message is in flight. ChangeMessageVisibility on a lapsed handle is accepted and re-hides the message; applying VisibilityTimeout=0 with the same handle releases it again. Smithy models `MessageNotInflight` (awsQueryError `AWS.SimpleQueueService.MessageNotInflight`, httpError 400, doc "The specified message isn't in flight.") but it is not what AWS returns for a handle that simply outlived its visibility window. The same rule binds ChangeMessageVisibilityBatch: entry-Id uniqueness (BatchEntryIdsNotDistinct) is a property of entry Ids, not of receipt handles, so two entries of one batch naming the same handle under two distinct Ids both land in `Successful` with no `Failed`, in either entry order.
Emulator: ChangeMessageVisibility on a lapsed handle is refused with HTTP 400 `AWS.SimpleQueueService.MessageNotInflight` (empty message). In the batch form, the entry that is evaluated after the sibling entry released the message comes back under `Failed` as `{'Id': 'extend', 'SenderFault': True, 'Code': 'MessageNotInflight', 'Message': "The specified message isn't in flight."}` while only the other entry succeeds. One root cause: the handler gates on "is this message currently in flight?" instead of "does this message still exist?".
Why: the standard consumer pattern of extending or releasing visibility from a handle held across a lapse breaks — the consumer cannot re-hide a message it is still working on, and a batch visibility change that AWS reports as fully successful is reported as partially failed, so the caller retries or abandons work AWS would have accepted.
Fix: the ChangeMessageVisibility and ChangeMessageVisibilityBatch handlers' handle-resolution step and the queue's message state — resolve the handle against message existence, and reserve MessageNotInflight for the case AWS reserves it for. Note the two open scaffold issues 20261025T065401Z-sqs-changemessagevisibility-stub.md and 20261025T065401Z-sqs-changemessagevisibilitybatch-stub.md; they describe these operations as unimplemented stubs, which is no longer what the replay shows (both operations answer with modeled errors), so neither covers this cause.
Traces: sqs.change-message-visibility-expired-handle-accepted, sqs.change-message-visibility-batch-repeated-receipt-handle-last-entry-wins.

Cause 3 of 10: a stale receipt handle superseded by a later receive is refused with ReceiptHandleIsInvalid/404 instead of InvalidParameterValue/400.
AWS: on a FIFO queue, ownership of an in-flight message moves with the receive. Once receiver A releases a message and receiver B receives it, A's handle is dead, and both DeleteMessage and ChangeMessageVisibility with it are refused with Error.Code `InvalidParameterValue` and the fixed-text template `Value {receipt handle} for parameter ReceiptHandle is invalid. Reason: The receipt handle has expired.` — the handle is the only variable part. Smithy models `ReceiptHandleIsInvalid` separately (awsQueryError `ReceiptHandleIsInvalid`, httpError 404, doc "The specified receipt handle isn't valid."), which is the wrong class for a handle that was valid and has been superseded. The refusal is real: the message stays B's and is redelivered to B with ApproximateReceiveCount 3 after B releases it.
Emulator: the stale-owner DeleteMessage is refused with `{'Code': 'ReceiptHandleIsInvalid', 'QueryErrorCode': 'ReceiptHandleIsInvalid', 'Type': 'Sender', 'Message': 'The input receipt handle is invalid.'}` — wrong error code, wrong HTTP status class (404 rather than 400), and a message that is not AWS's template.
Why: a consumer distinguishing "this handle was never mine / is malformed" (ReceiptHandleIsInvalid) from "my lease expired, re-receive" (InvalidParameterValue, "The receipt handle has expired") takes the wrong recovery branch, and a 404 is retried or surfaced as a missing resource rather than as a lease handoff.
Fix: the DeleteMessage and ChangeMessageVisibility handlers' handle-validation branch and the per-message ownership/generation state that distinguishes a superseded handle from an unknown one.
Traces: sqs.conc-limits-and-fanout-fifo-stale-owner-handle-rejected-after-handoff.

Cause 4 of 10: there is no automatic redrive to a dead-letter queue — maxReceiveCount is stored but never enforced, so an over-count message is redelivered from the source queue forever.
AWS: when a queue carries a RedrivePolicy `{"deadLetterTargetArn": ..., "maxReceiveCount": N}`, the N+1'th delivery attempt moves the message to the dead-letter queue instead of redelivering it from the source; the source queue's depth drops and the DLQ's rises. On a FIFO queue this is also the only thing that unblocks a message group whose head is a poison message: once the head moves to the DLQ, the group's successor is delivered. Recorded behavior only for the move's timing; the move itself is the contract.
Emulator: maxReceiveCount is accepted and read back but never counted. The replay shows the message "redelivered from the source queue instead" of being routed once maxReceiveCount=1 was exceeded; a second check reports the message "never landed in the DLQ"; and the FIFO arm observes 5186 redeliveries of the poison head without a move, so the successor in that message group was never delivered and the group is stalled forever. The DLQ-dependent checks that merely hang to the 60 s harness cap (dlq-move-withheld-until-the-maxreceivecount-th-delivery, dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo, message-move-task-completes-and-drains-dlq, sse-kms-consumer-without-decrypt-drives-message-to-dlq) are waiting on the same move that never happens.
Why: every poison-message design relying on a DLQ silently degrades to infinite redelivery; a FIFO message group blocked by one bad message never advances, and nothing a consumer can do recovers it. This also makes a source queue's depth and a DLQ's depth both wrong, so every depth-based alarm and every redrive/move-task workflow built on top of it is unobservable.
Fix: the receive path's per-message receive-count accounting and the queue state's redrive step — increment the count on each delivery and, past maxReceiveCount, enqueue the message on the RedrivePolicy target instead of making it visible again. Open issue 20261025T065401Z-sqs-never-redrives-to-a-dead-letter-queue.md describes the same root cause and owns 14 other checks' replay keys, but not the three listed below, so they are filed here to extend that replay set; closing it must re-replay these too.
Traces: sqs.dead-letter-queue-redrive-after-max-receive-count, sqs.dlq-redrive, sqs.fifo-group-blocked-by-a-poison-message-resumes-after-the-dlq-move, sqs.dlq-move-withheld-until-the-maxreceivecount-th-delivery, sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo, sqs.message-move-task-completes-and-drains-dlq, sqs.sse-kms-consumer-without-decrypt-drives-message-to-dlq.

Cause 5 of 10: a resource-based queue policy is not consulted when authorizing a non-owner principal — the grantee is refused as if only identity-based policy counted.
AWS: an SQS queue policy (here deployed as `AWS::SQS::QueueInlinePolicy`, granting `sqs:SendMessage`/`sqs:ReceiveMessage`/`sqs:DeleteMessage` to a principal via an `ArnEquals` condition on `aws:PrincipalArn`) is sufficient on its own for a same-account principal; no identity-based policy is required. In the recorded scenario the grantee's SendMessage therefore passes SQS authorization and is refused one layer further in, by KMS, with Error.Code `KMS.AccessDeniedException` and the message `User: {caller arn} is not authorized to perform: kms:GenerateDataKey on resource: {key arn} because no identity-based policy allows the kms:GenerateDataKey action (Service: AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: {request id}; Proxy: null)` — fixed text, with the caller ARN, key ARN and request id supplied by the request and the service. Granting the role kms:GenerateDataKey/kms:Decrypt then lets the same session send and receive.
Emulator: the grantee's SendMessage is refused by SQS with `AccessDenied` and the message `User: {assumed-role arn} is not authorized to perform: sqs:sendmessage on resource: {queue arn} because no identity-based policy allows the sqs:sendmessage action`, and it stays refused for the whole 120 s propagation window and past the KMS grant. Two defects in one: the queue's resource policy is not consulted at all, and the action name in the message is lower-cased (`sqs:sendmessage`) where AWS renders the canonical API action name as written in the policy evaluation (`sqs:SendMessage`).
Why: every cross-principal access pattern that relies on a queue policy rather than an identity policy — the normal way to let another role, another account or a service send to a queue — is denied, so a correctly configured deployment cannot be exercised at all; and the lower-cased action makes the message not match what a client or an operator greps for.
Fix: the SQS authorization component that evaluates a request against the queue's `Policy` attribute (it must be consulted alongside identity policy, not after it), and the AccessDenied message renderer's action-name casing.
Traces: sqs.deploy-cfn-kms-queue-inline-policy-enforces-key-and-queue-grants.

Cause 6 of 10: SetQueueAttributes accepts a queue Policy whose Action names are not valid SQS actions.
AWS: SQS validates every `Action` in a queue policy against its own action list and refuses anything that is not a literal valid action name — a partial wildcard (`sqs:Send*`, `sqs:*Message`), an action that is not in the list, an unknown action, and a bare `*` are each refused with Error.Code `InvalidParameterValue` and the fixed-text template `Value {action} for parameter ActionName is invalid. Reason: Please refer to the appropriate WSDL for a list of valid actions.` (the action string is the only variable part). Smithy models `InvalidAttributeValue` on SetQueueAttributes as a client/400 error. The refusal leaves the previously stored policy untouched and still in force. `sqs:*` and a lower-case `sqs:sendmessage` are both accepted.
Emulator: each of the five refused Action values is accepted — SetQueueAttributes answers HTTP 200 with an empty body, so the document is stored unparsed.
Why: a misconfigured policy that AWS rejects at deploy time is accepted by the emulator and surfaces only in production, where the wildcard grants nothing (or everything) the author expected; a deployment pipeline validated against the emulator ships a policy AWS will refuse.
Fix: the SetQueueAttributes handler's `Policy` attribute validation — parse the document and validate each statement's Action against the SQS action list before storing.
Traces: sqs.queue-policy-action-names-outside-the-sqs-action-list-rejected.

Cause 7 of 10: the out-of-range VisibilityTimeout refusal message does not use AWS's wording.
AWS: ChangeMessageVisibility with a VisibilityTimeout outside 0..43200 is refused with Error.Code `InvalidParameterValue` and the fixed-text template `Value {supplied value} for parameter VisibilityTimeout is invalid. Reason: VisibilityTimeout must be an integer between 0 and 43200.` The supplied value is taken from the request; the rest, including the bounds, is fixed text. Both the above-maximum and the negative case use the same template.
Emulator: the code is right (`InvalidParameterValue`) but the Reason clause is the emulator's own wording: `Value {supplied value} for parameter VisibilityTimeout is invalid. Reason: VisibilityTimeout must be >= 0 and <= 43200.`
Why: an error message is part of the contract; clients and operators that match on the Reason text (and test suites that assert it) diverge, and the emulator's phrasing appears nowhere in AWS.
Fix: the ChangeMessageVisibility handler's range validation message template.
Traces: sqs.op-visibility-timeout-out-of-range-rejected.

Cause 8 of 10: the out-of-range StartMessageMoveTask rate refusal message does not use AWS's wording.
AWS: StartMessageMoveTask with `MaxNumberOfMessagesPerSecond` above the documented 500 cap is refused with Error.Code `InvalidParameterValue` and the fixed text `Value for parameter MaxNumberOfMessagesPerSecond is invalid. Reason: You must enter a number that's between 1 and 500.` — note that this template does NOT echo the supplied value; it is entirely fixed text. No move task is created.
Emulator: the code is right but the message is the emulator's own template, and it additionally echoes the value: `Value 501 for parameter MaxNumberOfMessagesPerSecond is invalid. Reason: Must be >= 1 and <= 500.`
Why: same as Cause 7 — the refusal text diverges from AWS for an operation whose only observable on this path is the message. This belongs to the same family as open issue 20261025T225310Z-sqs-5-net-new-behavioral-root-causes-sendmessagebatch.md (emulator-authored refusal templates), but that issue carries neither this operation nor Cause 7's, so it does not cover them.
Fix: the StartMessageMoveTask handler's parameter validation message template.
Traces: sqs.start-message-move-task-invalid-rate-rejected.

Cause 9 of 10: ReceiveMessage never returns the SenderId system attribute.
AWS: Smithy's `MessageSystemAttributeName` enum lists `SenderId` alongside SentTimestamp, ApproximateReceiveCount, ApproximateFirstReceiveTimestamp, SequenceNumber, MessageDeduplicationId, MessageGroupId, AWSTraceHeader and DeadLetterQueueSourceArn, and `ReceiveMessage(MessageSystemAttributeNames=['All'])` returns every supported one. SenderId is derived from the calling principal (the IAM user id, or the role id for an assumed role) and is identical for two sends by the same principal.
Emulator: `ReceiveMessage(MessageSystemAttributeNames=['All'])` returns only `['ApproximateFirstReceiveTimestamp', 'ApproximateReceiveCount', 'SentTimestamp']` — SenderId is absent entirely. Only the attribute's PRESENCE is filed here; the exact principal-id string it carries is account-specific and is not part of this report.
Why: SenderId is how a shared or cross-account queue attributes a message to its producer; a consumer that reads it gets a KeyError rather than a value, so producer attribution and audit paths cannot be built against the emulator at all.
Fix: the ReceiveMessage handler's system-attribute projection and the message state written by SendMessage/SendMessageBatch — stamp the caller's principal id at enqueue time and project it under `SenderId` when requested (including via `All`).
Traces: sqs.sender-id-and-sent-timestamp-derive-from-the-caller-and-the-live-clock.

Cause 10 of 10: on a FIFO queue, single-message receives never advance past the oldest visible message, so later message groups are starved.
AWS: ReceiveMessage on a FIFO queue serves the head of the eligible message groups, so with one group's head held in flight a consumer polling `MaxNumberOfMessages=1` and handing each non-target message straight back with `ChangeMessageVisibility(VisibilityTimeout=0)` reaches every other group's head within a bounded number of calls. Order is defined WITHIN a message group only; the cross-group interleaving is not defined, but every unblocked group must be reachable.
Emulator: with 20 groups of 25 messages on one FIFO queue and group `g00`'s head held in flight, 120 consecutive single-message receives never returned a single message for group `g07` — the check fails with "never received a message for group 'g07' in 120 single receives". The earlier arms of the same check pass (the control arm drains all 500, and holding `g00`'s head correctly isolates exactly that group while the other 19 drain 475 in per-group order), which localises the defect to group selection on receive rather than to per-group isolation. Hypothesis: the receive path picks the globally oldest visible message rather than rotating over eligible groups, so a message released with VisibilityTimeout=0 is immediately handed back as the next receive's result and the poller loops on one group forever.
Why: a FIFO consumer that releases messages it is not ready for — the normal way to implement selective or routed consumption — makes no progress at all past the first eligible group, and only the groups that happen to be oldest are ever served, which no amount of consumer concurrency recovers.
Fix: the ReceiveMessage handler's FIFO group-selection logic and the per-group cursor state — rotate over eligible message groups instead of returning the single globally-oldest visible message.
Traces: sqs.fifo-twenty-group-head-of-line-isolation-at-volume.

## Evidence
All quoted from the behavioral replay report (suite simparity-behavioral-ga-prod-v1, sqs 212/269 passed, 57 failed).

Cause 1 — sqs.batch-total-size-exceeds-limit-rejected:
  expected: Error.Code AWS.SimpleQueueService.BatchRequestTooLong, HTTP 400, Message "Batch requests cannot be longer than {MaximumMessageSize} bytes. You have sent {total} bytes."
  actual:   AssertionError: expected AWS.SimpleQueueService.BatchRequestTooLong for a 1398104-byte batch of 4 individually-legal 349526-byte entries, got '413' ('')

Cause 2 — sqs.change-message-visibility-expired-handle-accepted:
  expected: ChangeMessageVisibility accepted on a handle whose visibility window lapsed; the message is re-hidden, then released with VisibilityTimeout=0.
  actual:   botocore.errorfactory.MessageNotInflight: An error occurred (AWS.SimpleQueueService.MessageNotInflight) when calling the ChangeMessageVisibility operation:
  sqs.change-message-visibility-batch-repeated-receipt-handle-last-entry-wins:
  expected: both entry Ids under Successful, no Failed, in either entry order.
  actual:   AssertionError: leg2: a batch with two Ids sharing one receipt handle answered Successful=[{'Id': 'release'}] Failed=[{'Id': 'extend', 'SenderFault': True, 'Code': 'MessageNotInflight', 'Message': "The specified message isn't in flight."}]

Cause 3 — sqs.conc-limits-and-fanout-fifo-stale-owner-handle-rejected-after-handoff:
  expected: Code InvalidParameterValue, Message "Value {handle} for parameter ReceiptHandle is invalid. Reason: The receipt handle has expired."
  actual:   AssertionError: stale-owner DeleteMessage on 'm2' was refused with 'ReceiptHandleIsInvalid', expected InvalidParameterValue ({'Message': 'The input receipt handle is invalid.', 'Code': 'ReceiptHandleIsInvalid', 'QueryErrorCode': 'ReceiptHandleIsInvalid', 'Type': 'Sender'})

Cause 4 — sqs.dead-letter-queue-redrive-after-max-receive-count:
  actual:   AssertionError: expected 'redrive-me-to-dlq' to be routed to the DLQ once maxReceiveCount=1 was exceeded, but it was redelivered from the source queue instead
  sqs.dlq-redrive:
  actual:   AssertionError: message never landed in the DLQ
  sqs.fifo-group-blocked-by-a-poison-message-resumes-after-the-dlq-move:
  actual:   AssertionError: the message group never resumed: 'successor-...' was not delivered within 30.0s of the poison head becoming over-count (5186 poison redeliveries observed). A group that stays blocked here is stalled forever
  sqs.dlq-move-withheld-until-the-maxreceivecount-th-delivery / sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo / sqs.message-move-task-completes-and-drains-dlq / sqs.sse-kms-consumer-without-decrypt-drives-message-to-dlq:
  actual:   Error: Check timed out after 60.0s (all four are waiting on the DLQ move that never occurs)

Cause 5 — sqs.deploy-cfn-kms-queue-inline-policy-enforces-key-and-queue-grants:
  expected: the grantee's SendMessage passes SQS authorization via the deployed AWS::SQS::QueueInlinePolicy and is refused by KMS with Code KMS.AccessDeniedException, Message "User: {caller} is not authorized to perform: kms:GenerateDataKey on resource: {key arn} because no identity-based policy allows the kms:GenerateDataKey action (Service: AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: {id}; Proxy: null)".
  actual:   botocore.exceptions.ClientError: An error occurred (AccessDenied) when calling the SendMessage operation: User: arn:aws:sts::{account}:assumed-role/{role}/{session} is not authorized to perform: sqs:sendmessage on resource: arn:aws:sqs:{region}:{account}:{queue} because no identity-based policy allows the sqs:sendmessage action
  (the queue policy is never consulted, and the action name is lower-cased)

Cause 6 — sqs.queue-policy-action-names-outside-the-sqs-action-list-rejected:
  expected: for each of sqs:Send*, sqs:*Message, sqs:SendMessageBatch, sqs:Bogus and *: Code InvalidParameterValue, Message "Value {action} for parameter ActionName is invalid. Reason: Please refer to the appropriate WSDL for a list of valid actions.", with the stored policy unchanged.
  actual:   AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 7 — sqs.op-visibility-timeout-out-of-range-rejected:
  expected Message: 'Value 43201 for parameter VisibilityTimeout is invalid. Reason: VisibilityTimeout must be an integer between 0 and 43200.'
  actual:   AssertionError: got {'Message': 'Value 43201 for parameter VisibilityTimeout is invalid. Reason: VisibilityTimeout must be >= 0 and <= 43200.', 'Code': 'InvalidParameterValue'}

Cause 8 — sqs.start-message-move-task-invalid-rate-rejected:
  expected Message: "Value for parameter MaxNumberOfMessagesPerSecond is invalid. Reason: You must enter a number that's between 1 and 500."
  actual:   AssertionError: unexpected refusal message 'Value 501 for parameter MaxNumberOfMessagesPerSecond is invalid. Reason: Must be >= 1 and <= 500.'

Cause 9 — sqs.sender-id-and-sent-timestamp-derive-from-the-caller-and-the-live-clock:
  actual:   AssertionError: ReceiveMessage(MessageSystemAttributeNames=['All']) did not return SenderId; SQS documents it as a supported system attribute. Got ['ApproximateFirstReceiveTimestamp', 'ApproximateReceiveCount', 'SentTimestamp']

Cause 10 — sqs.fifo-twenty-group-head-of-line-isolation-at-volume:
  actual:   AssertionError: never received a message for group 'g07' in 120 single receives
  (arms 1 and 2 of the same check pass: control drains all 500 in per-group order, and holding g00's head isolates exactly g00 while the other 19 groups drain 475)

## Steps to Reproduce
Boot the emulator and replay the sqs behavioral checks listed in trace_ids. Per cause:

1. CreateQueue; read MaximumMessageSize; SendMessageBatch with 4 entries of (MaximumMessageSize/3 + 1) bytes each. Observe HTTP 413 with an empty body instead of a 400 AWS.SimpleQueueService.BatchRequestTooLong carrying "Batch requests cannot be longer than {limit} bytes. You have sent {total} bytes."
2. CreateQueue with VisibilityTimeout=1; SendMessage; ReceiveMessage; wait for ApproximateNumberOfMessagesNotVisible to return to 0; ChangeMessageVisibility with the now-lapsed handle and VisibilityTimeout=120. Observe AWS.SimpleQueueService.MessageNotInflight. Then: CreateQueue with VisibilityTimeout=20; SendMessage; ReceiveMessage; ChangeMessageVisibilityBatch with two entries sharing that one handle under Ids 'release' (0) and 'extend' (60), in both orders. Observe one entry under Failed with Code MessageNotInflight.
3. CreateQueue (.fifo, VisibilityTimeout=300); SendMessageBatch of 5 messages in 5 groups; receive all 5 with client A; release 3 with ChangeMessageVisibility(0); receive those 3 with client B; DeleteMessage with A's now-stale handle. Observe ReceiptHandleIsInvalid/404 rather than InvalidParameterValue/400 with "...Reason: The receipt handle has expired."
4. CreateQueue as DLQ; CreateQueue with RedrivePolicy {deadLetterTargetArn: <dlq>, maxReceiveCount: 1}; SendMessage; receive and release it repeatedly. Observe it keeps being redelivered from the source queue and never appears on the DLQ. Repeat on a FIFO pair with a poison head and observe the group never advances to its successor.
5. CreateStack with an AWS::KMS::Key, an AWS::SQS::Queue using it as KmsMasterKeyId, and an AWS::SQS::QueueInlinePolicy granting a role sqs:SendMessage/ReceiveMessage/DeleteMessage with an ArnEquals condition on aws:PrincipalArn; AssumeRole that role; SendMessage. Observe SQS AccessDenied ("no identity-based policy allows the sqs:sendmessage action") instead of SQS allowing it and KMS refusing with KMS.AccessDeniedException for kms:GenerateDataKey.
6. CreateQueue; SetQueueAttributes(Policy) with Action in turn sqs:Send*, sqs:*Message, sqs:SendMessageBatch, sqs:Bogus, *. Observe HTTP 200 each time instead of InvalidParameterValue naming ActionName.
7. CreateQueue; SendMessage; ReceiveMessage; ChangeMessageVisibility with VisibilityTimeout=43201 and then -1. Compare each Error.Message with AWS's "...Reason: VisibilityTimeout must be an integer between 0 and 43200."
8. CreateQueue as DLQ and a source with a RedrivePolicy naming it; StartMessageMoveTask(SourceArn=<dlq>, MaxNumberOfMessagesPerSecond=501). Compare Error.Message with AWS's "Value for parameter MaxNumberOfMessagesPerSecond is invalid. Reason: You must enter a number that's between 1 and 500."
9. CreateQueue; SendMessage; ReceiveMessage(MessageSystemAttributeNames=['All']). Observe Attributes has no SenderId key.
10. CreateQueue (.fifo, ContentBasedDeduplication, VisibilityTimeout=600); SendMessageBatch 500 messages across 20 MessageGroupIds g00..g19, 25 each, interleaved so consecutive sends never share a group; receive one message to hold g00's head in flight; then loop ReceiveMessage(MaxNumberOfMessages=1) handing back every non-g07 message with ChangeMessageVisibility(0). Observe no g07 message in 120 iterations.
