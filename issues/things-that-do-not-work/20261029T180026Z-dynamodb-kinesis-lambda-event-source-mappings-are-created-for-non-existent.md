Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions dynamodb__kinesis__lambda.check-esm-wrong-stream-arn-rejected dynamodb__kinesis__lambda.check-kinesis-esm-fails-without-permissions dynamodb__kinesis__lambda.check-ddb-stream-event-name-field dynamodb__kinesis__lambda.check-ddb-stream-new-and-old-images dynamodb__kinesis__lambda.check-ddb-stream-shard-ordering dynamodb__kinesis__lambda.check-ddb-stream-triggers-lambda dynamodb__kinesis__lambda.check-kinesis-batch-window-aggregation dynamodb__kinesis__lambda.check-kinesis-record-base64-data dynamodb__kinesis__lambda.kinesis-esm-batch-item-failure-redelivers-suffix dynamodb__kinesis__lambda.kinesis-esm-same-partition-key-order-preserved dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-bisect-isolates-poison-and-processes-others-once dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-healthy-record-sends-no-on-failure-message dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-retry-exhaustion-drops-poison-batch-after-two-attempts dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment
Issue-Key: dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions|_error#7|AssertionError|raise AssertionError(
Issue-Key: dynamodb__kinesis__lambda.check-esm-wrong-stream-arn-rejected|_error#11|AssertionError|raise AssertionError(
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-esm-fails-without-permissions|run#12|AssertionError|raise AssertionError(
Issue-Key: dynamodb__kinesis__lambda.check-ddb-stream-event-name-field|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.check-ddb-stream-new-and-old-images|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.check-ddb-stream-shard-ordering|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.check-ddb-stream-triggers-lambda|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-batch-window-aggregation|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-record-base64-data|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.kinesis-esm-batch-item-failure-redelivers-suffix|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.kinesis-esm-same-partition-key-order-preserved|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-bisect-isolates-poison-and-processes-others-once|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-healthy-record-sends-no-on-failure-message|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer|_probe#16|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-retry-exhaustion-drops-poison-batch-after-two-attempts|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment|_deploy_cfn#7|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# Event source mappings are created for non-existent/unpermitted sources and then never deliver (3 causes, 15 checks)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Three net-new root causes in the dynamodb__kinesis__lambda group. Each needs its own fix.

Cause 1 of 3: CreateEventSourceMapping does not validate the event source or the role's permissions.
AWS: the call is refused with `InvalidParameterValueException` (HTTP 400) when the `EventSourceArn` names a stream that does not exist, and when the function's execution role lacks the stream-read permissions (`dynamodb:DescribeStream`/`GetRecords`/`GetShardIterator`/`ListStreams` or the Kinesis equivalents) — the message names the role and what it may not do.
Emulator: every one of these is ACCEPTED (HTTP 202) and a mapping is returned in `State: Creating` with the bogus `EventSourceArn` (e.g. `arn:aws:kinesis:<region>:<account>:stream/does-not-exist-...`).
Why: a caller gets a mapping that can never work and no error to act on; the permission model that gates stream reads is not applied at create time at all.
Fix: CreateEventSourceMapping validation (resolve the event source in state, and authorize the function role against it).
Traces: dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions, .check-esm-wrong-stream-arn-rejected, .check-kinesis-esm-fails-without-permissions

Cause 2 of 3: event source mappings never deliver records.
AWS: an enabled mapping polls the DynamoDB stream or Kinesis stream and invokes the function — records carry the event name and the new/old images, shard order and same-partition-key order are preserved, batching windows aggregate, base64 data is decoded, partial-batch responses redeliver only the reported suffix, bisect isolates a poison record, retry exhaustion drops the batch after the configured attempts, and the on-failure destination receives a batch-info pointer.
Emulator: nothing is delivered; each check polls for the effect and ends with `Check timed out after 60.0s`.
Why: all stream-driven compute is silently inert, and none of the ESM failure-handling semantics can be relied on.
Fix: the lambda event-source-mapping poller for DynamoDB streams and Kinesis (delivery, ordering, batching, retry/bisect, on-failure destination).
Traces: dynamodb__kinesis__lambda.check-ddb-stream-event-name-field, .check-ddb-stream-new-and-old-images, .check-ddb-stream-shard-ordering, .check-ddb-stream-triggers-lambda, .check-kinesis-batch-window-aggregation, .check-kinesis-record-base64-data, .kinesis-esm-batch-item-failure-redelivers-suffix, .kinesis-esm-same-partition-key-order-preserved, .xsvc-lambda-2-kinesis-esm-bisect-isolates-poison-and-processes-others-once, .xsvc-lambda-2-kinesis-esm-healthy-record-sends-no-on-failure-message, .xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer, .xsvc-lambda-2-kinesis-esm-retry-exhaustion-drops-poison-batch-after-two-attempts

Cause 3 of 3: the stack that wires a DDB stream to a Lambda returns outputs with no OutputValue.
AWS: DescribeStacks returns each declared Output with an `OutputValue` (the table name, stream ARN and function name the template created).
Emulator: reading it raises `KeyError: 'OutputValue'`.
Why: the deploy-then-use workflow cannot discover anything the stack created.
Fix: the cloudformation DescribeStacks output serializer / Output value resolution.
Traces: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment

## Evidence
- Cause 1: AssertionError: expected a refusal but the call succeeded (overly permissive): {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'UUID': ..., 'EventSourceArn': 'arn:aws:kinesis:us-west-2:000000000000:stream/does-not-exist-<id>', 'State': 'Creating', ...}; and "ESM creation succeeded despite missing kinesis stream permissions (overly permissive)".
- Cause 2 (twelve checks): Error: Check timed out after 60.0s.
- Cause 3: KeyError: 'OutputValue'.

## Steps to Reproduce
Boot the emulator and replay the 16 checks listed in trace_ids. Cause 1: CreateEventSourceMapping with an EventSourceArn naming a non-existent stream, and with a function role lacking stream-read permissions. Cause 2: create a mapping over a real stream, write records, and poll for the function's effect. Cause 3: deploy the stream+ESM template and read a stack Output.
