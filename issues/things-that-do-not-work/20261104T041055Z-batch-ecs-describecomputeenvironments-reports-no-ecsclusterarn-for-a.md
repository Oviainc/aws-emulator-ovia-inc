Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecs.batch-propagate-tags-copies-definition-and-job-tags-to-ecs-task-job-tag-wins
Issue-Key: batch__ecs.batch-propagate-tags-copies-definition-and-job-tags-to-ecs-task-job-tag-wins|run#13|KeyError|cluster_arn = _describe_ce(bat, ce_name)["ecsClusterArn"]
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# batch__ecs: DescribeComputeEnvironments reports no ecsClusterArn for a MANAGED compute environment, so the Batch-to-ECS task path cannot be reached

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeComputeEnvironments

## Description
Cause 1 of 1 — a MANAGED compute environment exposes no backing ECS cluster.
AWS: Smithy com.amazonaws.batch#ComputeEnvironmentDetail.ecsClusterArn — "The Amazon Resource Name (ARN) of the underlying Amazon ECS cluster that the compute environment uses." A MANAGED compute environment with the default ECS orchestration type is backed by a cluster Batch provisions, and DescribeComputeEnvironments reports that cluster's ARN; it is the documented handle a caller uses to find the ECS task a Batch job ran as (ListTasks/DescribeTasks on that cluster) and therefore the task's propagated tags.
Emulator: the member is absent — reading _describe_ce(...)["ecsClusterArn"] raises KeyError: 'ecsClusterArn' for a MANAGED compute environment the emulator itself created and reported.
Why: without the cluster ARN there is no way to get from a Batch job to its ECS task, so propagateTags behavior (job-definition tags and job tags copied onto the task, with the job tag winning on a key collision) is unobservable, as is anything else a client resolves through the backing cluster. Client code that indexes the modeled member gets a missing-key failure rather than an ARN.
Fix: managed-CE provisioning in CreateComputeEnvironment (record a backing ECS cluster in the batch crate's compute-environment state) and the DescribeComputeEnvironments serializer that must emit ecsClusterArn for MANAGED environments. The same absence is reported for service heading `batch` in this sweep; this cause is filed separately because it is grounded in a batch__ecs check with its own replay key.
Traces: batch__ecs.batch-propagate-tags-copies-definition-and-job-tags-to-ecs-task-job-tag-wins.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout; batch__ecs 0/1 passed.

check_batch__ecs_batch_propagate_tags_copies_definition_and_job_tags_to_ecs_task_job_tag_wins.py, line 512:
  cluster_arn = _describe_ce(bat, ce_name)["ecsClusterArn"]
  KeyError: 'ecsClusterArn'

Expected: DescribeComputeEnvironments on the MANAGED compute environment returns ecsClusterArn (arn:aws:ecs:<region>:<account-id>:cluster/<cluster name>), as real AWS does. Actual: the member is not present in the response.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. aws batch create-compute-environment --type MANAGED --state ENABLED --compute-resources '{"type":"FARGATE","maxvCpus":4,"subnets":[...],"securityGroupIds":[...]}' and wait for status VALID.
3. aws batch describe-compute-environments --compute-environments <name> -> the entry carries no ecsClusterArn; AWS returns the backing cluster's ARN.
4. Re-replay batch__ecs.batch-propagate-tags-copies-definition-and-job-tags-to-ecs-task-job-tag-wins.
