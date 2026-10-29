Issue-Source: trace-sweep-divergence
Issue-Trace: oam-sink-quota-and-request-validation-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: oam-sink-quota-and-request-validation-errors
Issue-Shape-Id: com.amazonaws.oam#oamservice
Issue-Key: oam|CreateLink|4xx->4xx|$.message
Issue-Key: oam|CreateLink|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# oam CreateLink missing-member rejection uses wrong error type and message

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLink

## Description
This issue has 1 cause.

Cause 1 of 1 - CreateLink with a required body member omitted/null is rejected with the wrong error code and the wrong message (fixture oam-sink-quota-and-request-validation-errors, step task#3).
AWS: CreateLink's LabelTemplate, ResourceTypes and SinkIdentifier are all @required (Smithy: CreateLinkInput.LabelTemplate required, @length 1..64; ResourceTypes required, @length 1..50, uniqueItems; SinkIdentifier required, pattern ^[a-zA-Z0-9][a-zA-Z0-9_:\.\-\/]{0,2047}$). A request whose JSON body omits a required property is rejected by oam's request-body validation with HTTP 400 and a body carrying the member `message` (lower-case, per InvalidParameterException/MissingRequiredParameterException, both of which model `message` plus amznErrorType on header x-amzn-ErrorType). The recorded message is 'Invalid request body: [object has missing required properties (["LabelTemplate"])]': its fixed text is "Invalid request body: [object has missing required properties ([...])]" and the only variable part is the quoted list of the required member names actually missing from the request (here LabelTemplate) - that part is taken from the request, not from this recording. The recorded x-amzn-errortype header is a different error name than the emulator's; the recording asserts its shape rather than its literal value, so the emulator's value is wrong for this error class.
Emulator: returns HTTP 400 (status is correct) but with message "Value null at 'labelTemplate' failed to satisfy constraint: Member must not be null" - an EC2/Query-style constraint-violation sentence naming a lower-camel member - and x-amzn-errortype: MissingRequiredParameterException, whose shape does not match the recorded error type.
Why: a client that branches on the error code or parses the message to learn which properties are missing gets neither: the code identifies a different error class than AWS raises for a malformed request body, and the message is in a different format with a different member spelling ('labelTemplate' instead of the modeled 'LabelTemplate'), so error handling, retry classification and user-facing diagnostics all diverge.
Fix: the request-body validation layer in front of the oam CreateLink handler (and its error serializer) - missing required JSON properties must produce AWS's body-validation error code and the 'Invalid request body: [object has missing required properties ([...])]' message listing the modeled member names.
Traces: oam-sink-quota-and-request-validation-errors#task#3. No open oam issue covers this.

## Evidence
Fixture oam-sink-quota-and-request-validation-errors, step task#3, CreateLink: status 400 == 400, but
$.message value_mismatch: expected 'Invalid request body: [object has missing required properties (["LabelTemplate"])]', got "Value null at 'labelTemplate' failed to satisfy constraint: Member must not be null".
$headers.x-amzn-errortype shape_mismatch: emulator sent 'MissingRequiredParameterException', which does not match the recorded error type's shape.
Smithy (com.amazonaws.oam): CreateLinkInput.LabelTemplate @required @length(1,64); CreateLink errors = ConflictException 409, InternalServiceFault 500, InvalidParameterException 400, MissingRequiredParameterException 400, ServiceQuotaExceededException 429; error message member is `message` with amznErrorType bound to header x-amzn-ErrorType.

## Steps to Reproduce
1. Replay fixture oam-sink-quota-and-request-validation-errors against the emulator.
2. Step task#3: POST /CreateLink with a body that omits (or nulls) the required LabelTemplate property while supplying ResourceTypes and SinkIdentifier.
3. Observe HTTP 400 with message "Value null at 'labelTemplate' failed to satisfy constraint: Member must not be null" and x-amzn-errortype: MissingRequiredParameterException, instead of the body-validation message 'Invalid request body: [object has missing required properties (["LabelTemplate"])]' and AWS's error type for a malformed request body.
