Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__logs.batch-array-child-index-drives-distinct-stdout batch__logs.batch-sequential-array-child-failure-leaves-remaining-children-blocked batch__logs.batch-sequential-array-children-run-one-at-a-time-in-index-order batch__logs.batch-job-stdout-matches-computed-output batch__logs.batch-awslogs-stream-prefix-derives-stream-name-from-prefix-and-ecs-task-id batch__logs.batch-attempt-timeout-kills-container-before-completion-marker batch__logs.batch-container-memory-limit-kills-job-before-completion-marker batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it batch__logs.batch-retry-attempt-number-visible-to-container-and-second-attempt-succeeds batch__logs.batch-terminate-running-job-stops-container-before-completion-marker
Issue-Key: batch__logs.batch-job-stdout-matches-computed-output|_poll_until#10|Timeout|return last
Issue-Key: batch__logs.batch-awslogs-stream-prefix-derives-stream-name-from-prefix-and-ecs-task-id|_poll_until#13|Timeout|return last
Issue-Key: batch__logs.batch-attempt-timeout-kills-container-before-completion-marker|_poll_until#12|Timeout|return last
Issue-Key: batch__logs.batch-container-memory-limit-kills-job-before-completion-marker|_poll_until#13|Timeout|return last
Issue-Key: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it|_poll_until#6|Timeout|return last
Issue-Key: batch__logs.batch-retry-attempt-number-visible-to-container-and-second-attempt-succeeds|_poll_until#8|Timeout|return last
Issue-Key: batch__logs.batch-terminate-running-job-stops-container-before-completion-marker|_poll_until#10|Timeout|return last
Issue-Related: 20261104T041055Z-batch-array-jobs-unimplemented-managed-ces-have-no.md
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# batch__logs: array-job submission is unimplemented and container jobs never run, so no Batch job ever writes a CloudWatch Logs stream (2 causes)

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Two net-new behavioral root causes grounded in failing checks of the simparity-behavioral-aws-v1 replay (batch__logs 0/10). Both must be closed before submitting.

Cause 1 of 2 — SubmitJob with arrayProperties is refused NotImplemented, so array children never run and never log.
AWS: Smithy com.amazonaws.batch#SubmitJob, POST /v1/submitjob, success 200; the optional input member `arrayProperties` targets ArrayProperties{size:Integer}; the only modeled errors are ClientException (400) and ServerException (500) — NotImplemented is not a modeled Batch error. AWS answers 200 {jobArn,jobName,jobId}, creates `size` children, gives each child ArrayPropertiesDetail.index, exports AWS_BATCH_JOB_ARRAY_INDEX into its container, and each child writes its own awslogs stream.
Emulator: the submission is refused — ClientError (NotImplemented) "operation SubmitJob with arrayProperties is scaffolded but not implemented in this emulator".
Why: per-index container output, sequential (size-ordered) child execution and the blocked-children-after-a-child-failure contract are all unreachable, and the client sees an error class AWS never produces for a modeled member.
Fix: the arrayProperties branch of the SubmitJob handler in the batch crate (child-job creation, per-child index, and the child's log stream).
Note on the teardown noise these checks also print ("could not delete internet gateway igw-... detach from vpc-...: NotImplemented"): that is EC2 DetachInternetGateway/DeleteInternetGateway and is already filed as open issues 20261103T215504Z-ec2-detachinternetgateway-stub.md and 20261103T215504Z-ec2-deleteinternetgateway-stub.md — it is not part of this issue.
Traces: batch__logs.batch-array-child-index-drives-distinct-stdout, batch__logs.batch-sequential-array-child-failure-leaves-remaining-children-blocked, batch__logs.batch-sequential-array-children-run-one-at-a-time-in-index-order.

Cause 2 of 2 — a submitted container job never runs, so nothing is ever written to CloudWatch Logs.
AWS: after SubmitJob returns 200 Batch places the job and runs its container without further client calls; the job reaches SUCCEEDED or FAILED, DescribeJobs reports the attempt's container logStreamName, and the container's stdout appears as events in the job definition's awslogs group (with the stream name derived from the awslogs-stream-prefix and the ECS task id). Termination, an attempt timeout and a container memory limit each kill the container before its completion marker is written; a maxvCpus cap holds the second job RUNNABLE and then runs it; a retried attempt sees an incremented attempt number.
Emulator: none of this is observable — seven checks that wait for a job to run, be terminated, be killed or write a log stream exhaust the harness budget with no terminal status and no log events. (Same defect family as the "submitted jobs never reach a terminal status" cause filed for service heading `batch` in this sweep; filed here because these checks are attributed to batch__logs and carry their own replay keys.)
Why: the entire observable output of Batch — exit status, logs, stream naming, resource limits, termination — is missing. A client that submits work and tails its logs gets neither an outcome nor a stream.
Fix: the job scheduler/state machine in the batch crate (advance the job, run/kill the container, record attempts) and the awslogs wiring that must create the stream in the configured CloudWatch Logs group.
Traces: batch__logs.batch-job-stdout-matches-computed-output, batch__logs.batch-awslogs-stream-prefix-derives-stream-name-from-prefix-and-ecs-task-id, batch__logs.batch-attempt-timeout-kills-container-before-completion-marker, batch__logs.batch-container-memory-limit-kills-job-before-completion-marker, batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it, batch__logs.batch-retry-attempt-number-visible-to-container-and-second-attempt-succeeds, batch__logs.batch-terminate-running-job-stops-container-before-completion-marker.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout; batch__logs 0/10 passed.

Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SubmitJob operation: operation SubmitJob with arrayProperties is scaffolded but not implemented in this emulator (batch-array-child-index-drives-distinct-stdout), and the same ClientError surfaced through the two sequential-array checks: AssertionError: ClientError('An error occurred (NotImplemented) when calling the SubmitJob operation: operation SubmitJob with arrayProperties is scaffolded but not implemented in this emulator').

Cause 2: Error: Check timed out after 60.0s for each of the seven run/log checks — no job reached a terminal status and no log stream or event was observed in the configured log group.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: aws batch submit-job --job-name j --job-queue <enabled queue> --job-definition <jd> --array-properties size=3 -> NotImplemented; AWS answers 200 and creates 3 children, each writing its own log stream.
3. Cause 2: register a FARGATE job definition with an awslogs logConfiguration, submit a job that echoes to stdout, then poll aws batch describe-jobs --jobs <jobId> and aws logs describe-log-streams --log-group-name <group> -> the job never settles and no stream is ever created.
4. Re-replay the check ids in trace_ids.
