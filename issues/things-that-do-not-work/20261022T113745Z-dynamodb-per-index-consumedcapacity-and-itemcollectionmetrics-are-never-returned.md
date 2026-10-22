Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown dynamodb-index-capacity-arms dynamodb-item-collection-metrics-size-estimate
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|PutItem|2xx->4xx|$.ConsumedCapacity
Issue-Key: dynamodb|GetItem|2xx->4xx|$.Item
Issue-Key: dynamodb|GetItem|2xx->4xx|status
Issue-Key: dynamodb|DeleteItem|2xx->4xx|$.ConsumedCapacity
Issue-Key: dynamodb|DeleteItem|2xx->4xx|status
Issue-Key: dynamodb|PutItem|2xx->4xx|$.ItemCollectionMetrics
Issue-Key: dynamodb|DeleteItem|2xx->4xx|$.ItemCollectionMetrics
Issue-Key: dynamodb|GetItem|2xx->4xx|$
Issue-Related: 20261022T113745Z-dynamodb-tables-created-with-secondary-indexes-never-materialize-later.md
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Per-index ConsumedCapacity and ItemCollectionMetrics are never returned by write operations

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Write operations that request ReturnConsumedCapacity=INDEXES or ReturnItemCollectionMetrics=SIZE return neither structure; on the index-bearing tables these probes use the request fails outright with 400. This bundle has 6 causes; all 6 must be closed.

Cause 1 of 6: PutItem does not return the per-index ConsumedCapacity fan-out.
AWS: Smithy: PutItem -> output ConsumedCapacity; ConsumedCapacity carries TableName, CapacityUnits, Table (Capacity) and LocalSecondaryIndexes / GlobalSecondaryIndexes maps ('the amount of throughput consumed on each local/global index affected by the operation'). The recording shows CapacityUnits 3.0 with Table 1.0 and one entry per LSI.
Emulator: status 400 and ConsumedCapacity absent.
Why: a client doing capacity accounting or cost attribution per index gets nothing, and the write itself fails.
Fix: PutItem's handler/serializer (capacity accounting that fans out over the table's indexes) on top of the table registry defect that makes indexed tables absent.
Traces: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown#task#3 (6 steps in cluster, incl. dynamodb-index-capacity-arms#task#3).

Cause 2 of 6: GetItem on the LSI fan-out table returns no Item and 400. AWS: Smithy: GetItem -> 200 with Item when the key exists. Emulator: 400, Item missing. Why: reads of any item written through an indexed table fail. Fix: table registry + GetItem handler. Traces: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown#task#7.

Cause 3 of 6: DeleteItem does not return per-index ConsumedCapacity. AWS: Smithy: DeleteItem -> output ConsumedCapacity with GlobalSecondaryIndexes and LocalSecondaryIndexes breakdowns (recording: CapacityUnits 3.0, gsi1 1.0, lsi1 1.0, Table 1.0). Emulator: 400, ConsumedCapacity absent. Why: as Cause 1 for deletes. Fix: DeleteItem capacity accounting. Traces: dynamodb-index-capacity-arms#task#7.

Cause 4 of 6: PutItem does not return ItemCollectionMetrics. AWS: Smithy: PutItem -> output ItemCollectionMetrics {ItemCollectionKey, SizeEstimateRangeGB}; the doc states it is returned when the request asks for it and the table has local secondary indexes. Recording: ItemCollectionKey {pk} and SizeEstimateRangeGB [0.0, 1.0]. Emulator: 400, ItemCollectionMetrics absent. Why: a client monitoring the 10GB item-collection limit for an LSI table loses its only signal. Fix: PutItem's ItemCollectionMetrics computation/serializer. Traces: dynamodb-item-collection-metrics-size-estimate#task#5.

Cause 5 of 6: DeleteItem does not return ItemCollectionMetrics. AWS/Emulator/Why as Cause 4, for DeleteItem (Smithy: DeleteItem -> output ItemCollectionMetrics). Fix: DeleteItem's ItemCollectionMetrics computation. Traces: dynamodb-item-collection-metrics-size-estimate#task#7.

Cause 6 of 6: GetItem on the item-collection table answers 400 where AWS returns an empty 200. AWS: Smithy: GetItem -> 200; a missing item yields 200 with no Item member (recording: empty body). Emulator: 400 {'__type': 'ResourceNotFoundException'}. Why: 'item absent' is indistinguishable from 'table absent'. Fix: table registry + GetItem handler. Traces: dynamodb-item-collection-metrics-size-estimate#task#13.

## Evidence
Representative: fixture dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown step task#3, PutItem, expected 200 / actual 400:
  $.ConsumedCapacity: missing (expected {'CapacityUnits': 3.0, 'LocalSecondaryIndexes': {'lsi1': {'CapacityUnits': 1.0}, 'lsi2': {'CapacityUnits': 1.0}}, 'Table': {'CapacityUnits': 1.0}, 'TableName': ...}, got None)
Other causes: DeleteItem 200->400 with ConsumedCapacity incl. GlobalSecondaryIndexes {'gsi1': 1.0} missing; PutItem and DeleteItem 200->400 with ItemCollectionMetrics {'ItemCollectionKey': {'pk': {'S': 'coll-a'}}, 'SizeEstimateRangeGB': [0.0, 1.0]} missing; GetItem 200->400 with body {'__type': 'ResourceNotFoundException', ...} where an empty 200 body was recorded. Capacity numbers are tied to the recorded items; the requirement is that the structures are present with a per-index breakdown.

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown, step task#3: PutItem with ReturnConsumedCapacity=INDEXES on an LSI table; observe 400 and no ConsumedCapacity.
3. Fixture dynamodb-item-collection-metrics-size-estimate, steps task#5 and task#7: PutItem/DeleteItem with ReturnItemCollectionMetrics=SIZE; observe no ItemCollectionMetrics.
4. Steps dynamodb-index-capacity-arms#task#7, dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown#task#7 and dynamodb-item-collection-metrics-size-estimate#task#13 for the remaining causes.
