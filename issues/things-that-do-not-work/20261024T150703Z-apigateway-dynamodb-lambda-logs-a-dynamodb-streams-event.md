Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# apigateway__dynamodb__lambda__logs: a DynamoDB-Streams event source mapping with ParallelizationFactor never processes any shard records (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: a DynamoDB-Streams event source mapping delivers no records to its Lambda ===
AWS: CreateEventSourceMapping over a DynamoDB stream polls the stream and invokes the target function with batches of records; with ParallelizationFactor > 1 several batches from the SAME shard are processed concurrently while per-partition-key order is preserved. The only observable proof is the function's own output — here, rows the handler writes to a results DynamoDB table, plus its log stream under /aws/lambda/<function>.
Emulator: no record is ever delivered. The check writes items across multiple shards and polls for the handler's results (its poll helpers return as soon as the predicate holds, so a correct emulator ends the check in one iteration); instead it runs to the suite's 60 s cap with nothing written and no log events. The emulator has no in-process Lambda runtime — open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records that lambda Invoke answers NotImplemented for a real deployment package — and the event-source poller produces no invocation attempt either.
Why: event source mappings are how most DynamoDB-stream consumers are built. A client gets a mapping that CreateEventSourceMapping/GetEventSourceMapping report as Enabled, a stream with records, and a function that is never called — with no error to show the pipeline is inert, so the concurrency/ordering semantics ParallelizationFactor exists for cannot be validated at all.
Fix: the Lambda event-source-mapping poller for DynamoDB streams (shard iteration, batching and ParallelizationFactor) and the Lambda invocation runtime it depends on.
Traces: apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive.

## Evidence
apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive: 'Check timed out after 60.0s'. The check creates the table with a stream, a handler (which sleeps 4 s and writes a row per batch to RESULTS_TABLE), an event source mapping with a ParallelizationFactor, writes items across shards, then polls (timeout_s=180/120/90/60 helpers that return on the first satisfying reading) for the result rows — none appeared. Expected: result rows proving several concurrent batches per shard, and the handler's events in /aws/lambda/<function>.

## Steps to Reproduce
dynamodb CreateTable with StreamSpecification (NEW_AND_OLD_IMAGES) and a results table; create a Lambda whose handler writes one row per invocation to the results table; lambda CreateEventSourceMapping(EventSourceArn=<stream ARN>, FunctionName=<fn>, StartingPosition='TRIM_HORIZON', ParallelizationFactor=N); PutItem several items across partition keys; poll the results table (and /aws/lambda/<fn> log streams) — both stay empty.
