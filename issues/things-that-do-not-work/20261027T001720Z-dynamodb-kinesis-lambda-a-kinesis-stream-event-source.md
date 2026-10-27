Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# dynamodb__kinesis__lambda: a Kinesis-stream event source mapping never invokes its function, so a record put on the stream produces no Lambda invocation and the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause, grounded in a failing behavioral check the replay report marked neither ALREADY FILED nor OWNED. The emulator is serving (164/458 checks pass, bootstrap_signal false), so this is a fidelity divergence rather than a bootstrap gap.

=== Cause 1 of 1: a Kinesis-stream event source mapping delivers nothing to the function it maps ===
AWS: a Lambda event source mapping whose EventSourceArn is a Kinesis data stream polls that stream and invokes the mapped function with a batch of records once the mapping is Enabled; the function's own side effect (here, a DynamoDB write recording what it received) is the observable proof. The contract is that a record put on the stream becomes a function invocation within the poller's batching window.
Emulator: nothing is ever invoked. The check creates the stream, the function and the mapping, puts records on the stream and then waits for the function's effect to appear; it never reaches an assertion on the delivered payload because no invocation happens, and the harness cuts it off ('Error: Check timed out after 60.0s'). No error code is returned anywhere — the call path simply never converges.
Why: the whole stream-to-function integration is unusable. A client that wires a Kinesis stream to a function sees a healthy mapping and no processing at all, and because the failure is a silent non-delivery rather than a refusal there is nothing for it to catch or retry.
Fix: the Kinesis event-source poller in the lambda service crate — the task that reads the mapped stream's shards and invokes the function with a Kinesis batch event — together with whatever in the invoke path that poller depends on.
Note on existing issues: the OPEN issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the direct Invoke refusal and the OPEN issue 20261025T035202Z-cloudwatch-kinesis-lambda-createeventsourcemapping-never-authorizes-the-function.md covers the mapping's role authorization; the OPEN issue 20261026T005239Z-dynamodb-kinesis-lambda-a-dynamodb-streams-event-source.md covers the DynamoDB-Streams source's OnFailure destination. None of them carries this check's replay key or this (Kinesis-source) delivery path, and the replay report marked this check neither ALREADY FILED nor OWNED.
Traces: dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda.

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the booted emulator: 164/458 passed (35.8%), dynamodb__kinesis__lambda 0/2. bootstrap_signal was false.

dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda [mismatch]:
  Error: Check timed out after 60.0s.
  Expected: after CreateStream (waited to ACTIVE), CreateFunction and CreateEventSourceMapping with the stream as EventSourceArn, a PutRecord/PutRecords on the stream causes the function to run and its DynamoDB side effect to become visible within the check's budget.
  Actual: the function is never invoked and the side effect never appears; the check is cut off at the 60 s harness cap with no error code returned by any call.

## Steps to Reproduce
1. Build and boot the emulator, then replay dynamodb__kinesis__lambda.check-kinesis-stream-triggers-lambda.
2. kinesis:CreateStream and wait for ACTIVE; dynamodb:CreateTable for the function's side effect.
3. lambda:CreateFunction with a handler that writes one item per received record, then lambda:CreateEventSourceMapping(EventSourceArn=<stream arn>, FunctionName=<fn>, StartingPosition=TRIM_HORIZON) and confirm the mapping reports Enabled.
4. kinesis:PutRecord on the stream, then poll the DynamoDB table for the handler's item -> observe nothing is ever written and no invocation occurs.
