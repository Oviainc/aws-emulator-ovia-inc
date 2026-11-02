Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement cloudwatch__dynamodb.metrics-dynamodb-conditional-check-failure-still-consumes-write-capacity-rejected cloudwatch__dynamodb.metrics-dynamodb-dimension-sets-are-exact-per-metric-family cloudwatch__dynamodb.metrics-dynamodb-read-units-4kb-boundary-by-consistency-agree-with-cloudwatch cloudwatch__dynamodb.metrics-dynamodb-write-units-round-up-per-kb-agree-with-cloudwatch cloudwatch__dynamodb.obs-dynamodb-3-transaction-conflict-sum-equals-client-rejected-count
Issue-Key: cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement|_poll_until_stable#38|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-conditional-check-failure-still-consumes-write-capacity-rejected|_await_exact#16|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-dimension-sets-are-exact-per-metric-family|_await_exact#22|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-read-units-4kb-boundary-by-consistency-agree-with-cloudwatch|_await_exact#16|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-write-units-round-up-per-kb-agree-with-cloudwatch|_await_exact#15|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-3-transaction-conflict-sum-equals-client-rejected-count|_await_exact#23|AssertionError|raise AssertionError(
Issue-Commit: b8358f56f4f44e240ca755ea7919bfa62f43cb3d

# cloudwatch__dynamodb: DynamoDB publishes no AWS/DynamoDB CloudWatch series at all — capacity, latency, item-count and conflict metrics are permanently empty (1 cause)

> _Note: this issue was identified against the code base at commit `b8358f56f4f44e240ca755ea7919bfa62f43cb3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: table activity produces no AWS/DynamoDB CloudWatch metrics, so every read of that namespace returns an empty series.
AWS: recorded behavior only (metric emission is not expressible in the Smithy model). A DynamoDB table emits AWS/DynamoDB metrics that CloudWatch serves through GetMetricStatistics/ListMetrics within roughly a minute of the API activity: ConsumedWriteCapacityUnits and ConsumedReadCapacityUnits (Sum and Maximum) under the `TableName` dimension set, SuccessfulRequestLatency and ReturnedItemCount under `TableName`+`Operation`, ConditionalCheckFailedRequests and TransactionConflict under `TableName`. The dimension sets are exact per family: capacity series exist under `{TableName}` and NOT under `{TableName, Operation}`, SuccessfulRequestLatency and ReturnedItemCount exist under `{TableName, Operation}` and NOT under `{TableName}` alone. The numbers agree with the API plane's own ConsumedCapacity accounting — writes rounded up per 1 KB, reads per 4 KB and halved when eventually consistent, a Query charged once over the total bytes returned, a transactional write charged double, and a refused conditional write charged its WCU even though its error body reports no ConsumedCapacity.
Emulator: after the same traffic, every one of those series reads back as no datapoints (`None`) and ListMetrics does not list ConsumedWriteCapacityUnits for the table at all. The API plane's in-band ConsumedCapacity is reported correctly (e.g. 20.0 WCU over 10 PutItems, 9.0 RCU over 8 GetItems), so only the CloudWatch emission is missing.
Why: nothing that reads DynamoDB through CloudWatch works — capacity dashboards, provisioned-throughput alarms, throttle and conditional-check-failure alarms, latency SLOs — and a client cannot distinguish "no traffic" from "metrics not emitted". Every two-plane reconciliation (in-band ConsumedCapacity vs the metric) is unverifiable.
Fix: the DynamoDB data-plane operations' metric emission into the CloudWatch metric store (the shared emitter that must publish AWS/DynamoDB datapoints under the exact dimension sets above), not CloudWatch's own read path — GetMetricStatistics serves custom namespaces correctly.
Traces: the six failing checks listed in trace_ids.

Related open issues that do NOT carry these replay keys and therefore do not cover these checks; the fixer should merge with them: 20261029T180026Z-cloudwatch-dynamodb-no-aws-dynamodb-cloudwatch-metrics-are-published-and.md, 20261031T180404Z-cloudwatch-dynamodb-aws-dynamodb-successfulrequestlatency-and-consumedwritecapacityunits-are-never-published.md, 20261029T065950Z-cloudwatch-dynamodb-no-aws-dynamodb-provisionedwritecapacityunits-metric-is.md.

## Evidence
Replayed against the emulator (suite service heading `### cloudwatch__dynamodb`). In each case the API plane reported the right numbers in-band and the CloudWatch series was empty:

- metrics-dynamodb-write-units-round-up-per-kb-agree-with-cloudwatch: 'API plane reported 20.0 WCU over 10 PutItems ... never equalled {ConsumedWriteCapacityUnits Sum: 20.0, ConsumedWriteCapacityUnits Maximum: 3.0, SuccessfulRequestLatency PutItem SampleCount: 10.0}; last reading {ConsumedWriteCapacityUnits Sum: None, ConsumedWriteCapacityUnits Maximum: None, SuccessfulRequestLatency PutItem SampleCount: None}'.
- metrics-dynamodb-read-units-4kb-boundary-by-consistency-agree-with-cloudwatch: 'API plane reported 9.0 RCU over 8 GetItems and 9.0 WCU ... last reading {ConsumedReadCapacityUnits Sum: None, ConsumedReadCapacityUnits Maximum: None, ConsumedWriteCapacityUnits Sum: None, SuccessfulRequestLatency GetItem SampleCount: None}'.
- metrics-dynamodb-dimension-sets-are-exact-per-metric-family: the whole 12-entry dimension matrix reads None, including the entries that must be present ('WCU {T}': 3.0, 'RCU {T}': 4.0, 'Latency {T,Op=PutItem} n': 3.0, 'ReturnedItemCount {T,Op=Query}': 3.0).
- metrics-dynamodb-conditional-check-failure-still-consumes-write-capacity-rejected: expected {ConsumedWriteCapacityUnits Sum: 7.0, ConditionalCheckFailedRequests Sum: 4.0, SuccessfulRequestLatency PutItem SampleCount: 3.0}; last reading all None.
- ddb-consumed-capacity-two-plane-numeric-agreement: 'AWS/DynamoDB consumed capacity for table ... never held one value for 60s; last reading was None'.
- obs-dynamodb-3-transaction-conflict-sum-equals-client-rejected-count: 'list_metrics ConsumedWriteCapacityUnits TableName=<table>: never equalled True ...; last reading False' — the metric is not even listed.

## Steps to Reproduce
1. Create an on-demand DynamoDB table.
2. PutItem ten items of sizes straddling the 1 KB boundary (e.g. 1024, 1025, 1536, 2048, 2049 bytes) with ReturnConsumedCapacity='TOTAL' and record the in-band ConsumedCapacity (20.0 WCU for that set).
3. GetItem the same items both eventually and strongly consistent, plus one Query and one Scan, recording the in-band RCU.
4. Issue a few conditional PutItems that fail the condition (ConditionalCheckFailedException).
5. Poll cloudwatch.get_metric_statistics(Namespace='AWS/DynamoDB', MetricName='ConsumedWriteCapacityUnits', Dimensions=[{'Name':'TableName','Value':<table>}], Statistics=['Sum','Maximum'], Period=60) over a window covering the traffic — expect datapoints matching the in-band totals, observe an empty Datapoints list.
6. Repeat for ConsumedReadCapacityUnits ({TableName}), SuccessfulRequestLatency and ReturnedItemCount ({TableName, Operation}), ConditionalCheckFailedRequests and TransactionConflict ({TableName}).
7. cloudwatch.list_metrics(Namespace='AWS/DynamoDB', MetricName='ConsumedWriteCapacityUnits', Dimensions=[{'Name':'TableName','Value':<table>}]) — expect the metric to be listed, observe it is not.
