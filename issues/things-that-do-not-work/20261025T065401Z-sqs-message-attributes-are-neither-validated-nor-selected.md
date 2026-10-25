Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.message-attribute-name-rules-rejected sqs.message-attribute-type-and-number-value-validated sqs.message-attribute-value-in-the-wrong-field-for-its-data-type-rejected sqs.empty-vs-absent-message-attribute-preserved sqs.receive-message-attribute-name-wildcard-selects-by-name-prefix sqs.message-digests-recomputed-from-the-documented-canonical-encoding sqs.message-size-counts-attribute-name-type-and-value sqs.message-system-attribute-other-than-aws-trace-header-rejected sqs.receive-message-attribute-selectivity
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.message-attribute-name-rules-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.message-attribute-type-and-number-value-validated|_error#5|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.message-attribute-value-in-the-wrong-field-for-its-data-type-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.empty-vs-absent-message-attribute-preserved|_client_error#4|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: sqs.receive-message-attribute-name-wildcard-selects-by-name-prefix|run#6|AssertionError|assert got == expected, (
Issue-Key: sqs|SendMessage|ValidationException
Issue-Key: sqs.message-system-attribute-other-than-aws-trace-header-rejected|run#7|AssertionError|assert (code, message) == ("InvalidParameterValue", expected), (
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS message attributes are neither validated nor selected per AWS rules, and MessageSystemAttributes (AWSTraceHeader) is refused outright by the emulator (5 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Smithy: SendMessage carries MessageAttributes (MessageBodyAttributeMap) and MessageSystemAttributes (MessageBodySystemAttributeMap), and models InvalidMessageContents (400) for a payload AWS refuses; MD5OfMessageAttributes / MD5OfMessageSystemAttributes are modelled output members, so the canonical encoding they digest is part of the contract.

Cause 1 of 5: message-attribute NAME rules are not enforced. AWS: a name that is empty, over 256 characters, starts with 'AWS.'/'Amazon.' (reserved), starts or ends with a period, holds consecutive periods, or holds a character outside [A-Za-z0-9_.-] is refused with InvalidParameterValue, 400, and nothing is enqueued. Emulator: SendMessage returns 200 with a MessageId and MD5OfMessageAttributes. Why: a producer whose attribute names AWS rejects at the edge enqueues silently in the emulator and fails only in production. Fix: attribute-name validation in the SendMessage handler's validation. Traces: sqs.message-attribute-name-rules-rejected.

Cause 2 of 5: DataType and value are not validated. AWS: an unsupported DataType, a Number whose StringValue is not numeric, and a custom type with an empty label are each refused with InvalidParameterValue, 400. Emulator: accepted, 200. Fix: the DataType/value validation in the SendMessage handler. Traces: sqs.message-attribute-type-and-number-value-validated.

Cause 3 of 5: value carried in the wrong field for its DataType is accepted. AWS: a String/Number attribute whose value is supplied in BinaryValue (or a Binary attribute supplied in StringValue) is refused with InvalidParameterValue, 400. Emulator: accepted, 200, and an MD5OfMessageAttributes is even computed over it. Fix: the same validation path. Traces: sqs.message-attribute-value-in-the-wrong-field-for-its-data-type-rejected.

Cause 4 of 5: the empty-vs-absent attribute distinction. AWS: the call this check makes is refused; the emulator accepted it (200 with MD5OfMessageAttributes 'be43507a132350f74fcb2f674c545de5'), so an empty attribute value is stored where AWS has no such message. Fix: the same validation path. Traces: sqs.empty-vs-absent-message-attribute-preserved.

Cause 5 of 5: MessageSystemAttributes is refused instead of accepted, and the attribute-name selector is a plain prefix match. AWS: SendMessage with MessageSystemAttributes={'AWSTraceHeader': {...}} is ACCEPTED, the system attribute counts toward the message size, MD5OfMessageSystemAttributes is returned, the attribute comes back on ReceiveMessage when selected, and a system attribute OTHER than AWSTraceHeader is refused with InvalidParameterValue, 400. Separately, ReceiveMessage with MessageAttributeNames=['order.*'] selects every attribute whose name begins with 'order' — including 'orderly'. Emulator: ANY MessageSystemAttributes value is refused with code 'ValidationException' and the message 'The request includes MessageSystemAttributes, which this emulator does not support yet: it carries the X-Ray trace header, which nothing here propagates.' — so the accepted case, the digest, the size accounting and the one genuinely-invalid case are all wrong; and 'order.*' returned only ['order.id','order.total'], dropping 'orderly'. Why: X-Ray tracing through SQS cannot be emulated at all, four distinct behaviours collapse into one refusal with an unmodelled code, and a wildcard selector silently under-delivers attributes. Fix: implement MessageSystemAttributes handling (accept AWSTraceHeader, refuse other keys with InvalidParameterValue, include it in the size accounting and in MD5OfMessageSystemAttributes) in the SendMessage handler, and fix the prefix semantics of MessageAttributeNames in the ReceiveMessage handler. Traces: sqs.message-digests-recomputed-from-the-documented-canonical-encoding, sqs.message-size-counts-attribute-name-type-and-value, sqs.message-system-attribute-other-than-aws-trace-header-rejected, sqs.receive-message-attribute-selectivity, sqs.receive-message-attribute-name-wildcard-selects-by-name-prefix.

## Evidence
Expected (AWS / Smithy): invalid attribute names, DataTypes, numeric values and wrong-field values are refused with InvalidParameterValue at 400; MessageSystemAttributes={'AWSTraceHeader': ...} is accepted and digested into MD5OfMessageSystemAttributes while any other system-attribute key is refused InvalidParameterValue; MessageAttributeNames=['order.*'] selects every attribute whose name starts with 'order'.
Actual (emulator replay): 'AssertionError: expected the call to be refused, but it succeeded: {MD5OfMessageBody: ..., MD5OfMessageAttributes: 5e95783c4dd235530ef8fb70530189c2, MessageId: ..., HTTPStatusCode: 200}' (name rules, DataType/number, wrong field, empty value); 'botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the SendMessage operation: The request includes MessageSystemAttributes, which this emulator does not support yet: it carries the X-Ray trace header, which nothing here propagates.'; "trace-header-not-counted: a message totalling exactly 1024 counted bytes must be accepted, got 'ValidationException'"; "SendMessage with system attribute 'traceparent': got 'ValidationException'" where InvalidParameterValue was required; "MessageAttributeNames=['order.*'] returned attributes ['order.id', 'order.total'], expected ['order.id', 'order.total', 'orderly']".

## Steps to Reproduce
1. CreateQueue, then SendMessage with MessageAttributes={'AWS.Reserved': {'DataType':'String','StringValue':'x'}} -> 200 instead of InvalidParameterValue.
2. SendMessage with MessageAttributes={'n': {'DataType':'Number','StringValue':'not-a-number'}} -> 200.
3. SendMessage with a String-typed attribute supplied via BinaryValue -> 200.
4. SendMessage with MessageSystemAttributes={'AWSTraceHeader': {'DataType':'String','StringValue':'Root=1-...'}} -> refused 'ValidationException'; AWS accepts and returns MD5OfMessageSystemAttributes.
5. SendMessage with MessageSystemAttributes={'traceparent': ...} -> the same 'ValidationException' instead of InvalidParameterValue.
6. Send a message carrying attributes 'order.id', 'order.total', 'orderly'; ReceiveMessage with MessageAttributeNames=['order.*'] -> 'orderly' is missing.
7. Re-replay the check ids listed in trace_ids.
