Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-fifo-queue-type-immutability-and-suffix-rules
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-fifo-queue-type-immutability-and-suffix-rules
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|SetQueueAttributes|4xx->4xx|$headers.x-amzn-query-error
Issue-Related: 20261029T180649Z-sqs-validation-refusals-carry-the-wrong-error-class.md
Issue-Commit: e4ba669456b5e24b120a6ca32a894b2be68408fa

# SQS SetQueueAttributes queue-type immutability refusal uses InvalidAttributeName instead of InvalidAttributeValue

> _Note: this issue was identified against the code base at commit `e4ba669456b5e24b120a6ca32a894b2be68408fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
This issue carries 1 cause. It must be closed before submitting.

Cause 1 of 1 - SetQueueAttributes that attempts to change an existing queue's type (setting the FifoQueue attribute on a queue that already exists).

AWS: refuses the request with HTTP 400 and the error class InvalidAttributeValue. Smithy (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): SetQueueAttributes binds BOTH com.amazonaws.sqs#InvalidAttributeName (httpError 400 defaulted, doc 'The specified attribute doesn't exist.') and com.amazonaws.sqs#InvalidAttributeValue (httpError 400 defaulted, doc 'A queue attribute value is invalid.'); SetQueueAttributesRequest.Attributes is a required QueueAttributeMap. The two are distinct, separately modelled classes: an attribute the service does not know is InvalidAttributeName, an attribute whose supplied value is not permitted is InvalidAttributeValue. Queue type is a well-known, settable-at-create attribute, so rejecting a change to it is a VALUE refusal. The wire fields AWS sends are __type 'com.amazonaws.sqs#InvalidAttributeValue', header x-amzn-query-error 'InvalidAttributeValue;Sender', and (recorded behavior only - the model carries no message text) the message 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.' In that message the fixed text is the prefix 'Invalid value for the parameter ', the separator '. Reason: ' and the Reason sentence 'Modifying queue type is not supported.'; the only variable part is the attribute name, taken verbatim from the key in the request's Attributes map (here 'FifoQueue'). No queue name, URL, id or timestamp appears in it.

Emulator: answers HTTP 400 (correct) but with the sibling class - __type 'InvalidAttributeName', header x-amzn-query-error 'InvalidAttributeName;Sender' - and an emulator-authored message, 'Attribute FifoQueue can be provided only when the queue is created.', which follows neither AWS's 'Invalid value for the parameter <Attribute>. Reason: ...' template nor its Reason sentence.

Why: a client branching on the modelled error class sees 'the specified attribute doesn't exist' where AWS says 'this value is not permitted'. Code that treats InvalidAttributeName as a signal to drop the unknown attribute and retry will strip FifoQueue and silently succeed against the emulator while failing against AWS, and code that asserts InvalidAttributeValue for immutability never matches. The message is the only field distinguishing one attribute refusal from another, so a caller matching the fixed Reason text 'Modifying queue type is not supported.' falls through to a generic failure path and loses the diagnosis.

Fix: the FifoQueue immutability check in the SetQueueAttributes handler's attribute validator must raise InvalidAttributeValue (with the matching awsQueryError code in the error envelope) and render AWS's 'Invalid value for the parameter <AttributeName>. Reason: Modifying queue type is not supported.' template, instead of raising InvalidAttributeName with its own prose. The status is already correct and must stay 400.

Related but NOT covering: the open issue 20261029T180649Z-sqs-validation-refusals-carry-the-wrong-error-class.md describes this same refusal as its 'Cause 5 of 7', but its replay pointer binds only the representative step sqs-create-queue-validation-rejections#task#2 and its Issue-Trace-Ids list fixture ids with no step component, so this cluster (sqs-fifo-queue-type-immutability-and-suffix-rules#task#7) is not bound by it and replay reports the step unowned. This issue carries the exact fixture#step binding so the fix is gated on it.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 254 steps, 233 passed, 21 mismatched, score 91.7%). Cluster divergence_key 'service=sqs|SetQueueAttributes|status=400->400|keys=$.__type,$.message,$headers.x-amzn-query-error', owned_by empty (no open issue carries this replay key), stub_path empty.

Fixture sqs-fifo-queue-type-immutability-and-suffix-rules, step task#7, SetQueueAttributes - expected status 400, actual status 400. Verbatim expected -> actual:
  $.__type: 'com.amazonaws.sqs#InvalidAttributeValue' -> 'InvalidAttributeName'
  $.message: 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.' -> 'Attribute FifoQueue can be provided only when the queue is created.'
  $headers.x-amzn-query-error: 'InvalidAttributeValue;Sender' -> 'InvalidAttributeName;Sender'

affected_count 1 (this cluster has no further members).

In the expected message, 'FifoQueue' is the attribute name supplied by the request and is the template's only variable part; everything else - 'Invalid value for the parameter ', '. Reason: ' and 'Modifying queue type is not supported.' - is fixed text. No per-run value (request id, host, port, generated queue id, clock time) participates in any of the three diverging fields, so none of this is environment-specific.

## Steps to Reproduce
1. CreateQueue a FIFO queue (name ending in '.fifo', Attributes={'FifoQueue':'true'}).
2. Call SetQueueAttributes on that queue's URL with Attributes={'FifoQueue': <a value that changes the queue type>}.
3. Observe the refusal: the emulator returns 400 with __type 'InvalidAttributeName', x-amzn-query-error 'InvalidAttributeName;Sender' and the message 'Attribute FifoQueue can be provided only when the queue is created.'
4. AWS returns 400 with __type 'com.amazonaws.sqs#InvalidAttributeValue', x-amzn-query-error 'InvalidAttributeValue;Sender' and the message 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.'
5. Re-replay fixture sqs-fifo-queue-type-immutability-and-suffix-rules step task#7 to confirm the fix; the cluster must pass.
