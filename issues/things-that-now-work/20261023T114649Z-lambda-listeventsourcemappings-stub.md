Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected lambda__sqs__stepfunctions.deploy-sfn-lambda-sqs-dlq-redrive-policy-stack
Issue-Key: lambda|ListEventSourceMappings|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement lambda ListEventSourceMappings: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEventSourceMappings

## Description
`ListEventSourceMappings` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/list_event_source_mappings.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `ListEventSourceMappings` call, so each one unblocks only when `ListEventSourceMappings` works: `cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard`, `cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger`, `cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative`, `cloudwatch__lambda__sqs.obs-lambda-2-sqs-esm-kafka-only-metrics-rejected`, `lambda__sfn__sqs.deploy-sfn-lambda-sqs-dlq-redrive-policy-stack`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/list_event_source_mappings.rs`. Replaying the corpus, the emulator refused the `ListEventSourceMappings` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListEventSourceMappings operation: operation ListEventSourceMappings is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda emulator
Call ListEventSourceMappings and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
