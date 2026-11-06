Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-transactional-capacity-is-double
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-transactional-capacity-is-double dynamodb-partiql-insert-select-update-delete dynamodb-partiql-statement-execution dynamodb-execute-statement-invalid-partiql-rejected dynamodb-partiql-index-qualifier
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|TransactGetItems|2xx->2xx|$.ConsumedCapacity[*].ReadCapacityUnits
Issue-Key: dynamodb|TransactWriteItems|2xx->2xx|$.ConsumedCapacity[*].WriteCapacityUnits
Issue-Key: dynamodb|BatchExecuteStatement|2xx->2xx|$.Responses[*].TableName
Issue-Key: dynamodb|ExecuteTransaction|2xx->2xx|$.Responses[*]
Issue-Key: dynamodb|ExecuteStatement|4xx->4xx|$.message
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB transaction and PartiQL responses omit capacity, table name and shape their item responses wrong

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactGetItems

## Description
The transactional and PartiQL operations succeed but their response payloads are incomplete or wrongly shaped, and the malformed-statement refusal carries an invented message. 5 causes, all dynamodb.

Cause 1 of 5: TransactGetItems' ConsumedCapacity entry omits ReadCapacityUnits.
AWS: Smithy: TransactGetItems output carries ConsumedCapacity (ConsumedCapacityMultiple), and ConsumedCapacity has CapacityUnits, ReadCapacityUnits and WriteCapacityUnits (all ConsumedCapacityUnits doubles). A transactional read consumes twice the capacity of an eventually consistent read, which is exactly what this fixture asserts: the recording reports ReadCapacityUnits for the transaction's reads.
Emulator: the ConsumedCapacity entry is present but ReadCapacityUnits is absent.
Why: the read/write split is how a client attributes transactional cost and the 2x transactional multiplier; with the member missing, capacity accounting and throughput-planning logic reads zero reads for a transaction that performed them.
Fix: the TransactGetItems response projection / its capacity accounting — populate ReadCapacityUnits (and CapacityUnits) on each ConsumedCapacity entry when ReturnConsumedCapacity was requested.
Traces: dynamodb-transactional-capacity-is-double#task#6.

Cause 2 of 5: TransactWriteItems' ConsumedCapacity entry omits WriteCapacityUnits.
AWS: the mirror of cause 1 — ConsumedCapacity.WriteCapacityUnits is reported for a transactional write, again at the doubled transactional rate.
Emulator: WriteCapacityUnits absent from the entry.
Why: as cause 1, on the write side.
Fix: the TransactWriteItems response projection / capacity accounting.
Traces: dynamodb-transactional-capacity-is-double#task#8.

Cause 3 of 5: BatchExecuteStatement's responses omit the TableName of each statement.
AWS: Smithy: BatchStatementResponse carries Error, TableName (@length 3..255, pattern ^[a-zA-Z0-9_.-]+$) and Item. The recording returns TableName on every entry of Responses, naming the table the statement ran against.
Emulator: $.Responses[*].TableName absent.
Why: a batch mixes statements over several tables, and the response entries are positional only; without TableName a caller cannot attribute a per-entry Error or Item to a table without re-parsing its own statements.
Fix: the BatchExecuteStatement response serializer — set TableName on each BatchStatementResponse from the statement's target table.
Traces: dynamodb-partiql-insert-select-update-delete#task#9 (cluster of 2, also dynamodb-partiql-statement-execution#task#5).

Cause 4 of 5: ExecuteTransaction emits an empty object per response where AWS emits none.
AWS: Smithy: ItemResponse has a single optional member Item, and ExecuteTransaction's Responses are omitted for statements that return no item — the recording has no Responses entries for this transaction.
Emulator: Responses[0] and Responses[1] are present as empty objects {}.
Why: a client iterating Responses and dereferencing each entry's Item sees entries that look like results but hold nothing, so result-count checks and 'did this statement return a row' tests both read wrong.
Fix: the ExecuteTransaction response serializer — omit an ItemResponse entry (and the Responses list) rather than emitting an empty structure.
Traces: dynamodb-partiql-insert-select-update-delete#task#10 (cluster of 2, also dynamodb-partiql-statement-execution#task#8).

Cause 5 of 5: the malformed-PartiQL refusal uses an invented sentence.
AWS: the recording shows HTTP 400 with "Statement wasn't well formed, can't be processed: <parser reason>" — here the reason is 'Expected data manipulation'. The fixed text is "Statement wasn't well formed, can't be processed: "; the clause after the colon is the parser's reason for the particular statement.
Emulator: 400 with "Invalid Statement: statement wasn't well formed, could not be parsed" — a different prefix and no parser reason at all.
Why: the clause after the colon is the only indication of what is wrong with the statement; a caller debugging a generated PartiQL string, or a test asserting on the refusal, gets neither AWS's wording nor the reason.
Fix: the ExecuteStatement PartiQL validation — emit AWS's prefix and carry the parser's reason into the message.
Traces: dynamodb-execute-statement-invalid-partiql-rejected#task#3 (cluster of 2, also dynamodb-partiql-index-qualifier#task#10).

Related open issues: '20261025T020208Z-cloudwatch-dynamodb-dynamodb-transactwriteitems-consumedcapacity-omits-writecapacityunits-and-no-aws.md' reports the TransactWriteItems WriteCapacityUnits omission; the replay still reports it as unowned, and the TransactGetItems read-side omission and the three PartiQL causes are not in it. '20261029T180026Z-dynamodb-partiql-resource-policy-arn-addressing-crc32-header.md' covers other PartiQL causes, not the TableName, empty-ItemResponse or malformed-statement message defects above.

## Evidence
Divergence 40 (TransactGetItems, 200->200): $.ConsumedCapacity[0].ReadCapacityUnits missing (expected 2.0 — the value is the fixture's doubled transactional read, the requirement is that the member be reported).
Divergence 41 (TransactWriteItems, 200->200): $.ConsumedCapacity[0].WriteCapacityUnits missing (expected 2.0).
Divergence 29 (BatchExecuteStatement, 200->200): $.Responses[0].TableName and $.Responses[1].TableName missing (expected the fixture's table name), cluster of 2.
Divergence 30 (ExecuteTransaction, 200->200): $.Responses[0] and $.Responses[1] extra — expected None, got {} (cluster of 2).
Divergence 24 (ExecuteStatement, 400->400): $.message expected "Statement wasn't well formed, can't be processed: Expected data manipulation", got "Invalid Statement: statement wasn't well formed, could not be parsed" (cluster of 2).
Model: smithy_query(operation=TransactGetItems) -> output ConsumedCapacity:ConsumedCapacityMultiple, Responses:ItemResponseList; shape=ConsumedCapacity -> CapacityUnits/ReadCapacityUnits/WriteCapacityUnits doubles; shape=BatchStatementResponse -> Error, TableName @length 3..255 pattern ^[a-zA-Z0-9_.-]+$, Item; shape=ItemResponse -> single optional member Item.

## Steps to Reproduce
1. Replay dynamodb-transactional-capacity-is-double#task#6 and #task#8 with ReturnConsumedCapacity set: ConsumedCapacity entries carry no ReadCapacityUnits / WriteCapacityUnits.
2. Replay dynamodb-partiql-insert-select-update-delete#task#9: BatchExecuteStatement -> Responses entries have no TableName.
3. Replay dynamodb-partiql-insert-select-update-delete#task#10: ExecuteTransaction -> Responses contains two empty {} entries instead of being omitted.
4. Replay dynamodb-execute-statement-invalid-partiql-rejected#task#3: ExecuteStatement with a malformed statement -> 'Invalid Statement: ...' instead of "Statement wasn't well formed, can't be processed: <reason>".
