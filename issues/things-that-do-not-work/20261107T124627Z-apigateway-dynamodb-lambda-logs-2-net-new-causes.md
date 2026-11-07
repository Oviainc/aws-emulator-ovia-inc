Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive|run#17|AssertionError|assert overlap_found, f"no overlapping invocation intervals observed: {intervals}"
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# apigateway__dynamodb__lambda__logs: 2 net-new causes — a DynamoDB-stream ESM ignores ParallelizationFactor (invocations are strictly serial), and the missing-logs-permission ESM check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two net-new root causes in the apigateway__dynamodb__lambda__logs surface, grounded in two failing behavioral checks. A fixer must close BOTH before submitting.

=== Cause 1 of 2: a DynamoDB-Streams event source mapping with ParallelizationFactor > 1 invokes the function strictly serially ===

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice) `CreateEventSourceMapping$ParallelizationFactor` targets `ParallelizationFactor` (integer, @range min 1 max 10) and is documented as "(Kinesis and DynamoDB Streams only) The number of batches to process from each shard concurrently." So with ParallelizationFactor=8, BatchSize=1 and enough reserved concurrency, up to eight batches from one shard are in flight at the same time, and the function's invocation intervals OVERLAP in wall-clock time. The requirement is the overlap (concurrency), not any particular timestamp.

Emulator: the ESM delivers the records, but every invocation runs to completion before the next one starts — the recorded (start, end) intervals of the eight invocations are strictly disjoint and consecutive, each ~4 s long (the handler's sleep) and separated by a ~1.2 s gap, so no two intervals overlap. The check fails with `AssertionError: no overlapping invocation intervals observed: [...]`. ParallelizationFactor is accepted on the mapping and then has no effect on dispatch.

Why: a client that sets ParallelizationFactor to raise per-shard throughput gets none of it — processing latency for a burst scales linearly with the number of records instead of being divided by the factor, so throughput tests, backlog-drain estimates and any ordering assumption that depends on concurrent per-partition batches all behave differently than on AWS. Code that is written to be safe under concurrent batches is also never exercised.

Fix: the Lambda event-source-mapping poller/dispatcher for stream sources (DynamoDB Streams / Kinesis) — honour the mapping's ParallelizationFactor by dispatching up to that many batches per shard concurrently (bounded by the function's reserved concurrency) instead of invoking one batch at a time.

Traces: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive. Related but NOT owning this cause: open issues 20261029T180026Z-dynamodb-lambda-dynamodb-stream-esm-record-envelope-images-never-observable.md and 20261029T180026Z-cloudwatch-dynamodb-lambda-cloudwatch-lambda-ddb-stream-no-esm-metrics-cfn.md; neither carries a key for ParallelizationFactor concurrency.

=== Cause 2 of 2: the DynamoDB-stream ESM logging-permission boundary check never reaches a verdict ===

AWS: a DynamoDB-Streams-triggered Lambda whose execution role carries the stream read grants and a DynamoDB write grant but NO `logs:CreateLogGroup`/`logs:CreateLogStream`/`logs:PutLogEvents` still processes its records (the side-effect write lands) while producing no log group `/aws/lambda/<function>` and no log events; a sibling function with the basic logging policy attached logs the same trigger. Recorded behavior only (the coupling between a role's logs permissions and log delivery is not expressible in the Smithy model); the observable contract is: both functions' side effects appear, the permitted function's log events appear, and `DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<bad fn>")` stays empty.

Emulator: the check is killed at the harness's 60 s cap (`Check timed out after 60.0s`) — it never gets past waiting for both functions' side-effect markers to appear in the results table, so neither the positive control (log events for the permitted function) nor the absence assertion (no log group for the unpermitted one) is ever evaluated.

Why: a client cannot rely on the emulator to model the logs-permission boundary at all — it cannot observe that a function without logging permission is still invoked, nor that no log group is created for it, so IAM least-privilege work against the emulator gets no feedback, and the scenario as a whole does not complete.

Fix: the Lambda event-source-mapping delivery path for DynamoDB Streams together with the CloudWatch Logs delivery gate on the function's execution-role permissions — the two mapped functions must both be invoked promptly for the same stream record, and log-group/log-stream creation must be gated on the role's logs grants.

Traces: apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), service heading `### apigateway__dynamodb__lambda__logs`, 2 failing checks not owned by any open issue:

1) [mismatch] ddb-esm-parallelization-factor-multiple-shards-positive
   AssertionError: no overlapping invocation intervals observed: [(…800.60, …804.60), (…805.81, …809.81), (…811.01, …815.01), (…816.21, …820.21), (…821.42, …825.42), (…826.64, …830.64), (…831.84, …835.84), (…837.05, …841.05)]
   Setup: CreateEventSourceMapping(stream=<DDB stream ARN>, BatchSize=1, StartingPosition=TRIM_HORIZON, ParallelizationFactor=8); PutFunctionConcurrency(ReservedConcurrentExecutions=10); handler sleeps 4 s and records (start, end) per invocation; 8 items written to the table.
   Expected: at least two of the recorded invocation intervals overlap (concurrent per-shard batches). Actual: eight strictly disjoint, consecutive 4 s intervals — serial dispatch. The timestamps above are this run's clock values; the requirement is that intervals overlap.

2) [mismatch] ddb-esm-lambda-missing-logs-permission-negative
   Error: Check timed out after 60.0s
   Expected: both the logs-permitted and the logs-unpermitted function write their side-effect marker for the same stream record; the permitted function's /aws/lambda/<fn> log group has events; the unpermitted function's log group does not exist and has no events. Actual: the check never gets past the first wait for both markers and is killed at the harness cap, so no verdict on either leg.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: in the report find `### apigateway__dynamodb__lambda__logs` → `[mismatch] ddb-esm-parallelization-factor-multiple-shards-positive`. By hand: create a DynamoDB table with a stream, a results table, a role with stream-read + PutItem, a function that sleeps 4 s and PutItems (requestId, start, end); PutFunctionConcurrency(ReservedConcurrentExecutions=10); CreateEventSourceMapping(BatchSize=1, StartingPosition=TRIM_HORIZON, ParallelizationFactor=8); write 8 items; scan the results table and compare the (start, end) intervals. On AWS some overlap; on the emulator they are strictly serial.
3. Cause 2: replay ddb-esm-lambda-missing-logs-permission-negative. By hand: wire two functions to the same DynamoDB stream — one role with logs:CreateLogGroup/CreateLogStream/PutLogEvents, one without — write one item, and poll the results table for both functions' markers, then DescribeLogGroups for each function's /aws/lambda/<fn>. On AWS both markers appear, only the permitted function has a log group; on the emulator the scenario never completes.
