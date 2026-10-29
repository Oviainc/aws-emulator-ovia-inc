Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-create-queue-validation-rejections
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-create-queue-validation-rejections sqs-dead-letter-queue-and-message-move sqs-message-move-task-errors sqs-message-move-task-lifecycle sqs-dead-letter-queue-configuration sqs-set-queue-attributes-validation-rejections sqs-fifo-queue-type-immutability-and-suffix-rules sqs-receive-and-delete-message-parameter-validation
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|CreateQueue|4xx->4xx|$.message
Issue-Key: sqs|CancelMessageMoveTask|4xx->4xx|$.__type
Issue-Key: sqs|CancelMessageMoveTask|4xx->4xx|$.message
Issue-Key: sqs|CancelMessageMoveTask|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|CancelMessageMoveTask|4xx->4xx|status
Issue-Key: sqs|SetQueueAttributes|4xx->4xx|$.message
Issue-Key: sqs|SetQueueAttributes|4xx->4xx|$.__type
Issue-Key: sqs|ReceiveMessage|4xx->4xx|$.message
Issue-Key: sqs|DeleteMessage|4xx->4xx|$.__type
Issue-Key: sqs|DeleteMessage|4xx->4xx|$.message
Issue-Key: sqs|DeleteMessage|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|DeleteMessage|4xx->4xx|status
Issue-Related: 20261029T180649Z-sqs-awsquerycompatible-error-envelope-is-not-emitted-no.md
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# SQS validation refusals carry the wrong error class, status or Reason wording

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
This issue bundles 7 causes. Each is a refusal the emulator DOES produce but classifies or words differently from AWS; two of them also return the wrong HTTP status. All 7 must be closed before submitting. (The x-amzn-query-error header is missing on several of these responses too; that envelope defect is the separate bundle 'SQS awsQueryCompatible error envelope is not emitted...' filed in this same sweep and should not be fixed here.)

Cause 1 of 7 — CreateQueue with a FIFO-only attribute on a non-FIFO queue.
AWS: refuses with InvalidAttributeName (Smithy: CreateQueue errors include com.amazonaws.sqs#InvalidAttributeName, httpError 400 defaulted, doc 'The specified attribute doesn't exist.') and the message template 'Unknown Attribute <AttributeName>.' — fixed text 'Unknown Attribute ' and the terminating '.', variable part the attribute name taken verbatim from the request's Attributes map. AWS does not treat a FIFO-only attribute on a standard queue as a wrong VALUE; it treats the attribute as non-existent for that queue. Recorded behavior only for the wording (the model carries no message text).
Emulator: right status and error class, wrong message — it answers 'Attribute ContentBasedDeduplication applies only to FIFO queues.', a sentence of documentation prose rather than AWS's template.
Why: the message is the only field that distinguishes one InvalidAttributeName from another, and tools match it; a client matching 'Unknown Attribute ' falls through to a generic path and loses the diagnosis.
Fix: the message template of the attribute-name validation in the CreateQueue handler's attribute validator.
Traces: sqs-create-queue-validation-rejections#task#2.

Cause 2 of 7 — CancelMessageMoveTask against a task that is not running.
AWS: refuses with HTTP 400, __type 'com.amazon.coral.service#InvalidParameterValueException' (query code 'InvalidParameterValue') and the fixed message 'Only active tasks can be cancelled.' — the whole message is fixed text, with no task handle, status or id interpolated. Recorded behavior only; the Smithy model binds CancelMessageMoveTask to InvalidAddress, InvalidSecurity, RequestThrottled, ResourceNotFoundException and UnsupportedOperation and carries no message text.
Emulator: 400 but with __type 'UnsupportedOperation', header 'AWS.SimpleQueueService.UnsupportedOperation;Sender' and the message 'The message movement task that you specified for the TaskHandle parameter is COMPLETED; a message movement can only be cancelled when the current status is RUNNING.' — a different error class and a message that interpolates the task's status.
Why: a caller branching on the error code sees UnsupportedOperation (which in SQS normally means the operation is not supported for that queue type) rather than a parameter-value refusal, so idempotent 'cancel if still running' logic takes the wrong branch and the message never matches.
Fix: the not-running branch of the CancelMessageMoveTask handler must raise the parameter-value refusal with AWS's fixed message instead of UnsupportedOperation.
Traces: sqs-dead-letter-queue-and-message-move#task#13.

Cause 3 of 7 — CancelMessageMoveTask with a TaskHandle that names no task.
AWS: refuses with HTTP 400, __type 'com.amazon.coral.service#InvalidParameterValueException', query code 'InvalidParameterValue', message 'Value for parameter TaskHandle is invalid.' — entirely fixed text; the member name 'TaskHandle' is the modelled input member (Smithy: CancelMessageMoveTaskRequest.TaskHandle, required) and the submitted handle is NOT echoed. An unresolvable handle is a malformed-parameter refusal, not a missing resource.
Emulator: HTTP 404 with __type 'ResourceNotFoundException', header 'ResourceNotFoundException;Sender' and message 'Task does not exist.'
Why: the status itself is wrong (404 vs 400), so SDK retry/error classification and any HTTP-status-based handling diverge before the body is even read; a caller that treats 404 as 'gone' versus 400 as 'bad input' makes the opposite decision.
Fix: the TaskHandle lookup in the CancelMessageMoveTask handler must surface an invalid/unknown handle as the 400 parameter-value refusal rather than mapping it onto ResourceNotFoundException (which the model does bind at 404, but AWS does not use for this case).
Traces: sqs-dead-letter-queue-and-message-move#task#18 (cluster also covers sqs-message-move-task-errors#task#3 and sqs-message-move-task-lifecycle#task#11).

Cause 4 of 7 — SetQueueAttributes with a RedrivePolicy that is not a JSON map.
AWS: refuses with the two-part message 'Invalid value for the parameter RedrivePolicy. Reason: Redrive policy is not a valid JSON map.' — fixed text is the prefix 'Invalid value for the parameter ' + the attribute name + '. Reason: ' and the Reason sentence 'Redrive policy is not a valid JSON map.'; the variable part is the attribute name. The Reason clause is the discriminator between the different RedrivePolicy defects. Recorded behavior only (Smithy models the refusal class, not its text).
Emulator: emits only the prefix — 'Invalid value for the parameter RedrivePolicy.' — dropping the whole ' Reason: ...' clause.
Why: without the Reason clause every RedrivePolicy refusal looks identical, so a caller cannot tell a malformed document from a bad deadLetterTargetArn or a bad maxReceiveCount, which is precisely the information AWS puts there. Related but NOT covering issues already open: 20261028T014019Z-sqs-cross-region-redrivepolicy-is-refused-with-the.md (region rule wording) and 20261029T101522Z-sqs-refuses-a-queue-type-mismatched-redrivepolicy-with.md (queue-type rule wording); neither mentions the not-a-valid-JSON-map Reason or the clause being omitted entirely.
Fix: the RedrivePolicy parse step of the shared SetQueueAttributes/CreateQueue attribute validator must attach its Reason clause to the message.
Traces: sqs-dead-letter-queue-configuration#task#9 (also sqs-set-queue-attributes-validation-rejections#task#2).

Cause 5 of 7 — SetQueueAttributes attempting to change a queue's type.
AWS: refuses with InvalidAttributeValue (Smithy: com.amazonaws.sqs#InvalidAttributeValue, httpError 400 defaulted, doc 'A queue attribute value is invalid.'), __type 'com.amazonaws.sqs#InvalidAttributeValue', query code 'InvalidAttributeValue', message 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.' — fixed text as in cause 4 plus the Reason sentence 'Modifying queue type is not supported.'; the variable part is the attribute name.
Emulator: answers InvalidAttributeValue's sibling instead — __type 'InvalidAttributeName' with the message 'Attribute FifoQueue can be provided only when the queue is created.'
Why: AWS distinguishes 'this attribute does not exist' (InvalidAttributeName) from 'this value is not permitted' (InvalidAttributeValue); immutability of the queue type is the latter. A client branching on the class mis-files the failure, and the Reason text it would match is absent.
Fix: the FifoQueue immutability check in the SetQueueAttributes handler must raise InvalidAttributeValue with AWS's Reason clause.
Traces: sqs-fifo-queue-type-immutability-and-suffix-rules#task#7.

Cause 6 of 7 — ReceiveMessage with an out-of-range VisibilityTimeout.
AWS: refuses with 400 InvalidParameterValue and the message template 'Value <submitted value> for parameter VisibilityTimeout is invalid. Reason: Must be between 0 and 43200, if provided.' — fixed text is 'Value ', ' for parameter VisibilityTimeout is invalid. Reason: Must be between 0 and 43200, if provided.'; the only variable part is the value echoed from the request (Smithy: ReceiveMessageRequest.VisibilityTimeout, NullableInteger; the 0..43200 bound is recorded behavior, the model states no range).
Emulator: same status and code, but words the Reason as 'Must be >= 0 and <= 43200, if provided.' instead of 'Must be between 0 and 43200, if provided.'
Why: the Reason sentence is fixed text a caller may match; the emulator's paraphrase never matches AWS's, and an emulator-trained matcher fails against AWS.
Fix: the VisibilityTimeout range-check message template in ReceiveMessage's input validation (the bound itself is already correct).
Traces: sqs-receive-and-delete-message-parameter-validation#task#5.

Cause 7 of 7 — DeleteMessage with the required ReceiptHandle absent/empty.
AWS: a missing required parameter is refused BEFORE any receipt-handle lookup: HTTP 400, __type 'com.amazon.coral.service#MissingRequiredParameterException', query code 'MissingParameter', message 'The request must contain the parameter <MemberName>.' — fixed text 'The request must contain the parameter ' plus the terminating '.', variable part the modelled member name (Smithy: DeleteMessageRequest.ReceiptHandle, required: true).
Emulator: skips the required-member check and goes straight to the handle lookup, answering HTTP 404 with __type 'ReceiptHandleIsInvalid', header 'ReceiptHandleIsInvalid;Sender' and message 'The input receipt handle is invalid.' (Smithy does bind ReceiptHandleIsInvalid at httpError 404 with awsQueryError ReceiptHandleIsInvalid — but that is the error for a handle that is present and unusable, not for an absent one.)
Why: wrong status (404 vs 400) and wrong class for the commonest client bug there is — a programming error that AWS reports as a missing parameter is reported as a server-side 'not found', so validation wrappers and tests that assert MissingParameter on an empty argument diverge.
Fix: required-member validation for the DeleteMessage input must run before the handler's state lookup; the same ordering applies to every required member of the operation.
Traces: sqs-receive-and-delete-message-parameter-validation#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (46 of 254 steps mismatched). Verbatim expected -> actual per cause:

Cause 1, CreateQueue, sqs-create-queue-validation-rejections#task#2 (expected 400, actual 400):
  $.message: 'Unknown Attribute ContentBasedDeduplication.' -> 'Attribute ContentBasedDeduplication applies only to FIFO queues.'
  (header 'InvalidAttributeName;Sender' also missing — covered by the envelope bundle.)

Cause 2, CancelMessageMoveTask, sqs-dead-letter-queue-and-message-move#task#13 (expected 400, actual 400):
  $.__type: 'com.amazon.coral.service#InvalidParameterValueException' -> 'UnsupportedOperation'
  $.message: 'Only active tasks can be cancelled.' -> 'The message movement task that you specified for the TaskHandle parameter is COMPLETED; a message movement can only be cancelled when the current status is RUNNING.'
  $headers.x-amzn-query-error: 'InvalidParameterValue;Sender' -> 'AWS.SimpleQueueService.UnsupportedOperation;Sender'

Cause 3, CancelMessageMoveTask, sqs-dead-letter-queue-and-message-move#task#18 (expected 400, actual 404):
  status: 400 -> 404
  $.__type: 'com.amazon.coral.service#InvalidParameterValueException' -> 'ResourceNotFoundException'
  $.message: 'Value for parameter TaskHandle is invalid.' -> 'Task does not exist.'
  $headers.x-amzn-query-error: 'InvalidParameterValue;Sender' -> 'ResourceNotFoundException;Sender'
  Cluster of 3 steps (sqs-message-move-task-errors#task#3, sqs-message-move-task-lifecycle#task#11).

Cause 4, SetQueueAttributes, sqs-dead-letter-queue-configuration#task#9 (expected 400, actual 400):
  $.message: 'Invalid value for the parameter RedrivePolicy. Reason: Redrive policy is not a valid JSON map.' -> 'Invalid value for the parameter RedrivePolicy.'
  Cluster of 2 (sqs-set-queue-attributes-validation-rejections#task#2).

Cause 5, SetQueueAttributes, sqs-fifo-queue-type-immutability-and-suffix-rules#task#7 (expected 400, actual 400):
  $.__type: 'com.amazonaws.sqs#InvalidAttributeValue' -> 'InvalidAttributeName'
  $.message: 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.' -> 'Attribute FifoQueue can be provided only when the queue is created.'

Cause 6, ReceiveMessage, sqs-receive-and-delete-message-parameter-validation#task#5 (expected 400, actual 400):
  $.message: 'Value -1 for parameter VisibilityTimeout is invalid. Reason: Must be between 0 and 43200, if provided.' -> 'Value -1 for parameter VisibilityTimeout is invalid. Reason: Must be >= 0 and <= 43200, if provided.'
  The '-1' is the value echoed from the request and is not part of the template; the divergence is entirely in the fixed Reason sentence.

Cause 7, DeleteMessage, sqs-receive-and-delete-message-parameter-validation#task#7 (expected 400, actual 404):
  status: 400 -> 404
  $.__type: 'com.amazon.coral.service#MissingRequiredParameterException' -> 'ReceiptHandleIsInvalid'
  $.message: 'The request must contain the parameter ReceiptHandle.' -> 'The input receipt handle is invalid.'
  $headers.x-amzn-query-error: 'MissingParameter;Sender' -> 'ReceiptHandleIsInvalid;Sender'

No per-run value (request id, timestamp, generated id, host) participates in any of these assertions: every diverging field is either an HTTP status, an error class, or fixed message text with at most a request-supplied value interpolated.

## Steps to Reproduce
1. Cause 1: CreateQueue with a plain (non-.fifo) queue name and Attributes={'ContentBasedDeduplication': 'false'} — expect 400 InvalidAttributeName 'Unknown Attribute ContentBasedDeduplication.'
2. Cause 2: StartMessageMoveTask, let it finish, then CancelMessageMoveTask with that TaskHandle — expect 400 InvalidParameterValue 'Only active tasks can be cancelled.'
3. Cause 3: CancelMessageMoveTask with a TaskHandle that names no task — expect 400 InvalidParameterValue 'Value for parameter TaskHandle is invalid.', not 404 ResourceNotFoundException.
4. Cause 4: SetQueueAttributes with Attributes={'RedrivePolicy': '<not a JSON map>'} — expect the message to end with '. Reason: Redrive policy is not a valid JSON map.'
5. Cause 5: CreateQueue a FIFO queue, then SetQueueAttributes on it with Attributes={'FifoQueue': ...} — expect 400 InvalidAttributeValue 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.'
6. Cause 6: ReceiveMessage with VisibilityTimeout=-1 — expect the Reason 'Must be between 0 and 43200, if provided.'
7. Cause 7: DeleteMessage with ReceiptHandle absent or empty — expect 400 MissingParameter 'The request must contain the parameter ReceiptHandle.', not 404 ReceiptHandleIsInvalid.
8. Re-replay the representative step sqs-create-queue-validation-rejections#task#2 and every binding in trace_ids; all 7 clusters must pass.
