Issue-Source: trace-sweep-divergence
Issue-Trace: connect-queue-crud-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connect-queue-crud-validation
Issue-Shape-Id: com.amazonaws.connect#AmazonConnectService
Issue-Key: connect|CreateQueue|4xx->4xx|$.Message
Issue-Key: connect|CreateQueue|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|DescribeQueue|4xx->4xx|$.Message
Issue-Key: connect|DescribeQueue|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|DescribeQueue|4xx->4xx|status
Issue-Key: connect|DeleteQueue|4xx->4xx|$.Message
Issue-Commit: 11742957972effc3a277f1c1e061799d0d90da1b

# connect queue CRUD refusals diverge: instance/queue not-found and parameter validation use emulator-authored messages, an unmodeled ValidationException envelope and the wrong status class

> _Note: this issue was identified against the code base at commit `11742957972effc3a277f1c1e061799d0d90da1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeQueue

## Description
Replay of the Amazon Connect queue CRUD plane (CreateQueue / DescribeQueue / DeleteQueue) diverges on SIX distinct root causes. They share one fault family: the emulator's queue handlers refuse requests with a generic framework error envelope (an unmodeled ValidationException code and an empty Message) or with an emulator-composed not-found sentence that interpolates the requested InstanceId, and in one case they resolve state before validating input so the refusal lands in the wrong HTTP status class. ALL SIX must be closed before this issue is resubmitted.

Cause 1 of 6 (fixture connect-queue-crud-validation, step task#1, CreateQueue): a QueueName longer than the modeled maximum is refused with an unmodeled ValidationException carrying no Message.
AWS: Smithy: CreateQueue http PUT /queues/{InstanceId}, success 200; input member Name is @required targeting CommonNameLength127 with @length min 1 max 127. The modeled client faults are InvalidParameterException (httpError 400, doc "One or more of the specified parameters are not valid", member Message doc "The message about the parameters"), InvalidRequestException (400), DuplicateResourceException (409), ResourceNotFoundException (404), LimitExceededException (429), ThrottlingException (429). ValidationException is NOT modeled on this operation. Recorded response: HTTP 400 with Message "The value for QueueName must be less than or equal to 127". In that sentence the fixed text is "The value for ", " must be less than or equal to "; the variable parts are the rejected parameter name (QueueName) and the violated @length max bound (127), both taken from the request and the model — never from one recording's literal values.
Emulator: HTTP 400 (correct class) but the body carries no Message member at all (expected the sentence above, got None) and the response header x-amzn-errortype is "ValidationException".
Why: an SDK deserializes a modeled error by x-amzn-errortype, so the client never receives InvalidParameterException and its generated error-handling branch for that shape never runs — the refusal falls through to a generic failure. With no Message, the operator-facing text is empty, so the caller cannot tell which parameter was rejected or what the limit is.
Fix: the CreateQueue input validation plus the connect error serializer — raise the service's modeled InvalidParameterException with its Message populated and the matching x-amzn-errortype, not the framework-wide ValidationException.

Cause 2 of 6 (fixture connect-queue-crud-validation, step task#2, CreateQueue): the not-found refusal for an unknown InstanceId is emulator-authored and echoes the requested id.
AWS: Smithy: CreateQueue's InstanceId is @required, httpLabel, target InstanceId @length min 1 max 100; the modeled 404 is ResourceNotFoundException (doc "The specified resource was not found", member Message doc "The message about the resource"). Recorded response for an InstanceId that resolves to nothing: HTTP 404 with Message "Resource not found" — that sentence is entirely FIXED text with NO variable parts: it does not name the resource kind and does not quote the id.
Emulator: HTTP 404 (correct status) but Message "Instance <requested-instance-id> not found", a sentence the emulator composes by interpolating the request's InstanceId.
Why: the error message is a requirement of the error class, so a client that asserts on, logs or routes by AWS's refusal text sees a message AWS never emits; the id echo additionally leaks request input into a reply AWS keeps opaque.
Fix: the CreateQueue not-found path / the connect error message templates — emit the fixed "Resource not found" sentence with no interpolation.

Cause 3 of 6 (fixture connect-queue-crud-validation, step task#3, DescribeQueue): the same emulator-authored not-found sentence on the read path.
AWS: Smithy: DescribeQueue http GET /queues/{InstanceId}/{QueueId}, both labels @required; modeled 404 is ResourceNotFoundException. Recorded response for an unresolvable instance: HTTP 404 with Message "Resource not found" — fixed text, no variable parts.
Emulator: HTTP 404 with Message "Instance <requested-instance-id> not found".
Why: as in cause 2 — the refusal text is not AWS's, and it leaks the requested id. The defect is duplicated per handler, so fixing CreateQueue alone leaves DescribeQueue diverging.
Fix: the DescribeQueue not-found path / the shared connect not-found message template.

Cause 4 of 6 (fixture connect-queue-crud-validation, step task#4, DescribeQueue; same cluster also covers task#8): an invalid QueueId is resolved against state before it is validated, so the refusal has the wrong status class, the wrong error shape and the wrong message.
AWS: Smithy: DescribeQueue.QueueId is @required and httpLabel; an invalid queue identifier fails input validation before any lookup, and the modeled fault is InvalidParameterException (httpError 400). Recorded response: HTTP 400 with Message "Invalid parameter: QueueId" — fixed text "Invalid parameter: " plus one variable part, the offending member name (QueueId) taken from the request.
Emulator: HTTP 404 with x-amzn-errortype "ResourceNotFoundException" and Message "Instance <requested-instance-id> not found" — i.e. it performs the instance state lookup first and reports a missing resource.
Why: the fault is reclassified from a 400 client input error to a 404 missing resource. A caller retries, or treats the queue/instance as deleted, instead of fixing its request, and status-class-driven logic (retry/alarm on 404 vs fail-fast on 400) takes the wrong branch. The resource verdict also masks the request fault, so the caller is never told which parameter was invalid.
Fix: validation ordering in the DescribeQueue handler — enforce the identifier constraints on the path labels before any state resolution, and return the modeled InvalidParameterException with the "Invalid parameter: <member>" message.

Cause 5 of 6 (fixture connect-queue-crud-validation, step task#6, DeleteQueue): the delete path repeats the emulator-authored not-found sentence.
AWS: Smithy: DeleteQueue http DELETE /queues/{InstanceId}/{QueueId}, output Unit; modeled errors include ResourceNotFoundException (404) and InvalidParameterException (400). Recorded response for an unresolvable instance: HTTP 404 with Message "Resource not found" — fixed text, no variable parts.
Emulator: HTTP 404 with Message "Instance <requested-instance-id> not found".
Why: as in causes 2 and 3; a delete-then-verify client keyed on AWS's refusal text mis-handles the refusal, and the id is echoed where AWS stays opaque.
Fix: the DeleteQueue not-found path / the shared connect not-found message template.

Cause 6 of 6 (fixture connect-queue-crud-validation, step task#7, DescribeQueue): an invalid InstanceId is refused with an empty-bodied, unmodeled ValidationException instead of InvalidParameterException.
AWS: Smithy: DescribeQueue.InstanceId is @required, httpLabel, target InstanceId @length min 1 max 100; the modeled 400 is InvalidParameterException. Recorded response: HTTP 400 with Message "Invalid parameter: InstanceId" — fixed text "Invalid parameter: " plus the offending member name (InstanceId) from the request.
Emulator: HTTP 400 with no Message member at all (expected the sentence above, got None) and x-amzn-errortype "ValidationException", a shape Amazon Connect does not model on this operation.
Why: the client receives an unmodeled error code and an empty message, so it cannot distinguish a malformed-identifier rejection from any other validation failure, and the modeled InvalidParameterException branch of its generated code never runs.
Fix: DescribeQueue input validation plus the connect error serializer — classify an invalid InstanceId label as InvalidParameterException with the "Invalid parameter: <member>" message and the matching x-amzn-errortype.

No open issue covers this surface. The closest open connect issue, 20261103T120437Z-connect-instance-plane-5-net-new-causes-listinstances.md, is the same fault FAMILY on the instance plane (ListInstances / DescribeInstance) and owns those replay keys; it does not cover CreateQueue / DescribeQueue / DeleteQueue, and the replay reports no owner for any of the six clusters above. The other open connect issues are stubs for different operations (connect-listtagsforresource-stub, connect-updateinstanceattribute-stub, connect-updateattachedfilesconfiguration-stub) and the "Queue"-named issues in the backlog all concern Amazon SQS, Batch, MediaConvert, PCS or Deadline queues, not Amazon Connect queues.

## Evidence
Replay of fixture connect-queue-crud-validation (trace set simparity-wire-aws-v1), part of a 16-step corpus run: 1 passed, 15 mismatched, 11 divergence clusters overall, score 6.2%. Five clusters are owned by 20261103T120437Z-connect-instance-plane-5-net-new-causes-listinstances.md; the six below carry an empty owned_by and an empty stub_path, so they are net-new.

1) task#1 CreateQueue — status 400/400 but $.Message missing (expected 'The value for QueueName must be less than or equal to 127', got None); $headers.x-amzn-errortype shape_mismatch (got 'ValidationException', a shape connect does not model on CreateQueue).
2) task#2 CreateQueue — status 404/404 but $.Message value_mismatch: expected 'Resource not found', got 'Instance d6f2a9c1-5e3b-4a7d-9c21-8f4e6b2d1a70 not found' (the uuid is this run's value; the defect is the interpolated sentence).
3) task#3 DescribeQueue — status 404/404 but $.Message value_mismatch: expected 'Resource not found', got 'Instance <id> not found'.
4) task#4 DescribeQueue (cluster also covers task#8) — expected status 400, got 404; $.Message value_mismatch (expected 'Invalid parameter: QueueId', got 'Instance <id> not found'); $headers.x-amzn-errortype got 'ResourceNotFoundException'.
5) task#6 DeleteQueue — status 404/404 but $.Message value_mismatch: expected 'Resource not found', got 'Instance <id> not found'.
6) task#7 DescribeQueue — status 400/400 but $.Message missing (expected 'Invalid parameter: InstanceId', got None); $headers.x-amzn-errortype got 'ValidationException'.

Model evidence (services/service-com-amazonaws-connect-amazonconnectservice/smithy-model/com-amazonaws-connect-amazonconnectservice.json): CreateQueue PUT /queues/{InstanceId} -> 200, Name @required target CommonNameLength127 @length min 1 max 127, InstanceId @required httpLabel @length min 1 max 100, errors DuplicateResourceException(409)/InternalServiceException(500)/InvalidParameterException(400)/InvalidRequestException(400)/LimitExceededException(429)/ResourceNotFoundException(404)/ThrottlingException(429). DescribeQueue GET /queues/{InstanceId}/{QueueId} with both labels @required, errors InternalServiceException(500)/InvalidParameterException(400)/InvalidRequestException(400)/ResourceNotFoundException(404)/ThrottlingException(429). DeleteQueue DELETE /queues/{InstanceId}/{QueueId}, errors add ResourceInUseException(409). InvalidParameterException = client error, httpError 400, single member Message ('The message about the parameters'); ResourceNotFoundException = client error, httpError 404, single member Message ('The message about the resource'). No queue operation models ValidationException.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay wire fixture connect-queue-crud-validation from trace set simparity-wire-aws-v1.
3. Step task#1: PUT /queues/{InstanceId} (CreateQueue) with a Name longer than the @length max of 127 -> emulator answers 400 with an empty body and x-amzn-errortype ValidationException; AWS answers 400 InvalidParameterException with Message 'The value for QueueName must be less than or equal to 127'.
4. Step task#2: PUT /queues/{unknown-instance-id} (CreateQueue) -> emulator answers 404 with Message 'Instance <id> not found'; AWS answers 404 ResourceNotFoundException with Message 'Resource not found'.
5. Step task#3: GET /queues/{unknown-instance-id}/{queue-id} (DescribeQueue) -> same divergence as step 4.
6. Step task#4 (and task#8): GET /queues/{instance-id}/{invalid-queue-id} (DescribeQueue) -> emulator answers 404 ResourceNotFoundException 'Instance <id> not found'; AWS validates the identifier first and answers 400 InvalidParameterException 'Invalid parameter: QueueId'.
7. Step task#6: DELETE /queues/{unknown-instance-id}/{queue-id} (DeleteQueue) -> emulator answers 404 'Instance <id> not found'; AWS answers 404 ResourceNotFoundException 'Resource not found'.
8. Step task#7: GET /queues/{invalid-instance-id}/{queue-id} (DescribeQueue) -> emulator answers 400 with no Message and x-amzn-errortype ValidationException; AWS answers 400 InvalidParameterException 'Invalid parameter: InstanceId'.
