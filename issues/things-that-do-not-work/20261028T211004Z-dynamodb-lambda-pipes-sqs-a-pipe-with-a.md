Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target
Issue-Key: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment|_warm_up#8|Timeout|for m in sqs.receive_message(QueueUrl=target_url, MaxNumberOfMessages=10,
Issue-Key: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target|_warm_up#7|Timeout|for m in sqs.receive_message(QueueUrl=target_url, MaxNumberOfMessages=10,
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# dynamodb__lambda__pipes__sqs: a pipe with a DynamoDB-Streams source and a Lambda enrichment never delivers anything to its target, so stream-source filtering and NewImage/OldImage shaping are unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
One net-new root cause, grounded in two failing behavioral checks (Cause 1 of 1).

Cause 1 of 1: an EventBridge Pipes pipe whose Source is a DynamoDB Streams stream ARN, whose Enrichment is a Lambda function and whose Target is an SQS queue is accepted by the control plane and settles in CurrentState RUNNING, but its data plane never reads the stream, never invokes the enrichment and never sends anything to the target. Failing checks: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment, dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target.

AWS: the Pipes Smithy model describes only the control plane for this (CreatePipe with Source = a DynamoDB stream ARN, SourceParameters.DynamoDBStreamParameters — `StartingPosition` (required), `BatchSize` @range 1..10000 — and SourceParameters.FilterCriteria.Filters @length 0..5, plus Enrichment/Target and DesiredState/CurrentState on DescribePipe); the delivery timing and record shape are recorded behavior only. On AWS, once DescribePipe reports CurrentState RUNNING, the pipe polls the stream's shards from StartingPosition, applies FilterCriteria BEFORE the enrichment (records whose eventName does not match a filter pattern are dropped and are never handed to the enrichment), invokes the enrichment with an array of stream records in the DynamoDB Streams envelope (each record carrying `eventName` and a `dynamodb` member with `Keys` and, for a NEW_AND_OLD_IMAGES stream, `NewImage` on INSERT/MODIFY and `OldImage` on MODIFY/REMOVE in typed DynamoDB JSON), and sends the enrichment's returned results to the target queue; per-key ordering within a shard is preserved and delivery is at-least-once.

Emulator: with the table created with StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}, the enrichment Active and the pipe reported in its desired state RUNNING, no change record ever reaches the target queue. Both checks first write warm-up/measured items and then poll the target with ReceiveMessage (the checks' own bounds are 150 s); no message ever arrives and the suite's per-check deadline cuts both off with `Error: Check timed out after 60.0s`. Because nothing is ever delivered, neither the filter-before-enrichment contract nor the typed NewImage/OldImage envelope can be observed at all.

Why: a client that wires DynamoDB Streams -> Pipes -> (Lambda enrichment) -> SQS sees its target queue stay empty forever while the control plane reports a healthy RUNNING pipe. Any consumer waiting on the target hangs, change-data-capture pipelines silently lose every record, and the two semantics clients depend on — that FilterCriteria drops MODIFY/REMOVE before the enrichment is ever invoked, and that the enrichment receives the typed NewImage (and OldImage only for MODIFY/REMOVE) so values can be computed from the post-image — are unverifiable.

Fix: the EventBridge Pipes runtime for stream sources — the shard poller that must, while CurrentState is RUNNING, read the DynamoDB stream from StartingPosition in batches of BatchSize, evaluate SourceParameters.FilterCriteria against each record before enrichment, invoke the Enrichment with the batch in the DynamoDB Streams record envelope (eventName + dynamodb.Keys/NewImage/OldImage per the table's StreamViewType), and forward each enrichment result to the Target. Related but non-covering open issues: 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md and 20261027T020619Z-lambda-pipes-sqs-a-started-pipe-with-an.md record the same "a RUNNING pipe does nothing" symptom for SQS-source pipes and do not carry these two replay keys; this cause adds the DynamoDB-Streams source path (filter-before-enrichment and the typed image envelope).

Traces: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment, dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target.

## Evidence
Both checks fail with the same report detail:
- [mismatch] dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment: `Error: Check timed out after 60.0s`
- [mismatch] dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target: `Error: Check timed out after 60.0s`

Check 1 (filter before enrichment) expected: for one key written INSERT(v=1) -> MODIFY(v=2) -> REMOVE -> INSERT(v=3) on a pipe whose SourceParameters carry FilterCriteria {"eventName": ["INSERT"]}, the target queue holds exactly the two INSERT records for that key (eventName/v pairs {(INSERT,1),(INSERT,3)}), each marked enriched by the enrichment, and the enrichment's own audit queue shows it was handed INSERT records only — no MODIFY and no REMOVE.
Actual: the target queue receives nothing at all; the check's drain loop makes no progress and is cut off at the suite deadline. The audit queue is likewise empty, so the filter cannot be distinguished from a pipe that delivers nothing.

Check 2 (NewImage computed at target) expected: for PutItem {pk, qty=3, price=7} followed by UpdateItem SET qty=5, the target receives an INSERT record whose new_image is exactly {"pk":{"S":<pk>},"qty":{"N":"3"},"price":{"N":"7"}} with no OldImage and line_total 21 (qty*price computed by the enrichment from the NewImage), and a MODIFY record whose new_image is {"pk":{"S":<pk>},"qty":{"N":"5"},"price":{"N":"7"}} with an OldImage present and line_total 35.
Actual: no INSERT or MODIFY record ever arrives at the target; the check is cut off at the suite deadline before any assertion on the record shape can run.

In both checks the control plane is healthy: CreateTable with StreamSpecification returns and DescribeTable reports TableStatus ACTIVE with a LatestStreamArn, the enrichment Lambda reaches State Active, and DescribePipe reports CurrentState RUNNING (the checks assert and pass these before polling). The divergence is purely that the running pipe never consumes the stream and never produces a target message. (Per-run values above — table/queue/function names, the generated pk and the stream ARN — are environment values, not part of the requirement; the requirement is that the records arrive with that shape.)

## Steps to Reproduce
1. Create an SQS target queue (and, for check 1, a second audit queue).
2. CreateTable with KeySchema pk (HASH), BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}; wait for TableStatus ACTIVE and capture LatestStreamArn.
3. Create a python3.12 Lambda enrichment that returns one object per input record derived from r["eventName"] and r["dynamodb"]["NewImage"]/["OldImage"]; wait for State Active.
4. Create an IAM role trusted by pipes.amazonaws.com allowing dynamodb:DescribeStream/GetRecords/GetShardIterator/ListStreams on the stream ARN, lambda:InvokeFunction on the enrichment and sqs:SendMessage on the target.
5. `aws pipes create-pipe --name <p> --role-arn <role> --source <stream-arn> --target <target-queue-arn> --enrichment <fn-arn> --desired-state RUNNING --source-parameters '{"DynamoDBStreamParameters":{"StartingPosition":"LATEST","BatchSize":1}}'` (for check 1 add `"FilterCriteria":{"Filters":[{"Pattern":"{\"eventName\":[\"INSERT\"]}"}]}`); wait for DescribePipe CurrentState RUNNING.
6. Write items to the table (PutItem, then UpdateItem, then DeleteItem, then PutItem again for check 1; PutItem then UpdateItem for check 2) and poll the target queue with ReceiveMessage.
7. Observe the target queue never receives any message, so neither the pre-enrichment filtering nor the typed NewImage/OldImage envelope is ever observable.
Replay: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment, dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target
