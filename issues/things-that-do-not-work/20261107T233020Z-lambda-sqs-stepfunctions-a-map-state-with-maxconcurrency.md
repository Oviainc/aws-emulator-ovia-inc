Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency
Issue-Key: lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency|run#39|AssertionError|assert max_running >= 2, (
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# lambda__sqs__stepfunctions: a Map state with MaxConcurrency=2 runs its iterations serially — the maximum observed Lambda overlap is 1

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause, 1 failing check.

Cause 1 of 1: the Map state's MaxConcurrency is a ceiling the emulator never uses — iterations are executed one at a time, so no two Lambda invocations of the same Map ever overlap.
  AWS: recorded behavior only for the ASL semantics (Step Functions' Smithy model declares StartExecution/DescribeExecution and ExecutionStatus but not the Amazon States Language). The recording shows a state machine whose single Map state (ItemsPath $.items, MaxConcurrency 2, iterator = an arn:aws:states:::lambda:invoke Task over a handler that sleeps and records its start/end to a side-channel queue) running items CONCURRENTLY up to the cap: the per-invocation start/end records overlap, so the maximum observed concurrency is 2 for a cap of 2.
  Emulator: the execution now completes, but the recorded start/end windows never overlap — "the max observed overlap was 1". Every item is processed sequentially regardless of MaxConcurrency.
  Why: Map is the standard fan-out construct and MaxConcurrency is its only throughput control. A client sizing a workload on AWS's parallelism gets N times the wall-clock time, timeouts that only appear in the emulator, and cannot observe (or test) the bound it configured; any check of concurrency-sensitive behavior — throttling, ordering, partial failure under parallelism — is unreachable.
  Note: this is a net-new failure point. Open issue 20261107T081525Z-lambda-sqs-stepfunctions-3-net-new-causes-a.md, Cause 1, records this check failing EARLIER — the Map execution ending FAILED (its replay key is run#24, the SUCCEEDED assertion). The execution now succeeds and the divergence has moved to the scheduler's parallelism (line 224, max_running >= 2), which that issue's key does not carry.
  Fix: the Map state's iteration scheduler in the sfn service crate (services/service-com-amazonaws-sfn-awsstepfunctions, the interpreter's Map/MaxConcurrency driver) — it must run up to MaxConcurrency iterations at the same time rather than serializing them.
  Traces: lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency.

## Evidence
AssertionError: Expected at least 2 Lambda invocations to run concurrently under MaxConcurrency=2, but the max observed overlap was 1

Expected: with a Map state {"Type": "Map", "ItemsPath": "$.items", "MaxConcurrency": 2, "Iterator": {... "Resource": "arn:aws:states:::lambda:invoke" ...}} over several items and a handler that sleeps per item, the per-invocation start/end records recorded to the side-channel queue overlap for at least 2 invocations. The item count and sleep duration are per-check values; the requirement is that concurrent iterations occur up to the configured cap.

## Steps to Reproduce
1. Create a Lambda whose handler sleeps a few seconds and writes its invocation start and end timestamps to an SQS results queue.
2. CreateStateMachine with a single Map state: ItemsPath $.items, MaxConcurrency 2, iterator = a Task with Resource arn:aws:states:::lambda:invoke on that function.
3. StartExecution with several items; after it succeeds, read the results queue and compute the maximum number of overlapping [start,end] windows — the emulator yields 1, AWS yields 2.
