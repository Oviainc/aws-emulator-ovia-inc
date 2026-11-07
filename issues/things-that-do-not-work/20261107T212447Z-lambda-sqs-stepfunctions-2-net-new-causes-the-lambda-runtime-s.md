Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth
Issue-Key: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative|_poll_invoke_until#18|AssertionError|raise AssertionError(
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative|_run_until_succeeded#7|AssertionError|raise AssertionError(f"no execution reached SUCCEEDED within {deadline_s}s (last={last!r})")
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth|run#13|AssertionError|assert max_concurrency >= 3, (
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 2 net-new causes: the Lambda runtime's bundled botocore knows no 'stepfunctions' client, and the SQS ESM never scales with queue depth

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
Two net-new root causes. Open issues 20261105T081644Z-lambda-sqs-stepfunctions-no-aws-sdk-in-the.md and 20261107T044335Z-lambda-sqs-stepfunctions-waitfortasktoken-callbacks-never-resume-the.md cover other checks in this service; neither cause below carries their replay keys.

Cause 1 of 2 — `boto3.client("stepfunctions")` inside a managed Python Lambda raises UnknownServiceError, so a handler can neither start an execution nor return a task token.
AWS: recorded behavior only. The managed python3.x runtimes ship a complete boto3/botocore, so `boto3.client("stepfunctions").start_execution(...)` / `.send_task_success(taskToken=..., output=...)` resolve the service model and are then served or refused by IAM with AccessDeniedException; a waitForTaskToken Task state resumes when SendTaskSuccess arrives.
Emulator: the client cannot be constructed. Replay details: `AssertionError: the granted lambda role starts the execution not observed within 90.0s. Last result: {'status_code': 200, 'payload': {'ok': False, 'error': "Unknown service: 'stepfunctions'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager"}}`; and an execution driven by such a handler ends FAILED with `error: 'UnknownServiceError'`, `cause: {"errorMessage":"Unknown service: 'stepfunctions'. ...","errorType":"UnknownServiceError",...}` raised from `botocore/client.py` `_raise_refusal`.
Why: a handler can neither launch a workflow nor complete a callback task, so waitForTaskToken workflows hang and orchestration from inside Lambda is impossible. The failure is an SDK-construction error, so a client cannot distinguish it from a missing states:StartExecution / states:SendTaskSuccess grant, and the duplicate-SendTaskSuccess refusal rule can never be exercised.
Fix: the in-process Lambda Python runtime's bundled botocore service catalogue — the `stepfunctions` (states) service model must be resolvable so calls reach the emulator's Step Functions handlers and are authorized there.
Traces: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative, lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative.

Cause 2 of 2 — an SQS event source mapping processes a backlog serially instead of scaling out with queue depth.
AWS: recorded behavior only. An SQS event source mapping scales the number of concurrent batch consumers as the queue's backlog grows, so draining a deep queue produces overlapping invocations of the target function.
Emulator: exactly one invocation runs at a time. Replay detail: `AssertionError: observed max concurrent invocations=1, expected the ESM to scale out batch consumption as queue depth (30 messages) grew, not process serially`.
Why: a client's queue drain rate is capped at one batch at a time no matter how deep the backlog, so throughput targets and latency SLOs built on SQS-to-Lambda scaling are unattainable and queues grow without bound under load — with no configuration the client can change to fix it.
Fix: the SQS event-source-mapping poller's scaling policy — grow concurrent batch consumers with backlog depth, bounded by ScalingConfig.MaximumConcurrency and the function's reserved concurrency.
Traces: lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth.

## Evidence
Cause 1: expected states:StartExecution / SendTaskSuccess from a python3.12 handler to be served or refused with AccessDeniedException; actual errorType 'UnknownServiceError', errorMessage "Unknown service: 'stepfunctions'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager", and the execution ends FAILED with error 'UnknownServiceError'.
Cause 2: expected concurrent batch consumers to grow with a 30-message backlog; actual max concurrent invocations = 1.

## Steps to Reproduce
Cause 1: CreateFunction (python3.12) whose handler calls boto3.client("stepfunctions").start_execution(...) or .send_task_success(...) and Invoke it (directly, and as a waitForTaskToken Task target) — the call fails with UnknownServiceError.
Cause 2: CreateEventSourceMapping on an SQS queue, send 30 messages at once, and record overlapping invocations of the target function — never more than 1.
