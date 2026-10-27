Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.ddb-stream-esm-batch-size-config-positive
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-batch-size-config-positive|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 2524c81a99ad2e7d923e1349dfe7c153509bdc71

# dynamodb__events__lambda: a DynamoDB-Streams event source mapping with BatchSize=3 never delivers any record to its Lambda, so the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `2524c81a99ad2e7d923e1349dfe7c153509bdc71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: a DynamoDB-Streams event source mapping configured with BatchSize never invokes its target function, so neither the at-least-once delivery contract nor the BatchSize ceiling can be observed.

AWS: recorded behavior only for the end-to-end propagation (no single Smithy operation models it). A table created with a StreamSpecification exposes LatestStreamArn; lambda CreateEventSourceMapping with that EventSourceArn, a StartingPosition and BatchSize=3 reaches state Enabled, and from then on every item written to the table is delivered to the function at least once, batched, with no single invocation receiving MORE records in event['Records'] than the configured BatchSize. The check observes this through the handler's own writes into an observation table: one `batch-<run>` counter row incremented per invocation, and one `rec-<run>-<eventID>` row per delivered stream record carrying that invocation's record count.
Emulator: nothing is ever delivered. The handler is never invoked, so neither the invocation counter row nor any per-record row appears, and the check polls the observation table until the harness cuts it at its 60 s cap (`Error: Check timed out after 60.0s`). The failure is a non-event, not a wrong value: no invocation at all.
Why: a client that wires a stream to a function gets a mapping that reports Enabled and then silently processes nothing. Every change-data-capture design — fanning table writes into aggregation, search indexing, outbox publishing — appears correctly configured on the emulator and moves zero records, and the BatchSize ceiling (the knob that bounds a handler's per-invocation work and its retry blast radius) can never be exercised at all.
Fix: the stream-to-Lambda poller that backs lambda event source mappings whose EventSourceArn is a DynamoDB stream — the component must actually iterate the stream's shards for an Enabled mapping, assemble batches no larger than BatchSize, and invoke the target function; today nothing drives it. Several sibling symptoms of the same missing poller are already filed and NOT covered by this issue's check: 20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md and 20261025T011017Z-dynamodb-createtable-with-streamspecification-leaves-the-table-invisible-to.md (stream enablement on CreateTable), 20261026T090231Z-dynamodb-lambda-a-dynamodb-streams-event-source-mapping.md (MaximumRecordAgeInSeconds), 20261024T150703Z-apigateway-dynamodb-lambda-logs-a-dynamodb-streams-event.md (ParallelizationFactor), 20261025T192630Z-dynamodb-lambda-s3-a-dynamodb-streams-event-source.md (INSERT records). If the poller is implemented as one fix, this check must still be replayed to confirm the BatchSize ceiling specifically.
Traces: dynamodb__events__lambda.ddb-stream-esm-batch-size-config-positive.

## Evidence
dynamodb__events__lambda.ddb-stream-esm-batch-size-config-positive
  Error: Check timed out after 60.0s

Expected (AWS): with a stream-enabled table, an execution role allowing dynamodb:GetRecords /
GetShardIterator / DescribeStream / ListStreams plus dynamodb:PutItem / UpdateItem, and an event
source mapping with BatchSize=3, the observation table accumulates:
  - one row pk="batch-<run_id>" whose `attempts` counter is incremented once per invocation, and
  - one row pk="rec-<run_id>-<eventID>" per delivered stream record, carrying
    batch_size = len(event['Records']) and key_id = the record's dynamodb.Keys.id,
with every observed batch_size <= 3 and every written record represented exactly once.

Actual (emulator): no invocation occurs — neither the `batch-<run_id>` counter row nor any
`rec-<run_id>-<eventID>` row is ever written, and the check polls to the 60 s harness cap.

## Steps to Reproduce
1. CreateTable an observation table (hash key `pk` (S)).
2. CreateTable a source table with a StreamSpecification (StreamEnabled=true, e.g. StreamViewType NEW_AND_OLD_IMAGES) and capture LatestStreamArn.
3. Create an IAM role for the function allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents, dynamodb:GetRecords/GetShardIterator/DescribeStream/ListStreams, and dynamodb:PutItem/UpdateItem.
4. CreateFunction with a handler that, per invocation, UpdateItem-increments `attempts` on pk="batch-<run_id>" in the observation table and PutItem-writes pk="rec-<run_id>-<eventID>" with batch_size=len(event['Records']) for every record in event['Records'].
5. lambda CreateEventSourceMapping(EventSourceArn=<LatestStreamArn>, FunctionName=<fn>, StartingPosition=LATEST, BatchSize=3); wait for State Enabled.
6. PutItem several items into the source table.
7. Poll the observation table. Expected: the `batch-<run_id>` counter rises and one `rec-` row appears per written item, each with batch_size <= 3. Actual: the observation table stays empty and the check runs to the 60 s cap.
