Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda.kinesis-esm-batch-size-caps-records-per-invoke kinesis__lambda.kinesis-esm-batching-window-gathers-more-records-per-invoke-than-an-immediate-mapping kinesis__lambda.kinesis-esm-default-maximum-retry-attempts-is-minus-one kinesis__lambda.kinesis-esm-tumbling-window-carries-state-and-closes-each-window-once
Issue-Key: kinesis__lambda.kinesis-esm-batch-size-caps-records-per-invoke|_poll#10|TimeoutError|raise TimeoutError(
Issue-Key: kinesis__lambda.kinesis-esm-batching-window-gathers-more-records-per-invoke-than-an-immediate-mapping|_poll#17|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting until {what}; last={last!r}")
Issue-Key: kinesis__lambda.kinesis-esm-default-maximum-retry-attempts-is-minus-one|_poll#17|TimeoutError|raise TimeoutError(f"timed out after {timeout_s:.0f}s waiting until {what}; last={last!r}")
Issue-Key: kinesis__lambda.kinesis-esm-tumbling-window-carries-state-and-closes-each-window-once|_poll#11|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting until {what}; last={last!r}")
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# A Kinesis event source mapping never delivers stream records to its function

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an event source mapping whose EventSourceArn is a Kinesis stream is created and reports an Active state, but the poller never invokes the function, so no record ever reaches the handler.

AWS: Smithy CreateEventSourceMapping accepts a Kinesis stream ARN with StartingPosition in EventSourcePosition (TRIM_HORIZON | LATEST | AT_TIMESTAMP), BatchSize 1..10000, MaximumBatchingWindowInSeconds 0..300, MaximumRetryAttempts -1..10000 (default -1 = infinite), TumblingWindowInSeconds 0..900. Once the mapping is Enabled and Active, Lambda polls the stream and invokes the function with a `Records` batch whose size is capped by BatchSize (and gathered for up to MaximumBatchingWindowInSeconds), carrying every record put to the stream from the starting position onward; tumbling-window mappings invoke per window and close each window exactly once.

Emulator: the mapping is created and settles, but nothing is ever invoked — every check times out with an empty observation: the batch-size cap is never exercised (sink forwarded 0 of 14 records), the batching-window mapping never drains its warmup record, a mapping with MaximumRetryAttempts=-1 never delivers its marker record, and no tumbling window is ever delivered or closed. Recorded symptom: TimeoutError/AssertionError with `last=[]` / `last=False` after 48–300 s.

Why: the Kinesis-to-Lambda path is one of the core event sources. A client that wires a stream to a function sees a healthy mapping that silently drops every record; nothing in the mapping's state or LastProcessingResult indicates that no poller exists, so the failure is invisible until data is missing.

Fix: the event-source-mapping poller for the Kinesis source (the mapping's runtime/state that reads shards and dispatches Invoke), together with the batching (BatchSize, MaximumBatchingWindowInSeconds) and tumbling-window accounting it must honor.

Traces: kinesis__lambda.kinesis-esm-batch-size-caps-records-per-invoke, kinesis__lambda.kinesis-esm-batching-window-gathers-more-records-per-invoke-than-an-immediate-mapping, kinesis__lambda.kinesis-esm-default-maximum-retry-attempts-is-minus-one, kinesis__lambda.kinesis-esm-tumbling-window-carries-state-and-closes-each-window-once.

## Evidence
kinesis-esm-batch-size-caps-records-per-invoke: TimeoutError: the sink has forwarded all 14 records: the run budget ended first; last=[]
kinesis-esm-batching-window-...: AssertionError: timed out after 180s waiting until the immediate ESM's poller to drain its warmup record; last=[]
kinesis-esm-default-maximum-retry-attempts-is-minus-one: TimeoutError: timed out after 48s waiting until the marker record reaches the function through the -1 mapping; last=False
kinesis-esm-tumbling-window-...: AssertionError: timed out after 300s waiting until every record is delivered and every window it fell in is closed; last=[]
(Record counts and wait budgets are the checks' parameters; the invariant is that zero records were delivered.)

## Steps to Reproduce
1. Create a Kinesis stream and a Lambda function.
2. CreateEventSourceMapping(EventSourceArn=<stream ARN>, FunctionName=<fn>, StartingPosition='TRIM_HORIZON', BatchSize=<n>) and wait for State=Enabled/Active.
3. PutRecords onto the stream.
4. Observe that the function is never invoked (no log stream, no side effect) however long you wait.
