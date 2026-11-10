Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sns.ddb-stream-esm-create-rejects-an-onfailure-destination-of-an-unsupported-service
Issue-Key: dynamodb__lambda__sns.ddb-stream-esm-create-rejects-an-onfailure-destination-of-an-unsupported-service|_error#7|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it")
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# CreateEventSourceMapping accepts an OnFailure destination of an unsupported service instead of refusing it

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the ESM destination-config validation does not check the destination ARN's service.

AWS: `CreateEventSourceMapping`'s `DestinationConfig.OnFailure.Destination` for a stream (DynamoDB/Kinesis) mapping may only name a supported destination type (an SQS queue ARN, an SNS topic ARN or an S3 bucket ARN). An ARN naming any other service is refused at create time with `InvalidParameterValueException` describing the unsupported destination; the mapping is not created. Smithy (lambda): CreateEventSourceMapping input `DestinationConfig` -> OnFailure{Destination}, errors include InvalidParameterValueException at HTTP 400.

Emulator: the call succeeded — the mapping is created with the unsupported destination recorded.

Why: a typo'd or wrong-service failure destination is accepted, so failed batches are routed nowhere and the loss is silent; the operator believes a dead-letter path exists.

Fix: the DestinationConfig validation in the CreateEventSourceMapping handler (and the matching check in UpdateEventSourceMapping).

Traces: dynamodb__lambda__sns.ddb-stream-esm-create-rejects-an-onfailure-destination-of-an-unsupported-service.

## Evidence
Expected: InvalidParameterValueException (HTTP 400) naming the unsupported OnFailure destination; no mapping created.
Actual: AssertionError: the call SUCCEEDED; real AWS rejects it.

## Steps to Reproduce
1. Create a table with a stream and a function.
2. lambda:CreateEventSourceMapping with the stream ARN and DestinationConfig.OnFailure.Destination set to an ARN of a service that is not SQS/SNS/S3.
3. Observe the mapping is created instead of being refused with InvalidParameterValueException.
