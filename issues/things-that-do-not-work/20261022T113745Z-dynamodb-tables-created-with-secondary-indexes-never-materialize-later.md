Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-backup-lifecycle
Issue-Step: task#22
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-backup-lifecycle dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown dynamodb-index-capacity-arms dynamodb-index-include-projection-echoed-by-describe-table dynamodb-index-key-validation-wording dynamodb-item-collection-metrics-size-estimate dynamodb-pagination-contracts dynamodb-partiql-index-qualifier dynamodb-on-demand-backup-and-restore
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DescribeTable|2xx->4xx|$
Issue-Key: dynamodb|DescribeTable|2xx->4xx|$.Table.TableStatus
Issue-Key: dynamodb|DescribeTable|2xx->4xx|status
Issue-Key: dynamodb|DeleteTable|2xx->4xx|status
Issue-Key: dynamodb|DescribeTable|2xx->4xx|$.Table
Issue-Key: dynamodb|PutItem|2xx->4xx|status
Issue-Key: dynamodb|PutItem|4xx->4xx|$.__type
Issue-Key: dynamodb|PutItem|4xx->4xx|$.message
Issue-Key: dynamodb|DescribeTable|2xx->4xx|$.Table.GlobalSecondaryIndexes[*].IndexStatus
Issue-Key: dynamodb|Query|2xx->4xx|$.Count
Issue-Key: dynamodb|Query|2xx->4xx|$.Items
Issue-Key: dynamodb|Query|2xx->4xx|$.ScannedCount
Issue-Key: dynamodb|Query|2xx->4xx|status
Issue-Key: dynamodb|Query|2xx->4xx|$.LastEvaluatedKey
Issue-Key: dynamodb|Query|4xx->4xx|$.__type
Issue-Key: dynamodb|Query|4xx->4xx|$.message
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Tables created with secondary indexes never materialize: later operations fail ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
Tables whose CreateTable request carries LocalSecondaryIndexes / GlobalSecondaryIndexes (and tables produced by RestoreTableFromBackup) are never registered in state: every subsequent call on them answers 400 ResourceNotFoundException, so whole fixtures collapse. This bundle has 9 causes; all 9 must be closed.

Cause 1 of 9: DescribeTable on a restored / index-bearing table never becomes ACTIVE - it answers ResourceNotFoundException until the poll caps.
AWS: Smithy: DescribeTable -> 200 with Table.TableStatus reaching ACTIVE; ResourceNotFoundException (400) is modeled only for a table that does not exist. The recording shows 200 and TableStatus ACTIVE.
Emulator: 400 with {'__type': 'ResourceNotFoundException'} for the whole 60s poll window.
Why: a client that waits for the table to become ACTIVE before using it never proceeds; every workflow built on an indexed table is unusable.
Fix: the CreateTable/RestoreTableFromBackup handlers and the table registry in state - a create carrying index definitions must insert the table (and its index descriptions) and transition it to ACTIVE.
Traces: dynamodb-backup-lifecycle#task#22 (10 steps in cluster, incl. dynamodb-index-capacity-arms#setup#2, dynamodb-pagination-contracts#setup#2).

Cause 2 of 9: DeleteTable on these tables answers 400 instead of 200. AWS: Smithy: DeleteTable -> 200 TableDescription for an existing table. Emulator: 400, because the table was never created. Why: teardown cannot complete and the client cannot tell a real failure from a phantom table. Fix: table registry (same root as Cause 1); DeleteTable handler. Traces: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown#teardown#8 (9 steps in cluster).

Cause 3 of 9: DescribeTable does not echo INCLUDE projections for GSI/LSI - it answers 400 instead of the full Table structure. AWS: Smithy: DescribeTable -> Table with GlobalSecondaryIndexes[].Projection {ProjectionType INCLUDE, NonKeyAttributes} and LocalSecondaryIndexes[].Projection, as in the recording. Emulator: 400 ResourceNotFoundException, so no projection is echoed at all. Why: a client verifying its index projections cannot. Fix: CreateTable index persistence + DescribeTable index serializer. Traces: dynamodb-index-include-projection-echoed-by-describe-table#task#5 (also #task#6).

Cause 4 of 9: PutItem into an index-bearing table answers 400 instead of 200. AWS: Smithy: PutItem -> 200; ResourceNotFoundException only when the table is absent. Emulator: 400. Why: no data can be written to any indexed table. Fix: table registry (Cause 1). Traces: dynamodb-index-include-projection-echoed-by-describe-table#task#7 (9 steps in cluster, incl. dynamodb-pagination-contracts#setup#4..#setup#7).

Cause 5 of 9: index key type validation reports the wrong error class. AWS: Smithy: PutItem errors include ResourceNotFoundException and the service-common ValidationException; for an item whose index key has the wrong type AWS answers 400 with __type com.amazon.coral.validate#ValidationException and a message naming the index key, its expected and actual type and the IndexName. Emulator: 400 with __type 'ResourceNotFoundException' and a 'Table: ... not found' message. Why: the client sees a retriable/absent-resource condition instead of a permanent input error, so it recreates tables or retries forever instead of fixing the item. Fix: CreateTable index persistence (Cause 1) and PutItem's index-key validation, which must run and raise ValidationException. Traces: dynamodb-index-key-validation-wording#task#3 (also #task#6).

Cause 6 of 9: DescribeTable never reports GlobalSecondaryIndexes[0].IndexStatus ACTIVE. AWS: GlobalSecondaryIndexDescription.IndexStatus reaches ACTIVE after create. Emulator: 400 ResourceNotFoundException through 31 poll attempts. Why: GSI readiness can never be awaited. Fix: table/index registry and the index status state machine. Traces: dynamodb-pagination-contracts#setup#3 (also dynamodb-partiql-index-qualifier#setup#3).

Cause 7 of 9: Query on an indexed table answers 400 instead of an empty page. AWS: Smithy: Query -> 200 with Items [], Count 0, ScannedCount 0 (both default 0). Emulator: 400, members absent. Why: a client that treats 400 as fatal aborts on what AWS reports as an empty result. Fix: table registry (Cause 1); Query handler. Traces: dynamodb-pagination-contracts#task#8.

Cause 8 of 9: paginated Query returns no page and no LastEvaluatedKey. AWS: Smithy: Query is paginated (inputToken ExclusiveStartKey, outputToken LastEvaluatedKey, pageSize Limit) and returns 200 with Items, Count, ScannedCount and LastEvaluatedKey when more data remains. Emulator: 400, nothing returned. Why: pagination cannot be driven at all. Fix: table registry (Cause 1); Query pagination. Traces: dynamodb-pagination-contracts#task#9.

Cause 9 of 9: an invalid ExclusiveStartKey is misclassified. AWS: 400 with __type com.amazon.coral.validate#ValidationException for a malformed starting key (recorded message 'The provided starting key is invalid'). Emulator: 400 with __type 'ResourceNotFoundException'. Why: the client cannot distinguish a bad token from a deleted table, so it retries a request that can never succeed. Fix: table registry (Cause 1) and Query's ExclusiveStartKey validation. Traces: dynamodb-pagination-contracts#task#13.

## Evidence
Representative: fixture dynamodb-backup-lifecycle step task#22, DescribeTable, expected 200 / actual 400:
  $: poll capped at 60s (expected None, got {'__type': 'ResourceNotFoundException', 'message': 'Requested resource not found: Table: ... not found'})
  $.Table.TableStatus: poll: not in ['ACTIVE'] after 5 attempt(s)
Same ResourceNotFoundException cascade for the other causes: DeleteTable 200->400 (9 steps), PutItem 200->400 (9 steps), DescribeTable 200->400 with the entire Table structure missing, Query 200->400 twice, and two clusters where the error CODE is wrong (expected com.amazon.coral.validate#ValidationException, got ResourceNotFoundException). Table names and request ids in the recordings are examples; the status/code/shape are the requirement.

## Steps to Reproduce
1. Replay the wire corpus.
2. CreateTable with LocalSecondaryIndexes and/or GlobalSecondaryIndexes (e.g. fixture dynamodb-pagination-contracts step setup#2/setup#3) and then DescribeTable: observe 400 ResourceNotFoundException instead of a table that reaches ACTIVE.
3. Any later PutItem/Query/DeleteTable on that table (dynamodb-pagination-contracts#task#8, #task#9, dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown#teardown#8) also answers 400.
4. dynamodb-index-key-validation-wording#task#3 and dynamodb-pagination-contracts#task#13 show the knock-on effect: the error class is ResourceNotFoundException where AWS raises ValidationException.
