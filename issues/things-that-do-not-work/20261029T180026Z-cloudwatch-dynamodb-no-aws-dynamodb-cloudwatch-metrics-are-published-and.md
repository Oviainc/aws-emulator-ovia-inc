Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.conc-ddb-optimistic-refused-conditional-write-still-billed-rejected cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement cloudwatch__dynamodb.metrics-dynamodb-conditional-check-failure-still-consumes-write-capacity-rejected cloudwatch__dynamodb.metrics-dynamodb-dimension-sets-are-exact-per-metric-family cloudwatch__dynamodb.metrics-dynamodb-read-units-4kb-boundary-by-consistency-agree-with-cloudwatch cloudwatch__dynamodb.metrics-dynamodb-write-units-round-up-per-kb-agree-with-cloudwatch cloudwatch__dynamodb.obs-dynamodb-3-transaction-conflict-sum-equals-client-rejected-count cloudwatch__dynamodb.metrics-dynamodb-gsi-consumption-under-index-dimension-not-folded-into-table
Issue-Key: cloudwatch__dynamodb.conc-ddb-optimistic-refused-conditional-write-still-billed-rejected|_poll_until_at_least#14|Timeout|time.sleep(_POLL_S)
Issue-Key: cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement|_poll_until_stable#38|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-conditional-check-failure-still-consumes-write-capacity-rejected|_await_exact#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-dimension-sets-are-exact-per-metric-family|_await_exact#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-read-units-4kb-boundary-by-consistency-agree-with-cloudwatch|_await_exact#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-write-units-round-up-per-kb-agree-with-cloudwatch|_await_exact#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-3-transaction-conflict-sum-equals-client-rejected-count|_await_exact#23|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb|Query|NotImplemented
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# No AWS/DynamoDB CloudWatch metrics are published, and indexed Query is refused 501 (2 causes, 8 checks)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two net-new root causes in the cloudwatch__dynamodb group. Each needs its own fix.

Cause 1 of 2: DynamoDB never publishes its CloudWatch metrics, so every two-plane agreement check polls until its budget ends.
AWS: DynamoDB emits the `AWS/DynamoDB` namespace metrics (ConsumedReadCapacityUnits, ConsumedWriteCapacityUnits, ConditionalCheckFailedRequests, TransactionConflict, ...) with exactly the documented dimension sets (TableName, and TableName+GlobalSecondaryIndexName / +Operation per metric family), and the numbers agree with the ConsumedCapacity the data plane returned for the same calls.
Emulator: the metric data never appears; each check polls CloudWatch and ends with `Check timed out after 60.0s`. (An already-owned sibling observed the same thing as `TimeoutError: list_metrics(AWS/DynamoDB, ProvisionedWriteCapacityUnits, ...) exact dimension set: the run budget ended first`.)
Why: nothing that alarms or dashboards on DynamoDB works — no alarm can ever transition, no capacity or throttle metric is visible, and the two-plane numeric agreement (ConsumedCapacity vs CloudWatch) cannot be checked at all.
Fix: the dynamodb request path's metric emission into the cloudwatch metric store (namespace, metric names and dimension sets).
Traces: the seven timing-out checks below.

Cause 2 of 2: a Query on a GSI is refused 501 NotImplemented, so index-level metrics can never be produced.
AWS: `Query` with `IndexName` is served (Smithy: Query.IndexName -> IndexName @length 3..255, pattern ^[a-zA-Z0-9_.-]+$), and the consumption is attributed to the index dimension rather than folded into the table.
Emulator: HTTP 501 `NotImplemented` `operation Query with IndexName is scaffolded but not implemented in this emulator` — a status/code the DynamoDB model does not define, so no SDK error class matches.
Why: index reads cannot be performed, so the GSI dimension of the consumption metrics is unreachable and index cost attribution is impossible.
Fix: the Query handler's IndexName branch and the index-dimension metric attribution.
Traces: cloudwatch__dynamodb.metrics-dynamodb-gsi-consumption-under-index-dimension-not-folded-into-table

## Evidence
- Cause 1 (all seven): Error: Check timed out after 60.0s — cloudwatch__dynamodb.conc-ddb-optimistic-refused-conditional-write-still-billed-rejected, .ddb-consumed-capacity-two-plane-numeric-agreement, .metrics-dynamodb-conditional-check-failure-still-consumes-write-capacity-rejected, .metrics-dynamodb-dimension-sets-are-exact-per-metric-family, .metrics-dynamodb-read-units-4kb-boundary-by-consistency-agree-with-cloudwatch, .metrics-dynamodb-write-units-round-up-per-kb-agree-with-cloudwatch, .obs-dynamodb-3-transaction-conflict-sum-equals-client-rejected-count.
- Cause 2: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Query operation: operation Query with IndexName is scaffolded but not implemented in this emulator.

## Steps to Reproduce
Boot the emulator and replay the eight checks listed in trace_ids. Cause 1 is reproduced by driving any metered DynamoDB call and then polling CloudWatch for the AWS/DynamoDB metric; cause 2 by a single Query carrying IndexName.
