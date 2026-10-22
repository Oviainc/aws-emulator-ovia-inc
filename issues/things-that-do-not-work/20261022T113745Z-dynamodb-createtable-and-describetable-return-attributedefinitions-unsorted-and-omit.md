Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-table-lifecycle-with-gsi-and-ttl
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-table-lifecycle-with-gsi-and-ttl dynamodb-transactional-write-and-read-multi-item
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|2xx->2xx|$.TableDescription.AttributeDefinitions[*].AttributeName
Issue-Key: dynamodb|CreateTable|2xx->2xx|$.TableDescription.GlobalSecondaryIndexes[*].ProvisionedThroughput.NumberOfDecreasesToday
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.AttributeDefinitions[*].AttributeName
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.GlobalSecondaryIndexes[*].ProvisionedThroughput.NumberOfDecreasesToday
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.GlobalSecondaryIndexes[*].WarmThroughput
Issue-Key: dynamodb|DeleteTable|2xx->2xx|$.TableDescription.ItemCount
Issue-Key: dynamodb|DeleteTable|2xx->2xx|$.TableDescription.TableSizeBytes
Issue-Related: 20261022T113745Z-dynamodb-table-description-responses-omit-tableid-numberofdecreasestoday-and.md
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# CreateTable and DescribeTable return AttributeDefinitions unsorted and omit index WarmThroughput

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Table-shape echoes differ from AWS in how AttributeDefinitions are ordered, in the per-index throughput members they carry, and in whether item counters are allowed to be live. This bundle has 3 causes; all 3 must be closed.

Cause 1 of 3: CreateTable echoes AttributeDefinitions in request order instead of AWS's ordering, and omits per-index/table throughput detail.
AWS: Smithy: CreateTable -> output TableDescription; TableDescription.AttributeDefinitions is a list of AttributeDefinition {AttributeName, AttributeType}. The recording shows the definitions ordered by attribute name ('gsi_pk' before 'pk') regardless of the order they were supplied in, and shows GlobalSecondaryIndexes[0].ProvisionedThroughput.NumberOfDecreasesToday and TableId present.
Emulator: AttributeDefinitions come back in submission order ('pk' then 'gsi_pk'), and GlobalSecondaryIndexes[0].ProvisionedThroughput.NumberOfDecreasesToday, ProvisionedThroughput.NumberOfDecreasesToday and TableId are absent.
Why: a client that compares its desired table shape to the echoed shape (idempotent create, drift detection, infrastructure diffing) sees a spurious diff on every call and may attempt a pointless UpdateTable; a client reading index throughput detail gets nulls.
Fix: the CreateTable handler's TableDescription serializer (sort AttributeDefinitions deterministically as AWS does, and populate index + table throughput descriptions).
Traces: dynamodb-table-lifecycle-with-gsi-and-ttl#setup#1.

Cause 2 of 3: DescribeTable repeats the unsorted AttributeDefinitions and omits WarmThroughput on both the table and its GSI.
AWS: Smithy: DescribeTable -> Table: TableDescription; TableDescription.WarmThroughput targets TableWarmThroughputDescription, and GlobalSecondaryIndexDescription carries its own WarmThroughput and ProvisionedThroughput.NumberOfDecreasesToday. The recording shows WarmThroughput {ReadUnitsPerSecond, WriteUnitsPerSecond, Status ACTIVE} on both levels.
Emulator: AttributeDefinitions unsorted, and Table.WarmThroughput, Table.TableId, Table.ProvisionedThroughput.NumberOfDecreasesToday, GlobalSecondaryIndexes[0].WarmThroughput and GlobalSecondaryIndexes[0].ProvisionedThroughput.NumberOfDecreasesToday all missing.
Why: index-level capacity/warm-throughput introspection is impossible, and shape comparison against the create request fails.
Fix: the DescribeTable handler's TableDescription/GlobalSecondaryIndexDescription serializer and the index state it reads.
Traces: dynamodb-table-lifecycle-with-gsi-and-ttl#task#6.

Cause 3 of 3: DeleteTable's echo omits table metadata and reports live ItemCount/TableSizeBytes.
AWS: Smithy: DeleteTable -> output TableDescription; ItemCount and TableSizeBytes are documented as updated 'approximately every six hours', and the recording shows 0 even though one item had just been written.
Emulator: TableId, ProvisionedThroughput.NumberOfDecreasesToday, BillingModeSummary.LastUpdateToPayPerRequestDateTime and TableThroughputModeSummary are missing, and ItemCount is 1 / TableSizeBytes 51 instead of 0.
Why: clients that treat these counters as an exact, immediately consistent total build logic AWS will never support.
Fix: the DeleteTable handler's serializer plus the table state's counter refresh policy.
Traces: dynamodb-transactional-write-and-read-multi-item#teardown#10.

## Evidence
Representative: fixture dynamodb-table-lifecycle-with-gsi-and-ttl step setup#1, CreateTable, expected 200 / actual 200:
  $.TableDescription.AttributeDefinitions[0].AttributeName: value_mismatch (expected 'gsi_pk', got 'pk')
  $.TableDescription.AttributeDefinitions[1].AttributeName: value_mismatch (expected 'pk', got 'gsi_pk')
  $.TableDescription.GlobalSecondaryIndexes[0].ProvisionedThroughput.NumberOfDecreasesToday: missing (expected 0, got None)
  $.TableDescription.ProvisionedThroughput.NumberOfDecreasesToday / $.TableDescription.TableId: missing
Cause 2 (dynamodb-table-lifecycle-with-gsi-and-ttl#task#6): same ordering diff plus $.Table.WarmThroughput and $.Table.GlobalSecondaryIndexes[0].WarmThroughput missing (expected {'ReadUnitsPerSecond': 5, 'Status': 'ACTIVE', 'WriteUnitsPerSecond': 5}).
Cause 3 (dynamodb-transactional-write-and-read-multi-item#teardown#10): $.TableDescription.ItemCount expected 0 got 1; $.TableDescription.TableSizeBytes expected 0 got 51; TableId/NumberOfDecreasesToday/TableThroughputModeSummary missing.

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture dynamodb-table-lifecycle-with-gsi-and-ttl, step setup#1: CreateTable supplying AttributeDefinitions in the order pk, gsi_pk with a GSI; compare the echoed AttributeDefinitions order and index throughput members.
3. Step task#6: DescribeTable the same table and look for WarmThroughput on the table and on GlobalSecondaryIndexes[0].
4. Fixture dynamodb-transactional-write-and-read-multi-item, step teardown#10: write one item, DeleteTable, and observe ItemCount/TableSizeBytes are live rather than the lagging 0.
