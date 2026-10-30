Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-stream-disable-closes-shard
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-stream-disable-closes-shard
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.TableStatus
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# UpdateTable reports TableStatus ACTIVE instead of UPDATING for an accepted change

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Cause 1 of 1 - UpdateTable skips the UPDATING state entirely.
AWS: an accepted UpdateTable answers HTTP 200 with TableDescription.TableStatus = 'UPDATING'. Smithy (com.amazonaws.dynamodb): TableDescription.TableStatus documents 'UPDATING - The table/index configuration is being updated. The table/index remains available for data operations when UPDATING' and 'ACTIVE - The table is ready for use'; the recording for an UpdateTable that turns a table's StreamSpecification off shows status 200 and TableStatus 'UPDATING'. The requirement is the enum value for an accepted-but-not-yet-settled change, not any particular table name or arn from the recording.
Emulator: answers HTTP 200 with TableDescription.TableStatus = 'ACTIVE' - the configuration change is applied synchronously and the intermediate state is never observable.
Why: a client that follows AWS's documented sequence - submit UpdateTable, confirm the table went to UPDATING, then wait for it to return to ACTIVE - never sees the transition. Waiters written that way either report the change as not accepted or treat the pre-existing ACTIVE as the post-change state and read configuration before it is meant to be settled; code that gates on UPDATING to avoid issuing a second overlapping UpdateTable loses that guard.
Fix: the dynamodb UpdateTable handler's table-state transition (mark the stored table UPDATING when the request is accepted and project that in the response, settling to ACTIVE afterwards).
Traces: dynamodbstreams-stream-disable-closes-shard#task#3

## Evidence
Fixture dynamodbstreams-stream-disable-closes-shard step task#3, service dynamodb, operation UpdateTable: status 200 expected and 200 got; $.TableDescription.TableStatus value_mismatch - expected 'UPDATING', got 'ACTIVE'. Smithy: TableDescription.TableStatus enum doc - 'UPDATING - The table/index configuration is being updated'; 'ACTIVE - The table is ready for use'.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Create a table with StreamSpecification StreamEnabled=true and wait for it to be ACTIVE.
3. Call UpdateTable on it with StreamSpecification StreamEnabled=false (fixture dynamodbstreams-stream-disable-closes-shard, step task#3).
4. Read $.TableDescription.TableStatus in the response: the emulator serves 'ACTIVE' where AWS serves 'UPDATING'.
