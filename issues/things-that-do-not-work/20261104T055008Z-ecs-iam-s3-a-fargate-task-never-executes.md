Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired
Issue-Key: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired|_wait_stopped#22|Timeout|assert writer.get("exitCode") == 0, (
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# ecs__iam__s3: a Fargate task never executes, so a task-role denial and its repair are never observable

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: RunTask on a Fargate task definition never produces a running container, so the task's own S3 calls never happen.
AWS: recorded behavior only — RunTask with launchType FARGATE starts the task, the container makes its S3 call under the task role, the task reaches STOPPED with an exit code, and the attempt is observable: with no s3:PutObject grant the write is denied and no object lands; after the task role is repaired a re-run writes the object.
Emulator: the task never runs, so neither the denied attempt nor the repaired write is observable and the check exhausts its budget with 'Check timed out after 60.0s'.
Why: task-role authorization — the one thing this check distinguishes — cannot be observed at all; a client polling DescribeTasks for a terminal state or polling S3 for the task's output waits out its entire timeout.
Fix: the ECS task runtime in the ecs service crate (the RunTask execution path and the task-role credential it must hand the container), and the task lifecycle DescribeTasks reports.
Traces: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired

## Evidence
Replay detail: 'Error: Check timed out after 60.0s'. Expected: the task reaching a terminal state with its S3 attempt refused (AccessDenied, no object created), then a repaired run writing the object. Actual: the polled condition is never reached.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite. 2. Replay ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired. 3. Directly: register a Fargate task definition whose container writes to S3, RunTask with a task role lacking s3:PutObject, then poll DescribeTasks and the bucket.
