Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.create-queue-existing-name-different-policy-not-conflicting sqs.create-queue-existing-name-different-tags-rejected sqs.queue-deleted-recently-on-fast-recreate-rejected sqs.list-queues-pagination-bad-input-rejected sqs.tag-queue-system-prefix-and-length-boundaries-rejected sqs.empty-message-body
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|CreateQueue|QueueAlreadyExists
Issue-Key: sqs.create-queue-existing-name-different-tags-rejected|_err#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.queue-deleted-recently-on-fast-recreate-rejected|_err#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.list-queues-pagination-bad-input-rejected|_err#5|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.tag-queue-system-prefix-and-length-boundaries-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.empty-message-body|_error_code#14|_UnexpectedSuccess|raise _UnexpectedSuccess(f"the empty-body send was ACCEPTED and returned {resp!r}")
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS queue lifecycle gaps: CreateQueue idempotency is inverted (differing Policy conflicts, differing tags does not), QueueDeletedRecently is never raised, ListQueues pagination and tag-key rules are unvalidated, and an empty MessageBody is accepted (6 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
Smithy: CreateQueue models QueueNameExists (awsQueryError QueueAlreadyExists, 400) and QueueDeletedRecently (awsQueryError AWS.SimpleQueueService.QueueDeletedRecently, 400); SendMessage requires MessageBody.

Cause 1 of 6: CreateQueue treats a differing Policy as a conflicting attribute. AWS: re-creating an existing queue with the same name and a DIFFERENT Policy is idempotent — Policy is not one of the attributes whose difference makes CreateQueue conflict — and returns the existing QueueUrl, 200. Emulator: it is refused with QueueAlreadyExists, 400, 'A queue already exists with the name <name> and attributes that differ from the ones this request supplied.' Why: an idempotent deploy or retry that re-asserts a queue with an updated policy fails where AWS succeeds. Fix: the conflicting-attribute set used by the CreateQueue handler. Traces: sqs.create-queue-existing-name-different-policy-not-conflicting.

Cause 2 of 6: CreateQueue accepts an existing name with differing TAGS. AWS: re-creating an existing queue with a different tag set is refused at 400. Emulator: it returns 200 with the QueueUrl. Why: the inverse of cause 1 — a genuine conflict is laundered into success, so the queue's tags silently stay as they were. Fix: the same comparison in the CreateQueue handler. Traces: sqs.create-queue-existing-name-different-tags-rejected.

Cause 3 of 6: QueueDeletedRecently is never raised. AWS: CreateQueue for a name deleted within the last 60 seconds is refused with AWS.SimpleQueueService.QueueDeletedRecently, 400. Emulator: the fast re-create returns 200 with a QueueUrl. Why: a client that handles this documented back-off never exercises it, and a recreate that AWS defers succeeds immediately — masking the receipt-handle/incarnation behaviour that depends on it. Fix: a recently-deleted-name ledger in the SQS crate's state, checked by the CreateQueue handler. Traces: sqs.queue-deleted-recently-on-fast-recreate-rejected.

Cause 4 of 6: ListQueues pagination input is not validated. AWS: a malformed NextToken / out-of-range MaxResults is refused at 400. Emulator: ListQueues returns 200 with a QueueUrls list. Why: a paginating client silently restarts from the beginning instead of being told its token is bad, which can loop forever. Fix: pagination validation in the ListQueues handler. Traces: sqs.list-queues-pagination-bad-input-rejected.

Cause 5 of 6: tag-key system prefix and length boundaries are not enforced. AWS: TagQueue with a key in the reserved 'aws:' prefix, or a key/value past the documented length boundary, is refused at 400 and the tag set is unchanged. Emulator: the call returns 200. Why: reserved and oversized tags are accepted, so tag-driven cost allocation and policy conditions diverge from AWS. Fix: tag-key validation in the TagQueue handler (sibling of the tag-count/value-length cause filed under the SQS refusal-taxonomy issue). Traces: sqs.tag-queue-system-prefix-and-length-boundaries-rejected.

Cause 6 of 6: SendMessage accepts an empty MessageBody. AWS: MessageBody is a required member and an empty string is refused with MissingParameter, 400 ('The request must contain the parameter MessageBody.'); nothing is enqueued. Emulator: the send is ACCEPTED and returns MD5OfMessageBody 'd41d8cd98f00b204e9800998ecf8427e' (the MD5 of the empty string) with a MessageId, 200. Why: a producer bug that AWS catches at the edge enqueues an empty message that consumers must then handle. Fix: the required/non-empty check for MessageBody in the SendMessage handler's validation. Traces: sqs.empty-message-body.

## Evidence
Expected (AWS / Smithy): CreateQueue with the same name and a differing Policy is idempotent (200, existing QueueUrl); with a differing tag set it is refused at 400; within 60 s of a delete it is refused AWS.SimpleQueueService.QueueDeletedRecently at 400; ListQueues rejects bad pagination input; TagQueue rejects 'aws:'-prefixed and oversized keys; SendMessage with MessageBody='' is refused MissingParameter at 400.
Actual (emulator replay): 'botocore.errorfactory.QueueNameExists: An error occurred (QueueAlreadyExists) when calling the CreateQueue operation: A queue already exists with the name sim2real-behav-cqpolicy-... and attributes that differ from the ones this request supplied.'; 'expected the call to be refused, but it succeeded: {QueueUrl: .../sim2real-behav-cqt-..., HTTPStatusCode: 200}' (differing tags); 'expected the call to be refused, but it succeeded: {QueueUrl: .../sim2real-behav-qdr-..., HTTPStatusCode: 200}' (fast recreate); 'expected the call to be refused, but it succeeded: {QueueUrls: [...], HTTPStatusCode: 200}' (ListQueues); 'expected the call to be refused, but it succeeded: {HTTPStatusCode: 200}' (TagQueue boundaries); 'the empty-body send was ACCEPTED and returned {MD5OfMessageBody: d41d8cd98f00b204e9800998ecf8427e, MessageId: 2aa79c4e-..., HTTPStatusCode: 200}'.

## Steps to Reproduce
1. CreateQueue 'q' with a Policy attribute; CreateQueue 'q' again with a DIFFERENT Policy -> QueueAlreadyExists instead of the existing QueueUrl.
2. CreateQueue 'r' with tags={'a':'1'}; CreateQueue 'r' again with tags={'a':'2'} -> 200 instead of a refusal.
3. DeleteQueue 'r', then immediately CreateQueue 'r' -> 200 instead of AWS.SimpleQueueService.QueueDeletedRecently.
4. ListQueues with a malformed NextToken -> 200 with QueueUrls.
5. TagQueue with a key 'aws:foo' -> 200.
6. SendMessage with MessageBody='' -> 200 with MD5OfMessageBody d41d8cd98f00b204e9800998ecf8427e.
7. Re-replay the check ids listed in trace_ids.
