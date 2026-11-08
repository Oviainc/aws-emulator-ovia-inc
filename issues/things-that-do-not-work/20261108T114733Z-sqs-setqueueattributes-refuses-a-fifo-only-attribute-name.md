Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-set-queue-attributes-validation-rejections
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-set-queue-attributes-validation-rejections
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|SetQueueAttributes|4xx->4xx|$headers.x-amzn-query-error
Issue-Related: 20261029T180649Z-sqs-validation-refusals-carry-the-wrong-error-class.md
Issue-Commit: 0435459e5768db99e0b30c9823c1aeff0587590f

# sqs: SetQueueAttributes refuses a FIFO-only attribute name as InvalidAttributeValue instead of InvalidAttributeName

> _Note: this issue was identified against the code base at commit `0435459e5768db99e0b30c9823c1aeff0587590f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
1 net-new WIRE root cause from the replay of trace set simparity-wire-aws-v1. The replay report marked this cluster with NO stub_path and an EMPTY owned_by, i.e. no open issue carries its replay key. The closest open issue is the BEHAVIORAL 20261108T104640Z-sqs-setqueueattributes-naming-fifoqueue-on-a-standard-queue.md (Issue-Corpus: simparity-behavioral-aws-v1), which describes the same classification fault from a behavioral check id but carries no Issue-Trace/Issue-Step, so the wire step below is not gated by it; this entry supplies that wire binding. Also note the FIXED issue 20261029T180649Z-sqs-queue-attribute-state-and-response-payloads-diverge.md, whose scope was CreateQueue and which therefore does not reach SetQueueAttributes.

--- Cause 1 of 1: a FIFO-only (create-only) attribute is classified as a known-but-immutable VALUE rather than as a NAME that does not exist for this operation ---
AWS: SetQueueAttributes binds both error shapes. Smithy: SetQueueAttributes errors {InvalidAddress 404 (awsQueryError InvalidAddress), InvalidAttributeName 400 [httpError defaulted], InvalidAttributeValue 400 [httpError defaulted], InvalidSecurity 403, OverLimit 403, QueueDoesNotExist 400 (awsQueryError AWS.SimpleQueueService.NonExistentQueue), RequestThrottled 403, UnsupportedOperation 400 (awsQueryError AWS.SimpleQueueService.UnsupportedOperation)}; input Attributes targets QueueAttributeMap and is required. For a FIFO-only attribute named on a queue for which it is not settable, AWS answers the NAME error, not the VALUE error: HTTP 400, __type com.amazonaws.sqs#InvalidAttributeName, header x-amzn-query-error 'InvalidAttributeName;Sender', message 'Unknown Attribute <AttributeName>.' — the fixed text is "Unknown Attribute " plus the trailing ".", and the variable part is the attribute name exactly as the request's Attributes map spelled it (for the recorded step, FifoQueue). The error shape, HTTP status and query-error code are modeled; the message template is recorded behavior only. The emulator already produces exactly this shape for ContentBasedDeduplication in the same recorded request sequence, so the defect is confined to the create-only/queue-type branch.
Emulator: answers HTTP 400 (status matches) but with __type 'com.amazonaws.sqs#InvalidAttributeValue', header x-amzn-query-error 'InvalidAttributeValue;Sender', and the emulator-authored message 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.' Here 'Invalid value for the parameter ' and 'Reason: Modifying queue type is not supported.' are wording AWS does not emit for this call at all; only the attribute name inside it comes from the request.
Why: clients branch on the error code, not the sentence. Code that catches InvalidAttributeName to mean "this attribute is not settable here" — the idiomatic guard, and the one AWS raises for every FIFO-only attribute on a standard queue — does not catch InvalidAttributeValue, so the exception escapes the caller's handler. Conversely a handler written for InvalidAttributeValue (a bad VALUE, normally retried with a corrected value) is entered for a request whose value was never the problem, producing an infinite corrective retry. Error-mapping tables and message assertions keyed on AWS's 'Unknown Attribute <name>.' template also fail.
Fix: the create-only/FIFO-only branch of SetQueueAttributes' attribute validation in the SQS crate — the state-level attribute validator that currently classifies a create-only attribute as a VALUE refusal, as consumed by the SetQueueAttributes handler's refusal-to-error mapping. Classify an attribute that is not settable on this operation/queue type as a NAME refusal carrying 'Unknown Attribute ' + the requested attribute name + '.', so it serializes as InvalidAttributeName with HTTP 400 and x-amzn-query-error 'InvalidAttributeName;Sender' — the same shape the handler already emits for ContentBasedDeduplication. The queue's stored type must remain unchanged, as it already does.
Traces: sqs-set-queue-attributes-validation-rejections#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator: 27 fixtures, 270 steps, 261 passed, 9 mismatched, score 96.7%, 7 divergence clusters (all service sqs).

Divergence cluster (verdict mismatch, stub_path empty, owned_by empty):
  service=sqs | operation=SetQueueAttributes | fixture sqs-set-queue-attributes-validation-rejections | step task#2
  divergence_key: service=sqs|SetQueueAttributes|status=400->400|keys=$.__type,$.message,$headers.x-amzn-query-error
  expected_status 400, actual_status 400 (status agrees)
  $.__type: expected 'com.amazonaws.sqs#InvalidAttributeName', got 'com.amazonaws.sqs#InvalidAttributeValue'
  $.message: expected 'Unknown Attribute FifoQueue.', got 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.'
  $headers.x-amzn-query-error: expected 'InvalidAttributeName;Sender', got 'InvalidAttributeValue;Sender'
  affected_count 1 (representative is the only member)

Message anatomy: in the expected message 'Unknown Attribute ' and the trailing '.' are AWS's fixed text; 'FifoQueue' is the attribute name taken verbatim from the request's Attributes map, so the requirement is the template, not that literal name. The emulator's sentence is not an AWS template for any input.

Smithy evidence (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): SetQueueAttributes binds InvalidAttributeName (client error, httpError 400, defaulted) and InvalidAttributeValue (client error, httpError 400, defaulted) as distinct error shapes, so the two are separately observable error classes a client may dispatch on.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from that checkout.
2. replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Replay fixture 'sqs-set-queue-attributes-validation-rejections', step 'task#2' — a SetQueueAttributes whose Attributes map names the FIFO-only attribute FifoQueue on a standard queue.
4. Expected (AWS / recorded): HTTP 400, body $.__type 'com.amazonaws.sqs#InvalidAttributeName', $.message 'Unknown Attribute FifoQueue.', response header x-amzn-query-error 'InvalidAttributeName;Sender'.
5. Actual (emulator): HTTP 400, $.__type 'com.amazonaws.sqs#InvalidAttributeValue', $.message 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.', x-amzn-query-error 'InvalidAttributeValue;Sender'.
6. Control in the same fixture: a SetQueueAttributes naming ContentBasedDeduplication on the same standard queue already returns InvalidAttributeName with 'Unknown Attribute ContentBasedDeduplication.' and passes, confirming the serializer is capable of the correct shape and the fault is in the create-only classification branch.
7. After the refusal, a GetQueueAttributes(All) on the queue must still carry no FifoQueue key (unchanged behavior).
