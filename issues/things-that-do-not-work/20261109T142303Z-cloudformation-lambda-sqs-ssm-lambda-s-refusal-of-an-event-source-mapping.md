Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs__ssm.cfn-sqs-event-source-mapping-create-rejects-a-role-without-receive-permission
Issue-Key: cloudformation__lambda__sqs__ssm.cfn-sqs-event-source-mapping-create-rejects-a-role-without-receive-permission|run#23|AssertionError|assert _REASON.fullmatch(mapping_failures[0]), (
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# Lambda's refusal of an event-source-mapping role lacking sqs:ReceiveMessage drops CloudFormation's "Invalid request provided: " prefix

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the CREATE_FAILED reason recorded for an AWS::Lambda::EventSourceMapping whose function execution role cannot ReceiveMessage omits the leading `Invalid request provided: ` that AWS's handler message carries.

AWS: CreateEventSourceMapping for an SQS source whose function execution role lacks sqs:ReceiveMessage on the queue is refused 400 InvalidParameterValueException, and CloudFormation records the resource's CREATE_FAILED ResourceStatusReason as:
  Resource handler returned message: "Invalid request provided: The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)
Fixed text: `Resource handler returned message: "Invalid request provided: The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: `, `) (SDK Attempt Count: `, `)" (RequestToken: `, `, HandlerErrorCode: InvalidRequest)`. Variable parts: the two request-id/token uuids and the attempt count — per-run values, not requirements. Smithy (lambda): CreateEventSourceMapping errors include InvalidParameterValueException (400); the `Invalid request provided: ` prefix is the handler's rendering of that error class, recorded behavior.

Emulator: the same resource's CREATE_FAILED reason is `Resource handler returned message: "The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: 1)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)` — identical except that the fixed prefix `Invalid request provided: ` is missing.

Why: a client that matches CloudFormation failure reasons to classify deployment failures (a very common CI/CD and remediation pattern, and exactly what the handler-error contract is for) does not match this reason, so a well-understood permission error is reported as an unclassified failure.

Fix: the AWS::Lambda::EventSourceMapping resource handler's error rendering — the InvalidParameterValueException from CreateEventSourceMapping must be rendered with the `Invalid request provided: ` prefix that the handler prepends for that error class.

Traces: check cloudformation__lambda__sqs__ssm.cfn-sqs-event-source-mapping-create-rejects-a-role-without-receive-permission.

Related but not covering: open issue 20261106T153504Z-cloudformation-lambda-sqs-ssm-a-sam-sqs-event.md notes a dropped "Inval..." prefix for a SAM-generated mapping; this check declares the mapping directly in the template and is a separate failing check with its own replay key.

## Evidence
Expected CREATE_FAILED ResourceStatusReason (regex over the fixed text, uuids/attempt-count variable):
  Resource handler returned message: "Invalid request provided: The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)

Actual:
  Resource handler returned message: "The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: 5678d781-2fdc-926a-db42-4ee5a0b2fb2a) (SDK Attempt Count: 1)" (RequestToken: 216709e1-cbd3-be09-6c9d-920c3230122c, HandlerErrorCode: InvalidRequest)

Difference: the fixed text `Invalid request provided: ` is absent. Everything else matches; the uuids and attempt count are per-run values.

## Steps to Reproduce
1. CreateStack a template with an SQS queue, a Lambda function whose execution role grants no sqs:ReceiveMessage on that queue, and an AWS::Lambda::EventSourceMapping binding them.
2. Poll DescribeStackEvents for the mapping's CREATE_FAILED row.
3. Compare its ResourceStatusReason with AWS's: the emulator's string lacks the leading `Invalid request provided: ` inside the handler message.
