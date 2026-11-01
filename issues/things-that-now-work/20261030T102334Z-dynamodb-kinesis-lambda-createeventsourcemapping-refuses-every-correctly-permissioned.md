Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.check-esm-wrong-stream-arn-rejected dynamodb__kinesis__lambda.check-kinesis-batch-window-aggregation dynamodb__kinesis__lambda.check-kinesis-esm-fails-without-permissions dynamodb__kinesis__lambda.check-kinesis-record-base64-data dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda dynamodb__kinesis__lambda.check-kinesis-to-dynamodb-pipeline dynamodb__kinesis__lambda.kinesis-esm-batch-item-failure-redelivers-suffix dynamodb__kinesis__lambda.kinesis-esm-same-partition-key-order-preserved dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-bisect-isolates-poison-and-processes-others-once dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-healthy-record-sends-no-on-failure-message dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-retry-exhaustion-drops-poison-batch-after-two-attempts dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions
Issue-Key: lambda|CreateEventSourceMapping|InvalidParameterValueException
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-esm-fails-without-permissions|_create_esm_retrying_role_propagation#5|Timeout|time.sleep(3)
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda|_wire_event_source#7|Timeout|time.sleep(3.0)
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-bisect-isolates-poison-and-processes-others-once|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-healthy-record-sends-no-on-failure-message|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-retry-exhaustion-drops-poison-batch-after-two-attempts|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions|run#9|AssertionError|assert message == (
Issue-Commit: 0e1c1b5eee37df9cd7b5f3886cad9124e3f02059

# dynamodb__kinesis__lambda: CreateEventSourceMapping refuses every correctly-permissioned Kinesis-source mapping (the gate demands kinesis:ListStreams on the stream ARN), and the DynamoDB-Streams refusal sentence drops its trailing "in IAM." (2 causes, 13 checks)

> _Note: this issue was identified against the code base at commit `0e1c1b5eee37df9cd7b5f3886cad9124e3f02059`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
REOPENS 20261025T035202Z-cloudwatch-kinesis-lambda-createeventsourcemapping-never-authorizes-the-function.md: its fix left the call refused.

That issue asked for an execution-role authorization gate on CreateEventSourceMapping. The gate now exists (services/service-com-amazonaws-lambda-awsgirapiservice/src/state/event_source_authorization.rs, reached from create_event_source_mapping.rs), but it is calibrated so that NO role can pass it for a Kinesis source, so the positive control of every Kinesis ESM check is now refused instead of accepted. Two net-new root causes, each needing its own fix. Related open issue 20261029T180026Z-dynamodb-kinesis-lambda-event-source-mappings-are-created-for-non-existent.md carries the earlier over-permissive form of these same checks; it does not cover the refusal described here.

Cause 1 of 2: the Kinesis-source authorization gate requires `kinesis:ListStreams` on the EventSourceArn, so a role granted exactly the stream-read actions is still refused.
  AWS: Smithy: Lambda `CreateEventSourceMapping` succeeds with HTTP 202 (output `EventSourceMappingConfiguration`) and models `InvalidParameterValueException` at httpError 400. At create time Lambda authorizes the function's execution role for the reads its poller makes against the NAMED stream. A role whose identity policy allows `kinesis:GetRecords`, `kinesis:GetShardIterator`, `kinesis:DescribeStream`, `kinesis:DescribeStreamSummary` and `kinesis:ListShards` on that stream's ARN passes that gate and the call is accepted (State `Creating`/`Enabled`). `kinesis:ListStreams` is a list action with no resource-level scope in IAM — it can only ever be granted on `*`, and AWS's own managed `AWSLambdaKinesisExecutionRole` grants it that way — so AWS cannot and does not require it on the stream ARN. It appears only in the refusal SENTENCE, as advice; it is not a precondition of the resource-scoped grant.
  Emulator: the gate walks a fixed six-action list for the Kinesis vendor (`GetRecords`, `GetShardIterator`, `DescribeStream`, `DescribeStreamSummary`, `ListShards`, `ListStreams`) and evaluates EVERY one of them with the EventSourceArn as the resource. `ListStreams` on a stream ARN is never allowed by any grantable policy, so `withheld` always names it and the call is refused 400 `InvalidParameterValueException` with `Cannot access stream <stream ARN>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, ListShards, and ListStreams Actions on your stream.` The fixed text of that sentence and the action list are correct; the ARN is the request's EventSourceArn. The defect is that the refusal fires at all.
  Why: a client can never create a Kinesis event source mapping. Correctly-written least-privilege roles are rejected with an error telling them to grant something IAM will not scope to a stream, so the only escape is a wildcard policy. Checks that first stand up a working mapping as a positive control cannot proceed: some surface the raw ClientError, and those whose helper retries CreateEventSourceMapping through the IAM-propagation window spin until the 60 s harness cap. It also hides Cause 1 of the related open issue: the negative half of the same checks can no longer be distinguished from a target that refuses every Kinesis mapping.
  Fix: the Kinesis entry of the event-source read catalog used by the CreateEventSourceMapping authorization gate — the set of (action, resource) pairs a Kinesis data stream is authorized on must not include a non-resource-level list action; `ListStreams` belongs in the refusal wording only, not in the evaluated set.
  Traces: dynamodb__kinesis__lambda.check-esm-wrong-stream-arn-rejected, .check-kinesis-batch-window-aggregation, .check-kinesis-esm-fails-without-permissions, .check-kinesis-record-base64-data, .check-kinesis-stream-triggers-lambda, .check-kinesis-to-dynamodb-pipeline, .kinesis-esm-batch-item-failure-redelivers-suffix, .kinesis-esm-same-partition-key-order-preserved, .xsvc-lambda-2-kinesis-esm-bisect-isolates-poison-and-processes-others-once, .xsvc-lambda-2-kinesis-esm-healthy-record-sends-no-on-failure-message, .xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer, .xsvc-lambda-2-kinesis-esm-retry-exhaustion-drops-poison-batch-after-two-attempts. File/line: services/service-com-amazonaws-lambda-awsgirapiservice/src/state/event_source_authorization.rs:127-140 (EVENT_SOURCE_ACCESS Kinesis entry) and :181-201 (withheld), reached from services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_event_source_mapping.rs:568-571.

Cause 2 of 2: the DynamoDB-Streams permission refusal message omits its trailing ` in IAM.`
  AWS: recorded behavior only (the message text is not in the Smithy model; `InvalidParameterValueException` is modelled at httpError 400). For a DynamoDB-Streams EventSourceArn whose function execution role holds none of the stream-read grants, Lambda refuses 400 `InvalidParameterValueException` with the message rule `Cannot access stream <stream ARN>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.` Fixed text: `Cannot access stream `, `. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.` Variable part: the stream ARN, taken verbatim from the request's EventSourceArn. Note the DynamoDB wording ends `on your stream in IAM.` while the Kinesis wording of the same error class ends `on your stream.` — the suffix is source-specific.
  Emulator: the refusal is raised with the right code and the right action list, but the sentence ends `...and ListStreams Actions on your stream.` — the ` in IAM` segment is missing, because the shard-source sentence is formatted from one template shared with the Kinesis source.
  Why: a client that matches or logs the exact AWS refusal text for this IAM-misconfiguration class (a very common one — it is the error that tells an operator the ESM role is short a grant) does not recognise the emulator's message, so error-string-driven remediation and test assertions diverge even though the code and status are right.
  Fix: the refusal wording for the shard-source authorization refusal — the DynamoDB-Streams source needs its own suffix (` in IAM.`) rather than the Kinesis source's.
  Traces: dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions (the same defect also shows in service dynamodb__lambda__s3, filed separately because an issue carries one service). File/line: services/service-com-amazonaws-lambda-awsgirapiservice/src/state/event_source_authorization.rs:204-218 (Refusal::Shards message) and :141-151 (the DynamoDB entry).

## Evidence
Cause 1 — the same refusal on a role explicitly granted the five resource-scoped read actions on that exact stream ARN:
- dynamodb__kinesis__lambda.check-esm-wrong-stream-arn-rejected: `botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValueException) when calling the CreateEventSourceMapping operation: Cannot access stream arn:aws:kinesis:<region>:<account>:stream/<stream>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, ListShards, and ListStreams Actions on your stream.`
- Identical refusal in .check-kinesis-batch-window-aggregation, .check-kinesis-record-base64-data, .check-kinesis-to-dynamodb-pipeline, .kinesis-esm-batch-item-failure-redelivers-suffix, .kinesis-esm-same-partition-key-order-preserved.
- .check-kinesis-esm-fails-without-permissions, .check-kinesis-stream-triggers-lambda, and the four .xsvc-lambda-2-kinesis-esm-* checks end `Check timed out after 60.0s`: their CreateEventSourceMapping helper retries on InvalidParameterValueException for 60–90 s to ride out IAM propagation, and the refusal never stops recurring.
- The role in those checks carries exactly `{"Effect":"Allow","Action":["kinesis:GetRecords","kinesis:GetShardIterator","kinesis:DescribeStream","kinesis:DescribeStreamSummary","kinesis:ListShards"],"Resource":"<that stream ARN>"}` plus logs. On AWS this call is accepted 202.
Expected: HTTP 202 with an EventSourceMappingConfiguration whose State is `Creating` or `Enabled`. Actual: HTTP 400 InvalidParameterValueException as quoted. The stream name/ARN, account and region in the quoted message are per-run values; the requirement is acceptance.

Cause 2 — dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions:
`AssertionError: Cannot access stream arn:aws:dynamodb:<region>:<account>:table/<table>/stream/<label>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream.`
Expected message: `Cannot access stream <that same ARN>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.`
Actual: the identical sentence with ` in IAM` absent before the final period. Code (`InvalidParameterValueException`), status 400 and the absence of any created mapping are all correct.

## Steps to Reproduce
Cause 1:
1. Boot the emulator. Create a Kinesis data stream (ShardCount=1) and wait for ACTIVE.
2. Create an IAM role trusted by lambda.amazonaws.com whose inline policy allows CloudWatch Logs plus exactly `kinesis:GetRecords`, `kinesis:GetShardIterator`, `kinesis:DescribeStream`, `kinesis:DescribeStreamSummary`, `kinesis:ListShards` with `Resource` = that stream's ARN. Do NOT add `kinesis:ListStreams` (IAM cannot scope it to a stream).
3. CreateFunction (python3.12) with that role; wait for State Active.
4. CreateEventSourceMapping(FunctionName=fn, EventSourceArn=<stream ARN>, StartingPosition='TRIM_HORIZON', Enabled=True).
   Expected: HTTP 202, State `Creating` or `Enabled`.
   Observed: HTTP 400 InvalidParameterValueException `Cannot access stream <arn>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, ListShards, and ListStreams Actions on your stream.`
5. Replay the 12 Cause-1 check ids; the six that do not retry surface the ClientError, the six that retry the IAM-propagation window hang to the 60 s cap.

Cause 2:
1. CreateTable with StreamSpecification enabled; take its LatestStreamArn.
2. Create a role trusted by lambda.amazonaws.com with ONLY CloudWatch Logs permissions, and a function using it.
3. CreateEventSourceMapping(EventSourceArn=<stream ARN>, FunctionName=fn, StartingPosition='LATEST', BatchSize=1).
   Expected: 400 InvalidParameterValueException, message ending `...Actions on your stream in IAM.`
   Observed: the same message ending `...Actions on your stream.`
4. Replay dynamodb__kinesis__lambda.check-ddb-esm-fails-without-stream-permissions.
