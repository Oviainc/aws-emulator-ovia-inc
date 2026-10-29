Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive|_poll_until#15|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 595d3d5604be8cb7f050ded6602d61000805e4f4

# apigateway__dynamodb__lambda__logs: a DynamoDB-Streams event source mapping with ParallelizationFactor delivers no Lambda invocations at all, so no invocation record is ever written

> _Note: this issue was identified against the code base at commit `595d3d5604be8cb7f050ded6602d61000805e4f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause, grounded in one failing behavioral check that the replay report marks neither ALREADY FILED nor OWNED. Cause count: 1.

=== Cause 1 of 1: a DynamoDB-Streams event source mapping reported Enabled never delivers any batch to its Lambda, so increasing ParallelizationFactor produces no concurrent invocations and no invocation records ===
AWS: Smithy (com.amazonaws.lambda#CreateEventSourceMapping) accepts EventSourceArn (a DynamoDB stream ARN), FunctionName, BatchSize, StartingPosition and ParallelizationFactor; GetEventSourceMapping reports State, which reaches "Enabled". ParallelizationFactor is documented as the number of batches to process concurrently from each shard. Recorded behavior (the model does not spell delivery timing): once the mapping is Enabled, every item written to the source table produces a stream record that the mapping delivers to the function, and with ParallelizationFactor > 1 several invocations for the same shard run CONCURRENTLY — observable as overlapping [start, end] execution intervals that the invocations themselves record. Delivery for a cold DynamoDB-stream mapping is observed within roughly 60-120 s.
Emulator: nothing is ever delivered. After the source table, the results table, the execution role, the function and the event source mapping are all created and the mapping is reported Enabled, a scan of the results table never reaches even one invocation record — the check's poll for >= 8 records never satisfies and dies with TimeoutError("results table has >=8 invocation records: the run budget ended first; not graded"). Because zero records arrive, the overlap assertion that is the point of the check is never reached: ParallelizationFactor's effect cannot be observed at all, and neither can the baseline that a single invocation happens.
Why: a client that wires a DynamoDB stream to a Lambda gets a mapping that reports itself healthy (State Enabled) and silently processes nothing. Stream-driven architectures — change-data-capture, aggregation, fan-out, audit trails — appear correctly configured and produce no effect, with no error anywhere to attribute it to; tuning ParallelizationFactor, BatchSize or starting position has no observable consequence.
Why: additionally, a caller cannot distinguish "my function failed" from "nothing was ever delivered", because there are no invocation records and no failure signal on the mapping.
Fix: the Lambda event-source-mapping poller for DynamoDB Streams and its invoke path — an Enabled mapping must read the source stream's shards, assemble batches honouring BatchSize, and invoke the target function, running up to ParallelizationFactor concurrent invocations per shard so their execution intervals genuinely overlap; GetEventSourceMapping's State/LastProcessingResult must reflect that polling actually runs rather than only that the record was stored.
Related open issue that does NOT carry this check id: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (lambda Invoke refusing real invocations) is a plausible upstream cause but is grounded in different checks and does not describe event-source-mapping delivery or ParallelizationFactor; if a fixer closes both together, this check id must still be re-replayed.
Traces: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive.

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1 (service group apigateway__dynamodb__lambda__logs: 2/13 passed).

  [error] apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive
  File "check_apigateway__dynamodb__lambda__logs_ddb_esm_parallelization_factor_multiple_shards_positive.py", line 154, in _wait_for_effect
    return _poll_until(
  File "...", line 143, in _poll_until
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: results table has >=8 invocation records: the run budget ended first; not graded

Sequence, all accepted before the hang: dynamodb CreateTable for the stream source (StreamSpecification enabled) and a plain results table; iam CreateRole granting dynamodb:GetRecords/GetShardIterator/DescribeStream; lambda CreateFunction; lambda CreateEventSourceMapping(EventSourceArn=<stream arn>, FunctionName=<fn>, BatchSize=..., StartingPosition=..., ParallelizationFactor=<n>, Enabled=True) followed by a poll on GetEventSourceMapping(UUID).State until 'Enabled' — which succeeds; then items are written to the source table and the results table is scanned.

Expected: each written item is delivered to the function, which records a [start, end] interval row in the results table; at least 8 such rows appear within the 180 s budget and at least two of their intervals overlap, demonstrating ParallelizationFactor concurrency. Actual: the results-table scan returns fewer than 8 rows for the whole budget (the check's predicate also accepts an observed overlap, and neither ever occurs), i.e. no delivery is observable at all; the overlap assertion `assert overlap_found` is never reached.

## Steps to Reproduce
Boot the emulator and point botocore at it.
  1. dynamodb CreateTable SRC with a hash key and StreamSpecification={'StreamEnabled': True, 'StreamViewType': 'NEW_AND_OLD_IMAGES'}; note LatestStreamArn. CreateTable RES (plain) for invocation records.
  2. iam CreateRole assumable by lambda.amazonaws.com with dynamodb:GetRecords, dynamodb:GetShardIterator, dynamodb:DescribeStream on the stream and write access to RES.
  3. lambda CreateFunction for a handler that, for each record in the batch, writes a row to RES holding the invocation's start and end epoch-millis.
  4. lambda CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<LatestStreamArn>, BatchSize=1, StartingPosition='LATEST', ParallelizationFactor=<n greater than 1>, Enabled=True); poll GetEventSourceMapping(UUID=<uuid>)['State'] until 'Enabled' (this succeeds).
  5. PutItem several items into SRC, then Scan RES repeatedly for up to 180 s.
     -> OBSERVE RES stays empty: no invocation record ever appears, and GetEventSourceMapping reports no failure either.
     -> EXPECT one row per delivered record within ~60-120 s, at least 8 rows for 8 written items, and at least two rows whose [start, end] intervals overlap (the ParallelizationFactor effect).
  6. Re-replay the check id in trace_ids to confirm the fix.
