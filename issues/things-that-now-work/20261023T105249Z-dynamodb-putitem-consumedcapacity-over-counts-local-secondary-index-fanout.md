Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|PutItem|2xx->2xx|$.ConsumedCapacity.CapacityUnits
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# PutItem ConsumedCapacity over-counts local secondary index fanout

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
1 cause.

Cause 1 of 1: PutItem on a table with local secondary indexes reports total ConsumedCapacity.CapacityUnits scaled by the number of indexes instead of the table's actual write cost.
AWS: Smithy: PutItem output member ConsumedCapacity targets ConsumedCapacity, whose CapacityUnits is 'the total number of capacity units consumed by the operation', returned when ReturnConsumedCapacity is requested. The recording shows the live service reporting CapacityUnits 1.0 for this write on an on-demand table that carries local secondary indexes - the top-level total is not the sum of a per-index fanout for this item.
Emulator: reports CapacityUnits 3.0 for the same request, i.e. the base write multiplied by the index count.
Why: ConsumedCapacity is what clients use to budget, throttle and cost-model their write path; reporting 3x the real cost makes capacity-aware writers back off or shard that they do not need to, and makes any assertion on consumed capacity in a caller's test suite fail against the emulator while passing against AWS.
Fix: the PutItem handler's consumed-capacity accounting - compute the top-level CapacityUnits from the item write itself rather than summing the per-index (Table/LocalSecondaryIndexes) breakdown into the total.
Traces: dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown#task#6.

## Evidence
Divergence 17 (PutItem, status 200->200): $.ConsumedCapacity.CapacityUnits: value_mismatch (expected 1.0, got 3.0). Fixture dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown, step task#6; 1 affected step.
Model: smithy_query(operation=PutItem) -> output {Attributes: AttributeMap, ConsumedCapacity: ConsumedCapacity, ItemCollectionMetrics: ItemCollectionMetrics}; ReturnConsumedCapacity is an input enum member.

## Steps to Reproduce
1. Replay fixture dynamodb-consumed-capacity-indexes-lsi-fanout-breakdown, step task#6: PutItem with ReturnConsumedCapacity on a table with local secondary indexes.
2. Compare $.ConsumedCapacity.CapacityUnits in the 200 body: emulator 3.0, recording 1.0.
