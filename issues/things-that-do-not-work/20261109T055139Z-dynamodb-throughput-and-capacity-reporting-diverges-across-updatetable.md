Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-update-table-throughput-mode-provisioned-round-trip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-update-table-throughput-mode-provisioned-round-trip dynamodb-ddb-gsi-six-index-table-round-trip dynamodb-ddb-wf-describe-omits-undeclared-optional-blocks dynamodb-vector-write-request-bytes-floor-and-per-dimension-cost
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.ProvisionedThroughput.LastIncreaseDateTime
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.WarmThroughput.ReadUnitsPerSecond
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.WarmThroughput.WriteUnitsPerSecond
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.WarmThroughput.ReadUnitsPerSecond
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.WarmThroughput.WriteUnitsPerSecond
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.OnDemandThroughput
Issue-Key: dynamodb|PutItem|2xx->2xx|$.ConsumedCapacity.VectorIndexes.idx-large.VectorWriteRequestUnits
Issue-Key: dynamodb|PutItem|2xx->2xx|$.ConsumedCapacity.VectorIndexes.idx-small.VectorWriteRequestUnits
Issue-Related: 20261104T061030Z-dynamodb-item-and-partiql-responses-return-wrong-items.md
Issue-Commit: bdb5184d827fec951c886c257ec0292728205109

# dynamodb throughput and capacity reporting diverges across UpdateTable, DescribeTable and PutItem

> _Note: this issue was identified against the code base at commit `bdb5184d827fec951c886c257ec0292728205109`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Four net-new root causes in how the emulator reports a table's throughput settings and an operation's consumed capacity. All four must be closed before submitting.

Cause 1 of 4: UpdateTable that raises provisioned capacity never records ProvisionedThroughputDescription.LastIncreaseDateTime.
AWS: Smithy: ProvisionedThroughputDescription.LastIncreaseDateTime (timestamp, doc "The date and time of the last provisioned throughput increase for this table."). The recording shows the member present in the TableDescription returned by an UpdateTable that moves the table to PROVISIONED capacity / raises its units. Its value is the time of that increase and is a per-run value, never a fixed one - the requirement is that the member exists and carries the time of the most recent increase.
Emulator: the member is absent.
Why: a client that enforces DynamoDB's capacity-decrease/increase cadence reads "never increased" and makes scaling decisions AWS would reject or throttle.
Fix: the table's throughput state (stamp the last-increase time when units rise) plus the UpdateTable/DescribeTable TableDescription serializer.
Traces: dynamodb-update-table-throughput-mode-provisioned-round-trip#task#4.

Cause 2 of 4: WarmThroughput is reported as the table's provisioned capacity units instead of its warm-throughput setting.
AWS: Smithy: WarmThroughput {ReadUnitsPerSecond, WriteUnitsPerSecond} (long), doc "Provides visibility into the number of read and write operations your table or secondary index can instantaneously support"; it is an independent UpdateTable/CreateTable input member, not a view of ProvisionedThroughput. The recording shows AWS reporting the table's warm-throughput values (a per-second instantaneous-support figure in the thousands) unchanged by a provisioned-capacity switch whose units are single digits.
Emulator: ReadUnitsPerSecond/WriteUnitsPerSecond equal to the table's provisioned RCU/WCU, i.e. it aliases WarmThroughput onto ProvisionedThroughput, on both UpdateTable and DescribeTable responses.
Why: a client pre-warming for a peak event reads a warm throughput orders of magnitude below the real one, so it either issues a redundant warm-throughput UpdateTable or concludes the table cannot absorb the event.
Fix: the table state must hold WarmThroughput separately (defaulted to the base-table warm-throughput floor, updated only by CreateTable/UpdateTable WarmThroughput), and the UpdateTable/DescribeTable serializers must read that, not ProvisionedThroughput.
Traces: dynamodb-update-table-throughput-mode-provisioned-round-trip#task#4 (UpdateTable), #task#6 (UpdateTable), #task#5 and #task#7 (DescribeTable).

Cause 3 of 4: DescribeTable invents an OnDemandThroughput block with the -1 removal sentinel on tables that declared no maximum.
AWS: Smithy: OnDemandThroughput.MaxReadRequestUnits / MaxWriteRequestUnits doc - "To specify a maximum OnDemandThroughput on your table, set the value ... as greater than or equal to 1. To remove the maximum OnDemandThroughput that is currently set on your table, set the value ... to -1." -1 is a request-side removal sentinel; the recording shows AWS omitting OnDemandThroughput from TableDescription entirely for a table with no configured maximum.
Emulator: TableDescription carries an OnDemandThroughput object whose MaxReadRequestUnits and MaxWriteRequestUnits are both the -1 sentinel.
Why: a client that round-trips DescribeTable into UpdateTable resends the removal sentinel as a configuration, and configuration-drift tooling reports a difference against a template that declares no maximum.
Fix: the DescribeTable/TableDescription serializer and the table state - do not materialise an OnDemandThroughput default; emit the member only when a maximum is actually set.
Note: open issue 20261108T011128Z-dynamodb-2-net-new-causes-describetable-invents-an.md describes this shape but does NOT carry this replay key, so this cause is unexercised by its pointer.
Traces: dynamodb-ddb-gsi-six-index-table-round-trip#task#3 (cluster also covers dynamodb-ddb-wf-describe-omits-undeclared-optional-blocks#task#3).

Cause 4 of 4: a write to a table carrying vector indexes reports no per-vector-index capacity and under-bills the total.
AWS: Smithy: ConsumedCapacity.VectorIndexes is a VectorIndexesCapacityMap keyed by IndexName whose value is VectorCapacity (doc "The amount of throughput consumed on each vector index affected by the operation"). The recording shows PutItem returning one VectorIndexes entry per affected vector index carrying a vector write-request-units figure, and a ConsumedCapacity.CapacityUnits / ConsumedCapacity.Table.CapacityUnits total that includes that vector-write cost on top of the base item write. Recorded behavior only for the member name: the recorded entries use VectorWriteRequestUnits, which the on-disk VectorCapacity shape does not model (it models VectorSearchRequestBytes/VectorWriteRequestBytes). The recorded figures scale with each index's dimension count and show a per-write floor for a small index.
Emulator: ConsumedCapacity carries no VectorIndexes map at all, and CapacityUnits / Table.CapacityUnits bill only the base item write, so the vector-write cost is omitted from the total.
Why: capacity-aware clients that throttle or forecast from ConsumedCapacity under-count vector writes by orders of magnitude and over-drive the table.
Fix: PutItem's capacity-metering component (and the vector-index state it must consult for each index's dimension count) plus the ConsumedCapacity serializer.
Traces: dynamodb-vector-write-request-bytes-floor-and-per-dimension-cost#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (242 fixtures, 2631 steps, score 90.5%); 4 of the 16 net-new divergence clusters:

1. dynamodb-update-table-throughput-mode-provisioned-round-trip#task#4 UpdateTable (200->200) - $.TableDescription.ProvisionedThroughput.LastIncreaseDateTime: missing (expected a timestamp, got None); $.TableDescription.WarmThroughput.ReadUnitsPerSecond: value_mismatch (expected the table's warm-throughput reads/s, got the provisioned RCU); $.TableDescription.WarmThroughput.WriteUnitsPerSecond: value_mismatch (expected the warm-throughput writes/s, got the provisioned WCU).
2. dynamodb-update-table-throughput-mode-provisioned-round-trip#task#5 DescribeTable (200->200) - $.Table.WarmThroughput.ReadUnitsPerSecond / WriteUnitsPerSecond: same aliasing onto provisioned units (cluster also covers #task#7).
3. dynamodb-update-table-throughput-mode-provisioned-round-trip#task#6 UpdateTable (200->200) - $.TableDescription.WarmThroughput.ReadUnitsPerSecond / WriteUnitsPerSecond: same aliasing.
4. dynamodb-ddb-gsi-six-index-table-round-trip#task#3 DescribeTable (200->200) - $.Table.OnDemandThroughput: unexpected_present (expected None, got {'MaxReadRequestUnits': -1, 'MaxWriteRequestUnits': -1}); 2 steps affected.
5. dynamodb-vector-write-request-bytes-floor-and-per-dimension-cost#task#3 PutItem (200->200) - $.ConsumedCapacity.VectorIndexes.<index>.VectorWriteRequestUnits: missing for every vector index (got None); $.ConsumedCapacity.CapacityUnits and $.ConsumedCapacity.Table.CapacityUnits: value_mismatch (expected the total including vector-write cost, got the base item-write cost only).

Model evidence: smithy_query operation CreateTable/UpdateTable and shape ProvisionedThroughputDescription/WarmThroughput/OnDemandThroughput/ConsumedCapacity/VectorIndexesCapacityMap/VectorCapacity on services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Causes 1 and 2: fixture dynamodb-update-table-throughput-mode-provisioned-round-trip, step task#4 - UpdateTable switching the table to PROVISIONED capacity; inspect TableDescription.ProvisionedThroughput.LastIncreaseDateTime and TableDescription.WarmThroughput. Then steps task#5/task#7 (DescribeTable) and task#6 (UpdateTable) for the same WarmThroughput aliasing.
4. Cause 3: fixture dynamodb-ddb-gsi-six-index-table-round-trip, step task#3 - DescribeTable on a table created without an OnDemandThroughput maximum; check whether the block is present.
5. Cause 4: fixture dynamodb-vector-write-request-bytes-floor-and-per-dimension-cost, step task#3 - PutItem with ReturnConsumedCapacity on a table with vector indexes of differing dimension counts; inspect ConsumedCapacity.VectorIndexes and the CapacityUnits totals.
