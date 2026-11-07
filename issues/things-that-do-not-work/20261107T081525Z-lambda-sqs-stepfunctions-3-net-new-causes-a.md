Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency lambda__sqs__stepfunctions.sfn-task-token-sendtaskfailure-from-lambda-propagates-error lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth
Issue-Key: lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency|run#24|AssertionError|assert desc["status"] == "SUCCEEDED", f"Expected SUCCEEDED, got: {desc['status']}"
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-sendtaskfailure-from-lambda-propagates-error|run#9|AssertionError|assert desc["status"] == "SUCCEEDED", f"expected SUCCEEDED, got {desc['status']}: {desc.get('error')}"
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth|run#10|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# lambda__sqs__stepfunctions: 3 net-new causes — a Map state with MaxConcurrency fails the execution, the in-function SDK has no stepfunctions client (UnknownServiceError), and an SQS ESM never runs concurrent batches

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three root causes in this scenario service.

Cause 1 of 3: a Map state with `MaxConcurrency` fails the execution instead of succeeding.
AWS: recorded behavior only for the execution outcome (Step Functions' Smithy model declares `StartExecution`/`DescribeExecution` and the `ExecutionStatus` enum with `SUCCEEDED`/`FAILED`, but the Amazon States Language semantics are not modeled). The recording shows a state machine whose single `Map` state (`ItemsPath: $.items`, `MaxConcurrency: 2`, iterator = a `arn:aws:states:::lambda:invoke` Task) runs every item through the Lambda, bounded by the cap, and the execution settles `SUCCEEDED`; DescribeExecution reports no error.
Emulator: `DescribeExecution` reports `FAILED` for that execution, so the Map state never completes its iterations.
Why: Map is the standard fan-out construct; a client whose state machine uses it gets a failed execution for a definition AWS runs to completion, and the MaxConcurrency bound it depends on can never be observed.
Fix: the Step Functions interpreter's Map state implementation and its MaxConcurrency scheduler, plus the `states:::lambda:invoke` task integration it drives (the sfn service's execution/interpreter component).
Traces: lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency.

Cause 2 of 3: the in-process Lambda runtime's AWS SDK knows no `stepfunctions` service, so SendTaskFailure from a Lambda cannot propagate.
AWS: recorded behavior only for the runtime's SDK inventory. The recording shows a Lambda handler calling `boto3.client('stepfunctions').send_task_failure(taskToken=..., error=..., cause=...)` succeeding, and the waiting `.waitForTaskToken` Task surfacing that exact error/cause through its `Catch`, with the execution ending `SUCCEEDED` in the check's wiring.
Emulator: the execution ends `FAILED` with error `UnknownServiceError` — botocore inside the emulator's Lambda runtime cannot construct a `stepfunctions` client at all, so the callback is never sent.
Why: any function that calls Step Functions (send_task_success/send_task_failure, start_execution) fails with a client-construction error rather than doing its work; the callback pattern AWS documents is unusable from Lambda.
Fix: the Lambda runtime's bundled AWS SDK/service-model inventory must include `stepfunctions` and resolve its endpoint to the emulator (the lambda service's runtime/SDK provisioning component). Related but not covering: 20261105T081644Z-lambda-sqs-stepfunctions-no-aws-sdk-in-the.md owns other checks in this service; this check was not marked owned by it in this run.
Traces: lambda__sqs__stepfunctions.sfn-task-token-sendtaskfailure-from-lambda-propagates-error.

Cause 3 of 3: an SQS event source mapping never runs concurrent Lambda batches for a deep queue.
AWS: recorded behavior only. With a standard queue holding a deep backlog (tens of messages) and a handler that sleeps a few seconds per batch, the ESM scales out and runs multiple Lambda batches concurrently, which the check measures from per-invocation start/end records written to an independent results queue.
Emulator: those records never arrive and the check exhausts its budget (`Check timed out after 60.0s`), so no concurrent batches — and in fact no observable batch progress — occur.
Why: throughput of an SQS-triggered function is wrong by orders of magnitude; a client sizing a consumer on AWS's scale-out behavior sees a queue that is effectively not drained.
Fix: the Lambda event-source-mapping poller's scale-out/batching driver (the lambda service's ESM runtime component).
Traces: lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth.

## Evidence
Cause 1 — AssertionError: Expected SUCCEEDED, got: FAILED (check_lambda__sqs__stepfunctions_sfn_map_state_lambda_concurrency.py line 190), for a definition {"StartAt":"MapState","States":{"MapState":{"Type":"Map","ItemsPath":"$.items","MaxConcurrency":2,"Iterator":{..."Resource":"arn:aws:states:::lambda:invoke"...}}}}.
Cause 2 — AssertionError: expected SUCCEEDED, got FAILED: UnknownServiceError (check_lambda__sqs__stepfunctions_sfn_task_token_sendtaskfailure_from_lambda_propagates_error.py line 78); the handler's boto3 cannot build a 'stepfunctions' client.
Cause 3 — sqs-esm-concurrent-batches-scale-with-queue-depth: 'Check timed out after 60.0s' with MESSAGE_COUNT=30 and FUNCTION_SLEEP_S=3; the per-invocation start/end records never reach the results queue.

## Steps to Reproduce
1. Create a Lambda + role, an SQS side-channel queue, and a state machine whose single Map state (ItemsPath $.items, MaxConcurrency 2) invokes the Lambda per item; StartExecution with several items and poll DescribeExecution — status is FAILED, AWS reports SUCCEEDED.
2. Create a state machine with a .waitForTaskToken Task and a Lambda that calls boto3.client('stepfunctions').send_task_failure(...): the execution fails with UnknownServiceError.
3. Create a source queue, an SQS event source mapping, and a handler that sleeps 3s and records start/end to a results queue; send 30 messages and poll the results queue — nothing arrives, so no concurrent batches are observable.
