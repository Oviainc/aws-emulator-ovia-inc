Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.query-and-json-protocol-clients-share-one-queue sqs.query-protocol-refusals-are-xml-error-responses-rejected sqs.raw-json-post-missing-target-or-mismatched-content-type-rejected sqs.sse-rejects-anonymous-sendmessage-that-an-unencrypted-queue-accepts sqs.create-queue-after-set-queue-attributes-compares-current-values sqs.send-message-malformed-awstraceheader-accepted-then-dropped
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.query-and-json-protocol-clients-share-one-queue|call#6|ParseError|return resp.status_code, ET.fromstring(resp.content)
Issue-Key: sqs.query-protocol-refusals-are-xml-error-responses-rejected|call#6|ParseError|return resp.status_code, ET.fromstring(resp.content)
Issue-Key: sqs.raw-json-post-missing-target-or-mismatched-content-type-rejected|run#7|AssertionError|assert payload_a == {"__type": "com.amazon.coral.service#UnknownOperationException"}, (
Issue-Key: sqs|SendMessage|UnknownService
Issue-Key: sqs|CreateQueue|QueueAlreadyExists
Issue-Key: sqs|SendMessage|InvalidParameterValue
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 5 net-new causes: the Query protocol does not answer XML, the raw-JSON refusal is wrong, anonymous SendMessage hits an unregistered service, CreateQueue compares stale attributes and the AWSTraceHeader is refused

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Five distinct root causes in the sqs suite (protocol and request-handling layer). Each is listed with its own failing check ids; all five must be closed.

Cause 1 of 5 — the AWS Query protocol does not produce XML responses. AWS: SQS still serves the AWS Query protocol (form-encoded Action=... POST): successes and refusals come back as XML documents (<SendMessageResponse>..., <ErrorResponse><Error><Code>...</Code><Message>...</Message></Error>...), with content-type text/xml, and a Query client and a JSON client can share one queue. Emulator: parsing the response raises xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0 — the body is not XML at all (the JSON-protocol body is returned instead). Why: every Query-protocol SDK (older AWS SDKs, hand-rolled signers, many language runtimes' default SQS clients) cannot read any response or any error from the emulator. Fix: the SQS protocol layer's Query-protocol response and error serializers. Traces: sqs.query-and-json-protocol-clients-share-one-queue, sqs.query-protocol-refusals-are-xml-error-responses-rejected.

Cause 2 of 5 — a raw JSON POST without X-Amz-Target is refused with the wrong body. AWS: a json-1.0 request missing X-Amz-Target (or with a mismatched content-type) is refused by the protocol layer with the error AWS's SQS endpoint returns for an unresolvable target — not a bespoke sentence. Emulator: the body is {'__type': 'SerializationException', 'message': 'missing required header `x-amz-target`'} — an emulator-authored message with a backticked header name. Why: a client's protocol-level error handling sees a message no AWS SDK produces, so the condition is unclassifiable. Fix: the SQS JSON-protocol request router's missing/mismatched-target refusal. Traces: sqs.raw-json-post-missing-target-or-mismatched-content-type-rejected.

Cause 3 of 5 — an anonymous (unsigned) SendMessage is dispatched to an unregistered service name. AWS: an anonymous SendMessage against an SSE-KMS queue is refused by SQS itself (an authorization refusal from the SQS endpoint), while the same anonymous call to an unencrypted queue with a permissive policy succeeds. Emulator: botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the SendMessage operation: no emulated service is registered for 'amazonsqs' — the request is routed by the Smithy service-shape name rather than the endpoint/sdkId, so no handler is found. Why: the client gets a non-AWS error code for SendMessage, and the SSE-KMS authorization behaviour cannot be exercised at all. Fix: the request-routing/service-registry lookup for unsigned requests (the same defect shape as 'awsevents' for PutEvents). Traces: sqs.sse-rejects-anonymous-sendmessage-that-an-unencrypted-queue-accepts.

Cause 4 of 5 — CreateQueue compares the request's attributes against the queue's ORIGINAL create values, not its current ones. AWS: CreateQueue is idempotent against the queue's CURRENT configuration: after SetQueueAttributes changes VisibilityTimeout, a CreateQueue naming the same queue with the NEW value succeeds and returns the existing QueueUrl; QueueAlreadyExists (Smithy QueueNameExists, awsQueryError 'QueueAlreadyExists', HTTP 400) is reserved for a request whose attributes differ from the current ones. Emulator: the call is refused with QueueAlreadyExists 'A queue already exists with the same name and a different value for attribute VisibilityTimeout' even though the request matches the live value. Why: idempotent create-or-get — the pattern every IaC and bootstrap script uses — breaks permanently as soon as a queue's attributes are ever changed. Fix: the CreateQueue handler's attribute comparison (compare against current state). Traces: sqs.create-queue-after-set-queue-attributes-compares-current-values.

Cause 5 of 5 — a malformed AWSTraceHeader is refused instead of accepted and dropped. AWS: SendMessage accepts a MessageSystemAttribute AWSTraceHeader whose value is malformed and simply does not propagate it (the message is sent). Smithy: SendMessage declares InvalidMessageContents and the KMS/queue errors; there is no modeled refusal for a malformed trace header. Emulator: An error occurred (InvalidParameterValue) when calling the SendMessage operation: Message system attribute 'AWSTraceHeader' must contain a non-empty value of type 'String'. Why: a producer whose tracing library emits an odd header has its messages rejected outright rather than merely untraced — a send that AWS accepts becomes a hard failure. Fix: MessageSystemAttributes validation in the SendMessage handler. Traces: sqs.send-message-malformed-awstraceheader-accepted-then-dropped.

## Evidence
Cause 1: xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0 (both query-protocol checks)
Cause 2: AssertionError: missing X-Amz-Target gave body {'__type': 'SerializationException', 'message': 'missing required header `x-amz-target`'}
Cause 3: botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the SendMessage operation: no emulated service is registered for 'amazonsqs'
Cause 4: botocore.errorfactory.QueueNameExists: An error occurred (QueueAlreadyExists) when calling the CreateQueue operation: A queue already exists with the same name and a different value for attribute VisibilityTimeout
Cause 5: botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the SendMessage operation: Message system attribute 'AWSTraceHeader' must contain a non-empty value of type 'String'.

## Steps to Reproduce
Cause 1: POST a form-encoded Action=SendMessage request (and one that must be refused) to the SQS endpoint and parse the body as XML.
Cause 2: POST application/x-amz-json-1.0 to the SQS endpoint without X-Amz-Target; read the error body.
Cause 3: SendMessage anonymously (unsigned) to an SSE-KMS queue, then to an unencrypted queue with a permissive policy.
Cause 4: CreateQueue(name, VisibilityTimeout=30); SetQueueAttributes VisibilityTimeout=60; CreateQueue(name, VisibilityTimeout=60) — expect the existing QueueUrl.
Cause 5: SendMessage with MessageSystemAttributes={'AWSTraceHeader': {'DataType':'String','StringValue':'<malformed>'}} — expect success and no propagation.
