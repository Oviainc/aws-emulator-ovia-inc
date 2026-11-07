Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 90646c8ffd3c669d4dcee60c7513b0fcae4d7b5c

# dynamodb__kinesis__lambda: a TRIM_HORIZON Kinesis event source mapping never invokes its function, so the record-driven DynamoDB write never happens

> _Note: this issue was identified against the code base at commit `90646c8ffd3c669d4dcee60c7513b0fcae4d7b5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause in the dynamodb__kinesis__lambda group (the sibling delivery checks of this group are carried by the open issue 20261029T180026Z-dynamodb-kinesis-lambda-event-source-mappings-are-created-for-non-existent.md, Cause 2, and by 20261105T111951Z-dynamodb-kinesis-lambda-the-lambda-python-runtime-has.md; neither of those carries this check's replay key, so it is filed here and those issues are named for the fixer's benefit).

Cause 1 of 1: an ENABLED Kinesis event source mapping created with StartingPosition=TRIM_HORIZON over an ACTIVE single-shard stream never invokes the mapped function, so nothing the handler would do downstream ever happens.

AWS: CreateEventSourceMapping (POST /2015-03-31/event-source-mappings, 202; Smithy: input StartingPosition -> EventSourcePosition enum, EventSourceArn -> Arn; errors InvalidParameterValueException 400 / ResourceNotFoundException 404 / ResourceConflictException 409) is accepted for a Kinesis stream ARN, and the mapping then transitions through Creating to Enabled and POLLS the stream. For TRIM_HORIZON the poller starts at the oldest untrimmed record, so a record written AFTER the mapping exists is delivered. The function is invoked synchronously with an event whose `Records` list carries one entry per Kinesis record under the `kinesis` key (`data` base64-encoded, `partitionKey`, `sequenceNumber`, `approximateArrivalTimestamp`), plus `eventSource: "aws:kinesis"` and `eventSourceARN` equal to the stream ARN. No error is involved on this path: the call succeeds and delivery is an asynchronous effect. Recorded behavior shows the handler runs and its own DynamoDB PutItem lands, so a ConsistentRead GetItem of the sink key returns the item with the handler's attributes.

Emulator: the mapping is created and reported, but no invocation ever occurs. The check writes one record with PutRecord and then polls GetItem on the sink table with ConsistentRead=True; the item never appears and the check ends with `Check timed out after 60.0s` (the harness polls up to 150 s for the effect). No refusal, no non-2xx: every control-plane call succeeds and the data path is silently inert.

Why: a client that wires Kinesis to Lambda gets a mapping that reports healthy and delivers nothing. Every stream-driven compute workload (fan-out, enrichment, Kinesis->Lambda->DynamoDB pipelines) silently produces no output, and because there is no error anywhere the caller has nothing to retry or alarm on — the failure only shows up as missing downstream data.

Fix: the Lambda event-source-mapping poller for Kinesis sources — the component that drives an Enabled mapping to read shards from the configured StartingPosition and invoke the function with the aws:kinesis event envelope (and, once it invokes, the handler's own outbound AWS calls, which the two issues named above also touch).

Traces: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda.

## Evidence
Check dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda (mismatch).
Setup (all succeeded against the emulator): kinesis.create_stream(ShardCount=1) -> DescribeStream StreamStatus=ACTIVE; a Lambda function whose role is granted kinesis:GetRecords/GetShardIterator/DescribeStream/ListShards on the stream ARN and dynamodb:PutItem on the sink table; CreateEventSourceMapping(EventSourceArn=<stream arn>, StartingPosition="TRIM_HORIZON").
Action: kinesis.put_record(Data=json.dumps({"key": <marker>}), PartitionKey=<marker>).
Expected: the mapped handler is invoked with Records[0].kinesis.data (base64 of that JSON) and writes the sink item, so dynamodb.get_item(Key={"pk": {"S": "kinesis-<marker>"}}, ConsistentRead=True) returns an Item with source = "kinesis-stream" (the fixed text here is the attribute names `pk`/`source` and the literal value `kinesis-stream` written by the handler; `<marker>` is a per-run value from the request).
Actual: the sink item never appears; polling ends with `Error: Check timed out after 60.0s`. No error response is produced by any call — the mapping simply never delivers.

## Steps to Reproduce
1. Boot the emulator.
2. kinesis CreateStream with ShardCount=1 and wait for StreamStatus=ACTIVE.
3. Create a DynamoDB sink table and an IAM role allowing kinesis:GetRecords/GetShardIterator/DescribeStream/ListShards on the stream and dynamodb:PutItem on the table; create a Lambda function with that role whose handler decodes each record['kinesis']['data'] and PutItems {"pk": {"S": "kinesis-<key>"}, "source": {"S": "kinesis-stream"}}.
4. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<stream arn>, StartingPosition="TRIM_HORIZON") and wait for the mapping to report Enabled.
5. kinesis PutRecord one JSON record.
6. Poll dynamodb GetItem on the sink key with ConsistentRead=True: the item never materialises, whereas on AWS it appears once the poller invokes the function.
Replay: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda.
