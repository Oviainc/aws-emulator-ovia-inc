Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing
Issue-Key: batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing|run#21|AssertionError|assert "CannotPullContainerError" in diagnosis, (
Issue-Commit: afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b

# batch__ecs__logs: a job whose image tag does not exist is FAILED with the generic no-container-runtime reason instead of CannotPullContainerError, so a bad image reference is indistinguishable from any other failure (1 cause)

> _Note: this issue was identified against the code base at commit `afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobs

## Description
One net-new behavioral root cause grounded in a failing check of the simparity-behavioral-aws-v1 replay (batch__ecs__logs 0/2 passed; the other failure is OWNED by 20261104T041055Z-batch-ecs-logs-a-managed-compute-environment-reports.md and is not part of this issue). Related open issue, named for the fixer: that same 20261104T041055Z issue records "Fargate jobs never settle" — they now DO settle, as FAILED, so what remains is the REASON they settle with, recorded here.

Cause 1 of 1 — the attempt's failure reason does not distinguish an unpullable image from anything else.
AWS: Smithy com.amazonaws.batch#AttemptContainerDetail.reason is "A short (255 max characters) human-readable string to provide additional details for a running or stopped container", and JobDetail.statusReason is the job-level counterpart. For a FARGATE job whose container image names a tag that does not exist in the registry, Batch places the ECS task, the ECS agent's image pull fails, and the job is FAILED with the attempt's container reason beginning "CannotPullContainerError: " and naming the image reference the pull was attempted with (the fixed text is the error class name "CannotPullContainerError"; the variable part is the image reference taken from the job definition's containerProperties.image, plus the registry's own not-found wording). AWS documents this failure under awslogs/Batch troubleshooting (https://docs.aws.amazon.com/batch/latest/userguide/using_awslogs.html, referenced by the check): the awslogs stream for the attempt exists but stays empty, because no process ever wrote to stdout.
Emulator: both the job's statusReason and the attempt's container reason are the single fixed string "This emulator does not run containers, so the job's container was never started" — the same text a job with a perfectly valid image gets. The image reference is never evaluated, so the pull failure is never reported, and no awslogs stream is created either.
Why: the failure reason is the only thing a client has to diagnose a job that will not run. With one fixed reason for every cause, an operator cannot tell a typo'd image tag from a capacity problem, a permissions problem or a crashed container; tooling that matches "CannotPullContainerError" to surface "your image does not exist" never fires, and the differential against a control job that differs only in the tag collapses (both jobs report the identical reason).
Fix: the batch crate's job placement/lifecycle state machine — the placement must evaluate the container image reference against the registry and fail the attempt with the pull reason, projected into JobDetail.statusReason and attempts[].container.reason; the awslogs wiring must still create the attempt's (empty) stream.
Traces: batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[mismatch] batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing
AssertionError: expected CannotPullContainerError for the missing tag public.ecr.aws/docker/library/busybox:sim2real-no-such-tag, got: "This emulator does not run containers, so the job's container was never started | This emulator does not run containers, so the job's container was never started" (the two halves are JobDetail.statusReason and the attempt's container reason).

Expected (AWS): status FAILED with a reason whose fixed text is "CannotPullContainerError" and whose variable part names the image reference from the job definition (here public.ecr.aws/docker/library/busybox:<nonexistent tag>); the attempt's awslogs stream exists and is empty, while a control job differing only in the tag runs and delivers its stdout.

The emulator's string is the batch crate's documented NO_CONTAINER_RUNTIME_REASON (services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs lines 29-42).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a log group and an ECS-tasks execution role with ecr:GetAuthorizationToken/BatchGetImage/GetDownloadUrlForLayer/BatchCheckLayerAvailability and logs:CreateLogStream/PutLogEvents.
3. Create a MANAGED FARGATE compute environment and an ENABLED job queue in front of it.
4. Register two FARGATE job definitions with an awslogs logConfiguration naming the log group: one whose image is a valid busybox tag, one whose image tag does not exist.
5. aws batch submit-job for the bad-tag definition and poll aws batch describe-jobs --jobs <jobId> to a terminal status, then read statusReason and attempts[0].container.reason.
   Observed: both are "This emulator does not run containers, so the job's container was never started" — identical to the control job's. AWS: a reason beginning "CannotPullContainerError" naming the image.
6. Re-replay the check id in trace_ids.
