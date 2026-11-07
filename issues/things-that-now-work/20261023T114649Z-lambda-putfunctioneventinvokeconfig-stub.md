Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.metrics-lambda-async-retry-meters-every-attempt-but-each-event-once cloudwatch__lambda.metrics-lambda-dead-letter-errors-counted-only-when-dlq-delivery-fails cloudwatch__lambda.obs-lambda-1-deleted-on-failure-queue-meters-delivery-failure-and-event-age cloudwatch__lambda.obs-lambda-1-fifo-queue-on-failure-destination-rejected-at-configuration dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive dynamodb__events__lambda.lambda-destination-eventbridge-missing-permission-rejected dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative events__lambda__s3.eventbridge-lambda-failure-routes-to-dlq events__lambda__sqs.a-failing-sibling-targets-retries-do-not-redeliver-to-the-target-that-succeeded lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected lambda.event-invoke-config-crud lambda.event-invoke-config-retry-bounds-rejected lambda.list-function-event-invoke-configs-reflects lambda.update-function-event-invoke-config-partial-update lambda__scheduler.scheduler-lambda-retry-policy-invocation-count lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs lambda__sns__sqs.snstrig-execution-role-without-sqs-grant-rejects-onfailure-destination lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs lambda__sqs.async-retry-budget-of-two-yields-exactly-three-attempts lambda__sqs.lambda-async-retry-exhausted-routes-to-onfailure-destination lambda__sqs.maximum-event-age-discards-the-throttled-async-invocation-to-the-on-failure-destination lambda__sqs.xsvc-lambda-3-async-onsuccess-destination-record-shape lambda__sqs.xsvc-lambda-3-sync-invoke-produces-no-onsuccess-destination-record
Issue-Key: lambda|PutFunctionEventInvokeConfig|NotImplemented
Issue-Key: lambda__sns__sqs.snstrig-execution-role-without-sqs-grant-rejects-onfailure-destination|run#8|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement lambda PutFunctionEventInvokeConfig: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
`PutFunctionEventInvokeConfig` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/put_function_event_invoke_config.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

25 recorded unit(s) fail on their `PutFunctionEventInvokeConfig` call, so each one unblocks only when `PutFunctionEventInvokeConfig` works: `cloudwatch__lambda.metrics-lambda-async-retry-meters-every-attempt-but-each-event-once`, `cloudwatch__lambda.metrics-lambda-dead-letter-errors-counted-only-when-dlq-delivery-fails`, `cloudwatch__lambda.obs-lambda-1-deleted-on-failure-queue-meters-delivery-failure-and-event-age`, `cloudwatch__lambda.obs-lambda-1-fifo-queue-on-failure-destination-rejected-at-configuration`, `dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive`, `dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive`, `dynamodb__events__lambda.lambda-destination-eventbridge-missing-permission-rejected`, `dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative`, `events__lambda__s3.eventbridge-lambda-failure-routes-to-dlq`, `events__lambda__sqs.a-failing-sibling-targets-retries-do-not-redeliver-to-the-target-that-succeeded`, `lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected`, `lambda.event-invoke-config-crud`, `lambda.event-invoke-config-retry-bounds-rejected`, `lambda.list-function-event-invoke-configs-reflects`, `lambda.update-function-event-invoke-config-partial-update`, `lambda__scheduler.scheduler-lambda-retry-policy-invocation-count`, `lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq`, `lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs`, `lambda__sns__sqs.snstrig-execution-role-without-sqs-grant-rejects-onfailure-destination`, `lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs` (+5 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/put_function_event_invoke_config.rs`. Replaying the corpus, the emulator refused the `PutFunctionEventInvokeConfig` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutFunctionEventInvokeConfig operation: operation PutFunctionEventInvokeConfig is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda emulator
Call PutFunctionEventInvokeConfig and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
