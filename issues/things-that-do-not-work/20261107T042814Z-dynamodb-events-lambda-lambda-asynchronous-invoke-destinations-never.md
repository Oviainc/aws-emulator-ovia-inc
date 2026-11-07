Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive dynamodb__events__lambda.lambda-destination-eventbridge-missing-permission-rejected
Issue-Key: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive|_receive_one#23|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive|_receive_one#22|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda|PutFunctionEventInvokeConfig|InvalidParameterValueException
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# dynamodb__events__lambda: Lambda asynchronous-invoke destinations never publish the Invocation Result envelope, and the PutFunctionEventInvokeConfig permission refusal is misworded (2 causes)

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
Two net-new root causes in the Lambda asynchronous-invoke destination surface. Neither was reported as owned by an open issue.

=== Cause 1 of 2: an asynchronous invoke with DestinationConfig pointing at an EventBridge bus publishes nothing. Failing checks: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive, dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive ===

AWS: Smithy (lambda, PutFunctionEventInvokeConfig): PUT /2019-09-25/functions/{FunctionName}/event-invoke-config accepts MaximumRetryAttempts (range 0..2), MaximumEventAgeInSeconds (range 60..21600) and DestinationConfig{OnSuccess,OnFailure}, each a Destination ARN — an EventBridge event-bus ARN is a supported destination. For an Invoke with InvocationType=Event, Lambda publishes the invocation result to that destination as an EventBridge envelope with `source` = `lambda` and `detail-type` = `Lambda Function Invocation Result - Success` (OnSuccess) or `Lambda Function Invocation Result - Failure` (OnFailure). Its `detail` carries `requestContext` (functionArn, condition, requestId, approximateInvokeCount), `requestPayload` (the exact payload the caller passed) and `responsePayload` (the handler's return value, or for a fault the {errorMessage,errorType,stackTrace} record). With MaximumRetryAttempts=0 and a handler that raises, `requestContext.condition` is `RetriesExhausted` and responsePayload.errorMessage carries the raised message. recorded behavior only for the exact detail body beyond those members.

Emulator: nothing is ever published to the destination bus. A rule on {"source":["lambda"]} with an SQS target stays empty after an InvocationType=Event invoke, and both checks exhaust the harness budget: `Check timed out after 60.0s`. The PutFunctionEventInvokeConfig call itself is accepted, so the configuration is stored but never acted on.

Why: the standard async-Lambda result-routing pattern is inert — a client gets no success record, no failure record and no RetriesExhausted signal, so an asynchronous invocation's outcome is completely unobservable and any downstream remediation or audit step never runs.

Fix: the asynchronous invoke path must dispatch the stored event-invoke-config destination on terminal success/failure (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the function event-invoke-config state it reads).

Traces: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive, dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive.

=== Cause 2 of 2: the InvalidParameterValueException message for a destination the execution role cannot reach is rewritten. Failing check: dynamodb__events__lambda.lambda-destination-eventbridge-missing-permission-rejected ===

AWS: Smithy (lambda, PutFunctionEventInvokeConfig) models InvalidParameterValueException with httpError 400. When the function's execution role lacks events:PutEvents on the DestinationConfig bus, the call is refused 400 InvalidParameterValueException with the message `The function execution role does not have permissions to call PutEvents on <destination ARN>` — fixed text `The function execution role does not have permissions to call PutEvents on `, with the variable part being the destination event-bus ARN taken from the request's DestinationConfig.

Emulator: the HTTP status and the error code are right (400 InvalidParameterValueException), but the message is `The provided execution role does not have permissions to call PutEvents on EventBridge`. Two divergences in one string: the fixed text opens `The provided execution role ...` instead of `The function execution role ...`, and the variable destination ARN is replaced by the literal word `EventBridge`, so the message no longer names which destination was unreachable.

Why: a client that matches on the refusal text, or that parses the destination ARN out of it to report which destination its role must be granted, gets the wrong answer; with several destinations configured the message is ambiguous.

Fix: the refusal message in PutFunctionEventInvokeConfig's destination-permission validation (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/put_function_event_invoke_config.rs).

Traces: dynamodb__events__lambda.lambda-destination-eventbridge-missing-permission-rejected.

## Evidence
Cause 1 — expected (AWS): after PutFunctionEventInvokeConfig(MaximumRetryAttempts=0, DestinationConfig={"OnFailure":{"Destination":<bus arn>}}) and Invoke(InvocationType="Event") of a raising handler, a rule on source `lambda` delivers an envelope with source=`lambda`, detail-type=`Lambda Function Invocation Result - Failure`, detail.requestPayload == the invoke payload, detail.requestContext.condition == `RetriesExhausted`, detail.requestContext.functionArn containing the function name, and detail.responsePayload.errorMessage carrying the raised message. Symmetrically for OnSuccess / `... - Success`.
Actual (emulator): `Error: Check timed out after 60.0s` for both checks — the SQS target behind the rule never receives any message.

Cause 2 — expected (AWS): `An error occurred (InvalidParameterValueException) ... The function execution role does not have permissions to call PutEvents on <bus arn>`.
Actual (emulator): `AssertionError: An error occurred (InvalidParameterValueException) when calling the PutFunctionEventInvokeConfig operation: The provided execution role does not have permissions to call PutEvents on EventBridge`.
Fixed text that diverges: `The provided execution role does not have permissions to call PutEvents on ` (AWS: `The function execution role does not have permissions to call PutEvents on `). Value that is missing: the destination bus ARN from the request, replaced by the literal `EventBridge`.

## Steps to Reproduce
Cause 1:
1. CreateEventBus, PutRule on {"source":["lambda"]} with an SQS target and a permissive queue policy.
2. CreateFunction with a role granted events:PutEvents on that bus; handler either returns a value (OnSuccess) or raises (OnFailure).
3. PutFunctionEventInvokeConfig(MaximumRetryAttempts=0, DestinationConfig={"OnSuccess"|"OnFailure":{"Destination": <bus arn>}}).
4. Invoke(InvocationType="Event") with a marker payload and poll the queue. Nothing ever arrives.

Cause 2:
1. CreateFunction with an execution role granted ONLY logs:* (no events:PutEvents).
2. PutFunctionEventInvokeConfig(DestinationConfig={"OnSuccess":{"Destination": <bus arn>}}).
3. Read the 400 InvalidParameterValueException message and compare it to AWS's wording, which must name the destination ARN.
4. Re-replay the three checks in trace_ids to confirm.
