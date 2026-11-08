Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-fifo-queue-type-immutability-and-suffix-rules
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-fifo-queue-type-immutability-and-suffix-rules sqs-queue-permissions-and-tags sqs-queue-tags-and-permissions-management sqs-dead-letter-queue-and-message-move sqs-message-move-task-lifecycle sqs-fifo-queue-ordering-and-dedup
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|CreateQueue|4xx->2xx|$.__type
Issue-Key: sqs|CreateQueue|4xx->2xx|$.message
Issue-Key: sqs|CreateQueue|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: sqs|CreateQueue|4xx->2xx|status
Issue-Key: sqs|GetQueueUrl|4xx->2xx|$.__type
Issue-Key: sqs|GetQueueUrl|4xx->2xx|$.message
Issue-Key: sqs|GetQueueUrl|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: sqs|GetQueueUrl|4xx->2xx|status
Issue-Key: sqs|GetQueueAttributes|2xx->2xx|$.Attributes.ContentBasedDeduplication
Issue-Key: sqs|GetQueueAttributes|2xx->2xx|$.Attributes.Policy
Issue-Key: sqs|GetQueueAttributes|2xx->2xx|$
Issue-Key: sqs|StartMessageMoveTask|2xx->2xx|$.TaskHandle
Issue-Key: sqs|ReceiveMessage|2xx->2xx|$.Messages[*].ReceiptHandle
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# SQS queue-attribute state and response payloads diverge from AWS: unenforced FifoQueue attribute, missing default attributes, wrong Policy version, empty-map envelope and non-AWS opaque handles

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueAttributes

## Description
This issue bundles 7 causes in the SQS queue-attribute store and the response serializers that read it, including two success responses AWS never produces at all. All 7 must be closed before submitting.

Cause 1 of 7 — CreateQueue accepts a FifoQueue attribute AWS refuses, and creates the queue.
AWS: a CreateQueue whose FifoQueue attribute is supplied in a form AWS does not accept is refused with HTTP 400, __type 'com.amazonaws.sqs#InvalidAttributeName', query code 'InvalidAttributeName' and the message 'Unknown Attribute FifoQueue.' — fixed text 'Unknown Attribute ' + the attribute name + '.' (Smithy: CreateQueue binds com.amazonaws.sqs#InvalidAttributeName, httpError 400 defaulted, doc 'The specified attribute doesn't exist.'; the wording is recorded behavior only). AWS treats the attribute as non-existent for that request rather than accepting it, and NO queue is created.
Emulator: answers HTTP 200 with a QueueUrl and creates the queue — no __type, no message, no header.
Why: the emulator silently accepts a request AWS rejects, so a client's queue-type invariant is not enforced; code that relies on AWS refusing the request proceeds against a queue that cannot exist in production, and the resulting state is unreachable on real SQS.
Fix: the FifoQueue attribute branch of the CreateQueue handler's attribute validation must reject rather than accept, before any state is mutated.
Traces: sqs-fifo-queue-type-immutability-and-suffix-rules#task#4.

Cause 2 of 7 — GetQueueUrl resolves a queue that must not exist.
AWS: GetQueueUrl for a name with no queue behind it is refused with HTTP 400, __type 'com.amazonaws.sqs#QueueDoesNotExist', query code 'AWS.SimpleQueueService.NonExistentQueue' and message 'The specified queue does not exist.' (Smithy: GetQueueUrl binds QueueDoesNotExist with awsQueryError code 'AWS.SimpleQueueService.NonExistentQueue', httpResponseCode 400; the message is recorded behavior).
Emulator: answers HTTP 200 and returns a QueueUrl for the name, with no error body at all.
Why: the negative lookup is how clients probe for existence; a 200 here makes a non-existent queue look present, and the error class a caller catches (QueueDoesNotExist) is never raised. Hypothesis: on this fixture the queue exists only because cause 1 wrongly created it, so cause 1 may close this too — but the assertion is independent (GetQueueUrl must raise QueueDoesNotExist for any unknown name) and must be re-replayed on its own.
Fix: the GetQueueUrl handler's name lookup must raise QueueDoesNotExist at 400 whenever the queue store has no entry.
Traces: sqs-fifo-queue-type-immutability-and-suffix-rules#task#5.

Cause 3 of 7 — GetQueueAttributes omits a FIFO queue's defaulted ContentBasedDeduplication.
AWS: for a FIFO queue, ContentBasedDeduplication is always materialised in the attribute set and reads back as the string 'false' when it was never set (Smithy: com.amazonaws.sqs#QueueAttributeName enumerates ContentBasedDeduplication; GetQueueAttributesResult.Attributes is a QueueAttributeMap of string->string. The defaulting to 'false' is recorded behavior).
Emulator: the Attributes map comes back WITHOUT the ContentBasedDeduplication key.
Why: a client reading the attribute to decide whether to send a MessageDeduplicationId gets None instead of 'false' and either crashes on the missing key or takes the wrong branch; 'absent' and 'false' are not interchangeable in SQS's attribute map.
Fix: the queue-creation path in the SQS state must seed the FIFO defaults (ContentBasedDeduplication='false') so GetQueueAttributes serialises them, rather than only storing explicitly-set attributes.
Traces: sqs-fifo-queue-type-immutability-and-suffix-rules#task#9.

Cause 4 of 7 — the generated SQSDefaultPolicy carries the wrong policy language Version.
AWS: when a permission is added to a queue that has no Policy, SQS synthesises an SQSDefaultPolicy whose 'Version' is the literal '2008-10-17' (SQS's long-standing default policy language version), with 'Id' '<queue arn>/SQSDefaultPolicy' and one Statement per added permission. Fixed text: the Version string and the '/SQSDefaultPolicy' Id suffix; variable parts: the queue ARN, the Sid (the caller's Label), the principal, the Action list and the Resource, all taken from the request and the queue's identity. Recorded behavior only (Smithy models Policy as an opaque attribute string).
Emulator: generates an otherwise byte-identical document with 'Version':'2012-10-17'.
Why: callers and policy linters round-trip the document and compare or re-submit it; a Version AWS does not emit for SQSDefaultPolicy makes an equality check against AWS fail and can change how a strict policy parser interprets the statement.
Fix: the default-policy synthesiser used by the AddPermission/RemovePermission path must emit Version '2008-10-17'.
Traces: sqs-queue-permissions-and-tags#task#4 (also sqs-queue-tags-and-permissions-management#task#7).

Cause 5 of 7 — GetQueueAttributes returns an empty Attributes map instead of omitting the member.
AWS: when the request matches no attributes (e.g. after the only attribute was removed), the response body is the EMPTY object {} — the optional Attributes member is omitted entirely rather than serialised as an empty map (Smithy: GetQueueAttributesResult.Attributes is optional, no default).
Emulator: returns {'Attributes': {}}.
Why: idiomatic SDK code tests 'Attributes' in response to decide whether anything came back; an always-present empty map makes that test true and the subsequent lookup fail, inverting the control flow. This is the general rule for optional members on this protocol, not a quirk of one call.
Fix: the GetQueueAttributes output serializer must omit an empty/absent optional map member instead of emitting it.
Traces: sqs-queue-permissions-and-tags#task#11 (also sqs-queue-tags-and-permissions-management#task#9).

Cause 6 of 7 — StartMessageMoveTask returns a TaskHandle that is not AWS-shaped.
AWS: TaskHandle is an opaque, long, base64-ish token that encodes the task's identity (Smithy: StartMessageMoveTaskResult.TaskHandle, String; CancelMessageMoveTaskRequest.TaskHandle, required). Its concrete value is per-run and is NOT a requirement, but its SHAPE is: the recorded expectation is a shaped-not-asserted token, and a bare 36-character UUID does not satisfy it.
Emulator: returns a bare UUID v4 as the whole TaskHandle.
Why: clients and test harnesses that validate or parse the handle's form (length, charset, encoded content) reject the emulator's value, and code paths that assume a handle is not a plain UUID behave differently. Nothing here asserts a specific recorded uuid — only the token's form.
Fix: the TaskHandle minting in the StartMessageMoveTask handler (and the matching parse in CancelMessageMoveTask/ListMessageMoveTasks) must produce AWS's opaque encoded form.
Traces: sqs-dead-letter-queue-and-message-move#task#11 (cluster also covers sqs-dead-letter-queue-and-message-move#task#21 and sqs-message-move-task-lifecycle#task#6).

Cause 7 of 7 — ReceiveMessage returns ReceiptHandles that are not AWS-shaped.
AWS: Message.ReceiptHandle is a single long opaque token (AWS emits a lengthy base64-style string); its value is per-run and not asserted, but its form is. A '<uuid>/<uuid>' pair is not that form.
Emulator: returns '<queue-uuid>/<message-uuid>' — two UUIDs joined by a slash.
Why: the receipt handle is passed verbatim to DeleteMessage/ChangeMessageVisibility, embedded in batch entries and sometimes placed in URLs or logs; a client that validates its form, or that splits on '/' assuming AWS's opaque token, diverges. It also makes emulator handles trivially forgeable, so a test asserting that a handle from another queue is rejected can pass for the wrong reason.
Fix: receipt-handle minting/validation in the SQS message state and the ReceiveMessage serializer must produce and accept AWS's opaque single-token form.
Traces: sqs-fifo-queue-ordering-and-dedup#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (46 of 254 steps mismatched). Verbatim expected -> actual per cause:

Cause 1, CreateQueue, sqs-fifo-queue-type-immutability-and-suffix-rules#task#4:
  status: expected 400, got 200
  $.__type: expected 'com.amazonaws.sqs#InvalidAttributeName', got None
  $.message: expected 'Unknown Attribute FifoQueue.', got None
  $headers.x-amzn-query-error: expected 'InvalidAttributeName;Sender', got None

Cause 2, GetQueueUrl, sqs-fifo-queue-type-immutability-and-suffix-rules#task#5:
  status: expected 400, got 200
  $.__type: expected 'com.amazonaws.sqs#QueueDoesNotExist', got None
  $.message: expected 'The specified queue does not exist.', got None
  $headers.x-amzn-query-error: expected 'AWS.SimpleQueueService.NonExistentQueue;Sender', got None

Cause 3, GetQueueAttributes, sqs-fifo-queue-type-immutability-and-suffix-rules#task#9 (200 vs 200):
  $.Attributes.ContentBasedDeduplication: expected 'false', got None (key absent)

Cause 4, GetQueueAttributes, sqs-queue-permissions-and-tags#task#4 (200 vs 200):
  $.Attributes.Policy expected '{"Version":"2008-10-17","Id":"<queue arn>/SQSDefaultPolicy","Statement":[{...}]}' but got the same document with '"Version":"2012-10-17"'. Every other field (Id, Sid, Effect, Principal, Action, Resource) matched byte for byte, so the sole divergence is the fixed Version string; the ARN, account id, queue name and Sid in the recorded document are per-run values from the request and are not part of the requirement. Cluster of 2 (sqs-queue-tags-and-permissions-management#task#7).

Cause 5, GetQueueAttributes, sqs-queue-permissions-and-tags#task#11 (200 vs 200):
  $: expected {}, got {'Attributes': {}}
  Cluster of 2 (sqs-queue-tags-and-permissions-management#task#9).

Cause 6, StartMessageMoveTask, sqs-dead-letter-queue-and-message-move#task#11 (200 vs 200):
  $.TaskHandle: expected '<shaped-not-asserted>', got 'af23108c-61d2-4c3e-814a-b23712284dd3' — the comparison is against the token's shape, not that literal value; a bare UUID fails it. Cluster of 3 (#task#21, sqs-message-move-task-lifecycle#task#6).

Cause 7, ReceiveMessage, sqs-fifo-queue-ordering-and-dedup#task#6 (200 vs 200):
  $.Messages[0..3].ReceiptHandle: expected '<shaped-not-asserted>', got values of the form '<uuid>/<uuid>' (e.g. 'b82a5fdd-0118-45a8-8cfd-db83b0f5e9e7/6daa7c6e-98c4-427b-abfe-884eb795fafb'). All four messages in the batch fail the same shape check; the uuids themselves are per-run and not the requirement.

Not filed from this replay, and deliberately excluded from this bundle: the two clusters whose only difference is the endpoint host:port inside a returned queue URL (sqs-dead-letter-queue-redrive-configuration#task#7 ListDeadLetterSourceQueues and sqs-queue-tags-and-permissions-management#setup#1 CreateQueue, both expecting 'https://sqs.us-west-2.amazonaws.com/...' and getting the local listener's host). A host and port differ in every environment and are not a requirement.

## Steps to Reproduce
1. Cause 1: CreateQueue supplying the FifoQueue attribute in the form the fixture uses (a non-'true' value / a name without the .fifo suffix) — expect 400 InvalidAttributeName 'Unknown Attribute FifoQueue.' and NO queue created.
2. Cause 2: GetQueueUrl for that same name — expect 400 'AWS.SimpleQueueService.NonExistentQueue' / 'The specified queue does not exist.'
3. Cause 3: CreateQueue a FIFO queue with Attributes={'FifoQueue':'true'} and nothing else, then GetQueueAttributes(AttributeNames=['All']) — the map must contain ContentBasedDeduplication='false'.
4. Cause 4: CreateQueue a queue with no Policy, AddPermission(Label=..., AWSAccountIds=[...], Actions=['SendMessage','ReceiveMessage']), then GetQueueAttributes(AttributeNames=['Policy']) — the synthesised document's Version must be '2008-10-17'.
5. Cause 5: RemovePermission so the queue has no attributes left, then GetQueueAttributes(AttributeNames=['Policy']) — the response body must be {} with no 'Attributes' key.
6. Cause 6: StartMessageMoveTask and inspect TaskHandle — it must be AWS's long opaque token, not a bare UUID.
7. Cause 7: SendMessage then ReceiveMessage and inspect Messages[*].ReceiptHandle — it must be AWS's single opaque token, not '<uuid>/<uuid>'.
8. Re-replay the representative step sqs-fifo-queue-type-immutability-and-suffix-rules#task#4 and every binding in trace_ids; all 7 clusters must pass.
