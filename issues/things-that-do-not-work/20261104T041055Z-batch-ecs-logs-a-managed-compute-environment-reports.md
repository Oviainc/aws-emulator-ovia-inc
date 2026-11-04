Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecs__logs.batch-fargate-job-runs-as-ecs-task-and-logs batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing
Issue-Key: batch__ecs__logs.batch-fargate-job-runs-as-ecs-task-and-logs|run#10|KeyError|ecs_cluster = ce["ecsClusterArn"]
Issue-Key: batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# batch__ecs__logs: a MANAGED compute environment reports no ecsClusterArn and Fargate jobs never settle, so no job ever runs as an ECS task or logs (2 causes)

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeComputeEnvironments

## Description
Two net-new behavioral root causes grounded in failing checks of the simparity-behavioral-aws-v1 replay. Both must be closed before submitting.

Cause 1 of 2 — DescribeComputeEnvironments omits ecsClusterArn for a MANAGED compute environment.
AWS: Smithy ComputeEnvironmentDetail.ecsClusterArn — "The Amazon Resource Name (ARN) of the underlying Amazon ECS cluster that the compute environment uses." A MANAGED (ECS-orchestrated) compute environment is backed by a cluster Batch provisions, and the ARN is reported once the CE exists; it is how a caller finds the ECS task a Batch job ran as, and from the task's awslogs configuration, the CloudWatch Logs stream its container wrote to.
Emulator: the member is absent — reading ce["ecsClusterArn"] raises KeyError: 'ecsClusterArn'.
Why: the Batch → ECS task → CloudWatch Logs chain starts at this ARN. Without it a client cannot locate the task or its log stream at all, and code that indexes the modeled member fails with a missing key instead of receiving an ARN.
Fix: managed-CE provisioning in CreateComputeEnvironment (record a backing ECS cluster in the batch crate's state) plus the DescribeComputeEnvironments serializer.
Traces: batch__ecs__logs.batch-fargate-job-runs-as-ecs-task-and-logs.

Cause 2 of 2 — a job whose container image tag does not exist never reaches a terminal status.
AWS: a Fargate job whose image tag cannot be resolved goes SUBMITTED → RUNNABLE → STARTING → FAILED with a statusReason naming the image pull, and writes nothing to the configured awslogs log group (no stream, no events).
Emulator: the job never settles — the check waiting for the terminal status exhausts the harness budget without FAILED ever being reported, so neither the failure nor the "and logs nothing" half of the contract can be evaluated.
Why: a client that submits work and waits for its outcome never gets one; an unresolvable image is indistinguishable from a job still waiting for capacity.
Fix: the job state machine/placement in the batch crate — advance the job, fail it with an image-pull statusReason, record the attempt, and route container output to the job definition's awslogs group only when a container actually ran.
Traces: batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout; batch__ecs__logs 0/2 passed.

Cause 1: check_batch__ecs__logs_batch_fargate_job_runs_as_ecs_task_and_logs.py, line 52: ecs_cluster = ce["ecsClusterArn"] -> KeyError: 'ecsClusterArn'. Expected the MANAGED CE's backing ECS cluster ARN, as real AWS returns.

Cause 2: Error: Check timed out after 60.0s — batch__ecs__logs.batch-job-with-missing-image-tag-fails-and-logs-nothing never observed the job reach FAILED.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: create a MANAGED FARGATE compute environment, then aws batch describe-compute-environments --compute-environments <name> -> no ecsClusterArn member.
3. Cause 2: register a FARGATE job definition whose image names a tag that does not exist, submit a job to an ENABLED queue on that CE, and poll aws batch describe-jobs --jobs <jobId> -> the status never becomes FAILED (AWS reports FAILED with an image-pull statusReason and no log stream).
4. Re-replay the check ids in trace_ids.
