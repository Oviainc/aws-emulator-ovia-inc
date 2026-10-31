Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-batch-send-receive-delete-and-purge
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-batch-send-receive-delete-and-purge sqs-fifo-queue-ordering-and-dedup sqs-message-send-receive-delete sqs-create-queue-validation-rejections sqs-dead-letter-queue-redrive-configuration sqs-fifo-queue-type-immutability-and-suffix-rules sqs-queue-lifecycle-core sqs-message-move-task-lifecycle sqs-queue-lifecycle-basic sqs-set-queue-attributes-validation-rejections sqs-queue-lifecycle-crud sqs-receive-and-delete-message-parameter-validation sqs-send-message-fifo-and-validation-rejections sqs-single-message-round-trip
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|ChangeMessageVisibility|4xx->4xx|$.__type
Issue-Key: sqs|ChangeMessageVisibility|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|CreateQueue|4xx->4xx|$.__type
Issue-Key: sqs|CreateQueue|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|StartMessageMoveTask|4xx->4xx|$.__type
Issue-Key: sqs|StartMessageMoveTask|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|SetQueueAttributes|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|ReceiveMessage|4xx->4xx|$.__type
Issue-Key: sqs|ReceiveMessage|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|SendMessage|4xx->4xx|$.__type
Issue-Key: sqs|SendMessage|4xx->4xx|$headers.x-amzn-query-error
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# SQS awsQueryCompatible error envelope is not emitted: no x-amzn-query-error header and unqualified __type on error responses

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeMessageVisibility

## Description
This issue bundles 9 causes, all of them the same defect in one component: the SQS error serializer does not produce the awsQueryCompatible error envelope. Every cause must be closed before submitting.

Shared AWS contract (Smithy: com.amazonaws.sqs#AmazonSQS, version 2012-11-05, protocols ['awsJson1_0'], protocol_modifiers ['awsQueryCompatible']). Because the service carries @awsQueryCompatible, EVERY error response AWS returns on the JSON protocol carries two things a client needs:
  (a) the HTTP response header 'x-amzn-query-error', whose value is '<QueryErrorCode>;<Fault>' where Fault is 'Sender' for client errors and 'Receiver' for server errors. The QueryErrorCode is the error shape's @awsQueryError code where the model gives one (e.g. QueueDoesNotExist -> 'AWS.SimpleQueueService.NonExistentQueue', UnsupportedOperation -> 'AWS.SimpleQueueService.UnsupportedOperation', ResourceNotFoundException -> 'ResourceNotFoundException'), and the error shape's own name where the model defaults the binding (InvalidAttributeName, InvalidAttributeValue, InvalidMessageContents, InvalidIdFormat all have httpErrorDefaulted 400 and no awsQueryError). For the framework validation errors that are not modelled on the service (parameter validation refusals), the codes are 'InvalidParameterValue' and 'MissingParameter' — recorded behavior only, taken from the recorded 400 responses.
  (b) the body member '__type' as a FULLY QUALIFIED shape id. For a modelled SQS error that is 'com.amazonaws.sqs#<ShapeName>'; for the Coral framework validation errors the recorded responses show 'com.amazon.coral.service#InvalidParameterValueException' and 'com.amazon.coral.service#MissingRequiredParameterException'.

Shared Emulator behavior: the status code and (in most causes) the message are right, but the emulator never sets 'x-amzn-query-error' at all, and where a framework validation error is involved it puts the BARE query code into '__type' instead of the qualified Coral shape id.

Shared Why: the awsQueryCompatible header is exactly how a client still speaking the legacy Query error vocabulary (older SDK major versions, the AWS CLI's query-compat path, and every SDK's retry/error classifier for SQS) recovers the error code. With the header absent, such a client cannot name the error at all and degrades it to a generic 400, so none of the per-error branches (retry on throttle, surface InvalidAttributeName to the user, etc.) can fire. '__type' is the modern SDK's discriminator: an unqualified value does not match the generated 'com.amazon.coral.service#...' / 'com.amazonaws.sqs#...' key, so the SDK raises an unmodelled generic error instead of the typed exception, and `except ClientError as e: e.response['Error']['Code']` sees a value AWS never sends.

Shared Fix: the SQS crate's error-response serializer (the single place that renders an error into status + body + headers), not the individual operation handlers: it must emit the 'x-amzn-query-error: <code>;<Sender|Receiver>' header for every error it renders, and render '__type' as the fully qualified shape id. Per-handler work is only needed where a handler picks the wrong error shape, which is a separate open issue (see the companion bundle 'SQS validation refusals carry the wrong error class...' filed in this same sweep).

Cause 1 of 9 — ChangeMessageVisibility parameter refusal. AWS: 400 with __type 'com.amazon.coral.service#InvalidParameterValueException' and header 'InvalidParameterValue;Sender'. Emulator: 400 with __type 'InvalidParameterValue' and no header. Why/Fix: as shared above. Traces: sqs-batch-send-receive-delete-and-purge#task#6 (cluster also covers sqs-fifo-queue-ordering-and-dedup#task#8 and sqs-message-send-receive-delete#task#5).

Cause 2 of 9 — CreateQueue parameter refusal. AWS: 400, __type 'com.amazon.coral.service#InvalidParameterValueException', header 'InvalidParameterValue;Sender'. Emulator: __type 'InvalidParameterValue', header absent. Traces: sqs-create-queue-validation-rejections#task#1 (cluster of 6, also sqs-dead-letter-queue-redrive-configuration#task#8, sqs-fifo-queue-ordering-and-dedup#task#9, sqs-fifo-queue-type-immutability-and-suffix-rules#task#2/#3/#6).

Cause 3 of 9 — CreateQueue attribute-value refusal. AWS: 400 with header 'InvalidAttributeValue;Sender' (Smithy: CreateQueue errors include InvalidAttributeValue, httpError 400 defaulted). Emulator: correct status, code and message, header absent. Traces: sqs-create-queue-validation-rejections#task#3 (also sqs-queue-lifecycle-core#task#8).

Cause 4 of 9 — StartMessageMoveTask parameter refusal. AWS: 400, __type 'com.amazon.coral.service#InvalidParameterValueException', header 'InvalidParameterValue;Sender'. Emulator: __type 'InvalidParameterValue', header absent. Traces: sqs-message-move-task-lifecycle#task#9.

Cause 5 of 9 — SetQueueAttributes attribute-value refusal. AWS: 400 with header 'InvalidAttributeValue;Sender' (Smithy: SetQueueAttributes errors include InvalidAttributeValue). Emulator: header absent. Traces: sqs-queue-lifecycle-basic#task#8 (also sqs-set-queue-attributes-validation-rejections#task#3 and #task#4).

Cause 6 of 9 — GetQueueAttributes unknown-attribute refusal. AWS: 400 with header 'InvalidAttributeName;Sender' (Smithy: GetQueueAttributes errors include InvalidAttributeName). Emulator: header absent. Traces: sqs-queue-lifecycle-crud#task#9.

Cause 7 of 9 — ReceiveMessage parameter refusal. AWS: 400, __type 'com.amazon.coral.service#InvalidParameterValueException', header 'InvalidParameterValue;Sender'. Emulator: __type 'InvalidParameterValue', header absent. Traces: sqs-receive-and-delete-message-parameter-validation#task#3 (also #task#4).

Cause 8 of 9 — SendMessage missing-required-parameter refusal. AWS: 400, __type 'com.amazon.coral.service#MissingRequiredParameterException', header 'MissingParameter;Sender'. Emulator: __type 'MissingParameter', header absent. Traces: sqs-send-message-fifo-and-validation-rejections#task#5 (cluster of 4, also #task#6, #task#7, #task#8).

Cause 9 of 9 — SendMessage invalid message contents. AWS: 400 with header 'InvalidMessageContents;Sender' (Smithy: SendMessage errors include InvalidMessageContents, httpError 400 defaulted). Emulator: correct status/code/message, header absent. Traces: sqs-single-message-round-trip#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1: 46 of 254 steps mismatched; these 9 clusters cover 23 of those steps and all report verdict 'mismatch' with the expected and actual HTTP status EQUAL — only the error envelope diverges.

Missing header, verbatim from the divergence details (expected -> actual):
- 'x-amzn-query-error': 'InvalidParameterValue;Sender' -> None (ChangeMessageVisibility, CreateQueue, StartMessageMoveTask, ReceiveMessage clusters)
- 'x-amzn-query-error': 'MissingParameter;Sender' -> None (SendMessage cluster)
- 'x-amzn-query-error': 'InvalidAttributeValue;Sender' -> None (CreateQueue and SetQueueAttributes clusters)
- 'x-amzn-query-error': 'InvalidAttributeName;Sender' -> None (GetQueueAttributes cluster)
- 'x-amzn-query-error': 'InvalidMessageContents;Sender' -> None (SendMessage cluster)
In every one of those the actual value is literally absent, not merely different.

Wrong __type, verbatim:
- $.__type expected 'com.amazon.coral.service#InvalidParameterValueException', got 'InvalidParameterValue'
- $.__type expected 'com.amazon.coral.service#MissingRequiredParameterException', got 'MissingParameter'
The emulator's value is the query error CODE, not a shape id; the required value is the fully qualified Coral shape id. The error messages themselves matched in these clusters, so the defect is confined to the envelope (header + __type), which is why one serializer change closes all nine.

Nothing environment-specific is asserted here: no host, port, id, timestamp or request id takes part in either the header or the __type value — both are fixed text derived from the error class and the Sender/Receiver fault, so they are reproducible in any environment.

## Steps to Reproduce
1. Point an SQS client at the emulator's data listener and make any call that AWS refuses as a client error — e.g. SendMessage without MessageBody, ReceiveMessage with VisibilityTimeout=-1, CreateQueue with an attribute value AWS rejects, GetQueueAttributes with an unknown attribute name.
2. Inspect the RAW HTTP response headers. AWS returns 'x-amzn-query-error: <code>;Sender' (e.g. 'MissingParameter;Sender', 'InvalidParameterValue;Sender', 'InvalidAttributeName;Sender', 'InvalidAttributeValue;Sender', 'InvalidMessageContents;Sender'); the emulator sends no such header.
3. Inspect the JSON body's '__type'. AWS returns a fully qualified shape id ('com.amazon.coral.service#InvalidParameterValueException', 'com.amazon.coral.service#MissingRequiredParameterException', or 'com.amazonaws.sqs#<ShapeName>' for a modelled SQS error); the emulator returns the bare query code.
4. Confirm the status code and message are unaffected — only the envelope diverges.
5. Re-replay the representative fixture step sqs-batch-send-receive-delete-and-purge#task#6 plus every binding listed in trace_ids; all 9 clusters (23 steps) must pass.
