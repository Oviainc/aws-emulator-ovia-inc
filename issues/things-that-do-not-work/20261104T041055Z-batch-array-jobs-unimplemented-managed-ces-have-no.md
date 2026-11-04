Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.array-job-children-receive-distinct-array-index batch.batch-array-n-to-n-dependency-gates-only-matching-child-index batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs batch.managed-fargate-compute-environment-reaches-valid batch.server-side-job-placement-honors-compute-environment-order batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries batch.batch-job-runs-to-succeeded batch.job-runs-to-succeeded-with-exit-code-and-status-sequence batch.failing-job-reports-real-nonzero-exit-code batch.cancel-running-job-requires-terminate batch.compute-environment-disabled-withholds-placement-until-reenabled batch.managed-compute-environment-auto-scales-server-side batch.job-attempt-duration-timeout-forces-failed batch.job-attempt-duration-timeout-terminates-job batch.job-attempt-duration-timeout-terminates-running-job batch.job-timeout-attribute-triggers-timeout-failure batch.job-retry-strategy-evaluate-on-exit-controls-attempt-count batch.job-dependency-releases-on-success-and-blocks-on-parent-failure batch.job-parameter-substitution-and-container-override-precedence batch.job-queue-priority-scheduling-order-enforced batch.job-queue-snapshot-capacity-usage-reflects-real-running-job batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs batch.concurrent-consumable-resource-allocation-enforces-capacity batch.delete-consumable-resource-in-use-rejected batch.fair-share-quota-weight-orders-job-dispatch batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job batch.ce-managed-fargate-missing-subnets-fails batch.job-attempt-duration-below-minimum-rejected batch.batch-submit-with-unregistered-consumable-resource-rejected batch.op-contract-update-compute-environment batch.submit-job-to-disabled-queue-rejected batch.get-job-queue-snapshot-unknown-queue-rejected batch.delete-scheduling-policy-lifecycle
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|SubmitJob|NotImplemented
Issue-Key: batch.managed-fargate-compute-environment-reaches-valid|run#14|AssertionError|assert ce.get("ecsClusterArn"), "expected a backing ecsClusterArn for a MANAGED CE"
Issue-Key: batch.server-side-job-placement-honors-compute-environment-order|run#6|AssertionError|assert ce_a_cluster and ce_b_cluster and ce_a_cluster != ce_b_cluster, (
Issue-Key: batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries|_poll_until#7|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: batch.batch-job-runs-to-succeeded|_poll_until#12|Timeout|return last
Issue-Key: batch.job-runs-to-succeeded-with-exit-code-and-status-sequence|_trace_job_to_terminal#8|Timeout|return trace, job
Issue-Key: batch.failing-job-reports-real-nonzero-exit-code|_poll_until#12|Timeout|return last
Issue-Key: batch.cancel-running-job-requires-terminate|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: batch.compute-environment-disabled-withholds-placement-until-reenabled|_poll_until#11|Timeout|return last
Issue-Key: batch.managed-compute-environment-auto-scales-server-side|_poll_until#18|Timeout|last = probe()
Issue-Key: batch.job-attempt-duration-timeout-forces-failed|_trace_job_to_terminal#12|Timeout|return trace, job
Issue-Key: batch.job-attempt-duration-timeout-terminates-job|_poll_until#14|Timeout|return last
Issue-Key: batch.job-attempt-duration-timeout-terminates-running-job|_wait_job_status#18|Timeout|return last
Issue-Key: batch.job-timeout-attribute-triggers-timeout-failure|_poll_until#16|Timeout|return last
Issue-Key: batch.job-retry-strategy-evaluate-on-exit-controls-attempt-count|_poll_until#13|Timeout|return last
Issue-Key: batch.job-dependency-releases-on-success-and-blocks-on-parent-failure|_trace_job_to_terminal#14|Timeout|return trace, job
Issue-Key: batch.job-parameter-substitution-and-container-override-precedence|_poll_until#11|Timeout|return last
Issue-Key: batch.job-queue-priority-scheduling-order-enforced|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-queue-snapshot-capacity-usage-reflects-real-running-job|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs|run#21|Timeout|assert any(n == 1 for n in concurrent_running), (
Issue-Key: batch.concurrent-consumable-resource-allocation-enforces-capacity|_poll_until#12|Timeout|raise AssertionError(f"condition not met within {timeout_s}s; last value={last!r}")
Issue-Key: batch.delete-consumable-resource-in-use-rejected|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: batch.fair-share-quota-weight-orders-job-dispatch|_poll_job_status#13|Timeout|raise AssertionError(
Issue-Key: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: batch.ce-managed-fargate-missing-subnets-fails|_err_code#12|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: batch.job-attempt-duration-below-minimum-rejected|_error#16|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: batch.batch-submit-with-unregistered-consumable-resource-rejected|_error_code#19|AssertionError|raise AssertionError(
Issue-Key: batch.op-contract-update-compute-environment|_client_error#25|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: batch.submit-job-to-disabled-queue-rejected|run#17|AssertionError|assert message == f"JobQueue {queue_arn} not ENABLED.", f"expected the exact DISABLED-queue refusal message, got {message!r}"
Issue-Key: batch.get-job-queue-snapshot-unknown-queue-rejected|run#27|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.delete-scheduling-policy-lifecycle|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# batch: array jobs unimplemented, MANAGED CEs have no backing ECS cluster, submitted jobs never settle, and 7 refusal contracts diverge (10 causes)

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Ten net-new behavioral root causes on the batch control and job planes, all grounded in failing checks of the simparity-behavioral-aws-v1 replay against this checkout. Each must be closed before submitting.

Cause 1 of 10 — SubmitJob with arrayProperties answers NotImplemented instead of creating an array job.
AWS: Smithy com.amazonaws.batch#SubmitJob, POST /v1/submitjob, success code 200; input member `arrayProperties` targets ArrayProperties{size:Integer} ("An object that represents an Batch array job"); the only modeled errors are ClientException (httpError 400) and ServerException (500) — no NotImplemented is modeled. AWS accepts the submission with 200 {jobArn,jobName,jobId}, creates `size` children, and DescribeJobs reports ArrayPropertiesDetail.size/statusSummary on the parent and ArrayPropertiesDetail.index on each child.
Emulator: the call is refused — ClientError (NotImplemented) "operation SubmitJob with arrayProperties is scaffolded but not implemented in this emulator".
Why: no array workload can be submitted at all. Per-child AWS_BATCH_JOB_ARRAY_INDEX, N_TO_N dependency gating, the parent's statusSummary and front-of-queue de-duplication of array children are all unreachable, and a client sees an error class AWS never returns for a modeled, optional member.
Fix: the arrayProperties branch of the SubmitJob handler in the batch crate (child-job creation and array state), plus the ArrayPropertiesDetail projection in DescribeJobs.
Traces: batch.array-job-children-receive-distinct-array-index, batch.batch-array-n-to-n-dependency-gates-only-matching-child-index, batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs.

Cause 2 of 10 — a MANAGED compute environment that reaches VALID reports no ecsClusterArn.
AWS: Smithy ComputeEnvironmentDetail.ecsClusterArn — "The Amazon Resource Name (ARN) of the underlying Amazon ECS cluster that the compute environment uses." A MANAGED compute environment (containerOrchestrationType ECS, the default) is backed by an ECS cluster Batch provisions, and once the CE is VALID DescribeComputeEnvironments reports that cluster's ARN; two distinct CEs are backed by two distinct clusters.
Emulator: the member is absent from the DescribeComputeEnvironments entry of a MANAGED/FARGATE CE that reached status VALID ("expected a backing ecsClusterArn for a MANAGED CE"), and for two CEs created in one run both read None ("expected two distinct ECS clusters backing CE-A and CE-B, got None and None").
Why: the ECS cluster ARN is how a caller joins a Batch compute environment to the tasks that run its jobs — it is the only documented handle for ListTasks/DescribeTasks, task-level tag propagation and awslogs stream inspection. Absent, every Batch→ECS correlation a client performs fails with a missing key, and placement across a compute-environment order cannot be observed at all.
Fix: managed-CE provisioning in CreateComputeEnvironment (record a backing ECS cluster in the batch crate's compute-environment state) and the DescribeComputeEnvironments serializer that must then emit ecsClusterArn.
Traces: batch.managed-fargate-compute-environment-reaches-valid, batch.server-side-job-placement-honors-compute-environment-order.

Cause 3 of 10 — a submitted container job never reaches a terminal status: it is never advanced, never run and never recorded.
AWS: after SubmitJob returns 200 Batch advances the job SUBMITTED → PENDING → RUNNABLE → STARTING → RUNNING → SUCCEEDED|FAILED server-side with no further client calls, and DescribeJobs reports startedAt/stoppedAt and an `attempts` entry carrying the container's exitCode/reason. SUCCEEDED or FAILED always arrives for a job placed on an ENABLED queue whose CE is VALID (or the job is FAILED with a statusReason).
Emulator: the terminal status never arrives. One check states it literally — TimeoutError "job <job-id> reached SUCCEEDED or FAILED: the run budget ended first; not graded" — and twenty further data-plane checks that wait on a terminal job status, a running job, an exit code, a retry attempt or an auto-scaled CE exhaust the harness budget with the status never arriving.
Why: every job-execution contract of the service is unobservable: exit codes, timeouts (attemptDurationSeconds / job timeout), retryStrategy and evaluateOnExit, dependsOn release-and-block, queue priority order, consumable-resource capacity capping, maxvCpus holding, cancel-vs-terminate and the queue snapshot's capacity usage. A client that submits work and waits for it — the normal use of Batch — waits forever.
Hypothesis: this is the downstream effect of Cause 2 — with no backing ECS cluster there is nothing for the scheduler to place a task on, so the job state machine never leaves its initial status.
Fix: the job scheduler/placement loop in the batch crate — the state machine that advances a job's status and records its attempts — not the SubmitJob handler, which already answers 200.
Traces: batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries (the explicit TimeoutError), batch.batch-job-runs-to-succeeded, batch.job-runs-to-succeeded-with-exit-code-and-status-sequence, batch.failing-job-reports-real-nonzero-exit-code, batch.cancel-running-job-requires-terminate, batch.compute-environment-disabled-withholds-placement-until-reenabled, batch.managed-compute-environment-auto-scales-server-side, batch.job-attempt-duration-timeout-forces-failed, batch.job-attempt-duration-timeout-terminates-job, batch.job-attempt-duration-timeout-terminates-running-job, batch.job-timeout-attribute-triggers-timeout-failure, batch.job-retry-strategy-evaluate-on-exit-controls-attempt-count, batch.job-dependency-releases-on-success-and-blocks-on-parent-failure, batch.job-parameter-substitution-and-container-override-precedence, batch.job-queue-priority-scheduling-order-enforced, batch.job-queue-snapshot-capacity-usage-reflects-real-running-job, batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs, batch.concurrent-consumable-resource-allocation-enforces-capacity, batch.delete-consumable-resource-in-use-rejected, batch.fair-share-quota-weight-orders-job-dispatch, batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job.

Cause 4 of 10 — CreateComputeEnvironment accepts a MANAGED FARGATE compute environment with no subnets.
AWS: Smithy ComputeResource.subnets — "This parameter is required for compute environments using EC2, SPOT, FARGATE, or FARGATE_SPOT compute resources" — and CreateComputeEnvironment models ClientException (400). The refusal is HTTP 400 / ClientException with message "Error executing request, Exception : Resource subnets are required., RequestId: <request id>": the fixed text is "Error executing request, Exception : Resource subnets are required., RequestId: " and the only variable part is the refused call's own request id. No compute environment is left behind.
Emulator: HTTP 200 with computeEnvironmentName/computeEnvironmentArn for computeResources {type: FARGATE, maxvCpus: 16, subnets: [], securityGroupIds: []}, and the CE is created. The identical shape WITH subnets is accepted in the same run, so only the missing-subnets refusal is absent.
Why: the refusal is the only thing that tells a caller its networking is unusable at create time. Accepted, the caller gets an ARN for a compute environment that can never place a task, and the real failure surfaces much later as jobs that never run.
Fix: CreateComputeEnvironment's computeResources validation in the batch crate (require a non-empty subnets list for EC2/SPOT/FARGATE/FARGATE_SPOT before any state is written).
Traces: batch.ce-managed-fargate-missing-subnets-fails.

Cause 5 of 10 — SubmitJob accepts timeout.attemptDurationSeconds below the 60-second minimum.
AWS: Smithy JobTimeout.attemptDurationSeconds — "The minimum value for the timeout is 60 seconds." SubmitJob models ClientException (400); the refusal message is "Error executing request, Exception : AttemptDurationSeconds in Timeout must be at least 60 seconds., RequestId: <request id>" (fixed text, with the refused call's own request id as the only variable part), and no job is created in any status.
Emulator: HTTP 200 with {jobArn, jobName, jobId} for timeout={"attemptDurationSeconds": 59}. The 60-second positive control is accepted in the same run, so the boundary is simply unenforced.
Why: a client relying on AWS to reject an out-of-range timeout gets a job whose timeout semantics AWS would never have created, and the invalid value is silently persisted on the job.
Fix: SubmitJob's input validation in the batch crate (range check on JobTimeout.attemptDurationSeconds before the job is created).
Traces: batch.job-attempt-duration-below-minimum-rejected.

Cause 6 of 10 — SubmitJob accepts a consumableResourcePropertiesOverride naming a consumable resource that was never registered.
AWS: SubmitJob models ClientException (400); a consumableResourceList entry whose `consumableResource` does not resolve is refused with message "Consumable resource arn:aws:batch:<region>:<account-id>:consumable-resource/<name> does not exist." — fixed text "Consumable resource " … " does not exist.", with the ARN built from the caller's region and account id and the resource name taken from the request. No job is created.
Emulator: HTTP 200 with {jobArn, jobName, jobId}; the job exists on the queue. A submission naming a REGISTERED consumable resource is accepted in the same run, so the existence check is simply missing.
Why: the job is admitted holding a quantity of a resource that does not exist, so consumable-resource accounting is silently wrong and the caller never learns its resource name was a typo.
Fix: resolution of consumableResourcePropertiesOverride.consumableResourceList against consumable-resource state in the SubmitJob handler, before the job is created.
Traces: batch.batch-submit-with-unregistered-consumable-resource-rejected.

Cause 7 of 10 — UpdateComputeEnvironment accepts an EC2-only computeResources member on a FARGATE compute environment.
AWS: Smithy ComputeResource.instanceRole — "This parameter isn't applicable to jobs that are running on Fargate resources. Don't specify it." UpdateComputeEnvironment models ClientException (400) and refuses with the fixed sentence "The following parameters can't be updated for Fargate compute environment: allocationStrategy, bidPercentage, ec2Configuration, ec2KeyPair, imageId, instanceRole, instanceTypes, launchTemplate, placementGroup, tags, type, updatePolicy, updateToLatestImageVersion." (no interpolation; the list is the EC2/SPOT-only member set).
Emulator: HTTP 200 with {computeEnvironmentName, computeEnvironmentArn} for update_compute_environment(computeEnvironment=<FARGATE CE>, computeResources={"instanceRole": "arn:aws:iam::<account>:instance-profile/bogus"}). A legitimate maxvCpus update on the same CE is accepted in the same run, so only the Fargate field gate is absent.
Why: the caller believes an EC2-only setting took effect on a Fargate environment. Nothing applies it, so the configuration the client thinks it has and the one the service has diverge silently, and the named-parameter list AWS sends — the caller's only guide to which field was wrong — is never produced.
Fix: UpdateComputeEnvironment's computeResources validation in the batch crate (reject the EC2/SPOT-only members when the CE's computeResources.type is FARGATE/FARGATE_SPOT, with AWS's sentence).
Traces: batch.op-contract-update-compute-environment.

Cause 8 of 10 — SubmitJob's DISABLED-queue refusal message is emulator-authored.
AWS: SubmitJob models ClientException (400); submitting to a queue whose state is DISABLED is refused with message "JobQueue <jobQueueArn> not ENABLED." — fixed text "JobQueue " … " not ENABLED.", the one variable part being the queue's full ARN (arn:aws:batch:<region>:<account-id>:job-queue/<name>) resolved from the jobQueue the request names.
Emulator: the status and error code are right (400 / ClientException) but the message reads "Job queue arn:aws:batch:<region>:<account-id>:job-queue/<name> is DISABLED and can't accept new jobs" — a different sentence, with a lower-cased noun and emulator-authored prose.
Why: Batch answers every client mistake with one error shape, so the message text is what callers and operators match on to tell "queue disabled" from "queue missing" or "definition inactive". A sentence AWS never sends breaks those matches.
Fix: the state-gate refusal text in SubmitJob's job-queue resolution in the batch crate.
Traces: batch.submit-job-to-disabled-queue-rejected.

Cause 9 of 10 — GetJobQueueSnapshot's unknown-queue refusal names the bare queue name instead of the ARN AWS reports.
AWS: Smithy GetJobQueueSnapshot, POST /v1/getjobqueuesnapshot, models ClientException (400). An unknown jobQueue is refused with message "<jobQueueArn> does not exist." — fixed text " does not exist." preceded by the full ARN arn:aws:batch:<region>:<account-id>:job-queue/<name> built from the caller's region/account and the name the request named.
Emulator: 400 / ClientException with message "JobQueue <name> not found." — a different sentence that drops the ARN and adds a noun prefix.
Why: a client that detects a missing queue by AWS's wording treats this as an unclassified failure, and the message loses the fully-qualified identifier AWS echoes for logs and error surfaces. Related open issue 20261029T234420Z-batch-refusal-contracts-diverge-not-found-arn-wording.md covers other operations' not-found ARN wording and does not carry this check.
Fix: the not-found refusal text in GetJobQueueSnapshot's queue resolution in the batch crate.
Traces: batch.get-job-queue-snapshot-unknown-queue-rejected.

Cause 10 of 10 — DeleteSchedulingPolicy's still-referenced refusal is emulator-authored prose.
AWS: Smithy DeleteSchedulingPolicy, POST /v1/deleteschedulingpolicy, models ClientException (400). Deleting a scheduling policy still referenced by a job queue is refused with the fixed message "Cannot delete, found existing JobQueue relationship" (no interpolation), and the policy stays visible to DescribeSchedulingPolicies.
Emulator: 400 / ClientException with message "Cannot delete a scheduling policy that's used in any job queues." — a different sentence.
Why: the relationship-delete refusal text is the caller's signal to drop the referencing queue first; an emulator-authored sentence defeats matching on it. Open issue 20261029T173319Z-batch-8-net-new-root-causes-relationship-delete.md carries a sibling check for the same message family but not this check's replay key, so this cause is filed with its own.
Fix: the referenced-policy refusal text in DeleteSchedulingPolicy in the batch crate.
Traces: batch.delete-scheduling-policy-lifecycle.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout: 160/265 passed; batch 153/232 with 79 failures. Expected = real AWS behavior asserted by the check; actual = emulator.

Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SubmitJob operation: operation SubmitJob with arrayProperties is scaffolded but not implemented in this emulator (3 checks).

Cause 2: AssertionError: expected a backing ecsClusterArn for a MANAGED CE; AssertionError: expected two distinct ECS clusters backing CE-A and CE-B, got None and None.

Cause 3: TimeoutError: job 29ceea34-32e4-4b5b-8fd8-ee287a2193a8 reached SUCCEEDED or FAILED: the run budget ended first; not graded — plus 20 further checks that wait on a terminal/RUNNING job status and never observe one within the harness budget.

Cause 4: AssertionError: expected a rejection, but the call SUCCEEDED and returned {... 'HTTPStatusCode': 200 ..., 'computeEnvironmentName': 'sim2real-behav-ce-fg-bad-<id>', 'computeEnvironmentArn': 'arn:aws:batch:<region>:<account>:compute-environment/sim2real-behav-ce-fg-bad-<id>'}; expected 400 ClientException "Error executing request, Exception : Resource subnets are required., RequestId: <request id>".

Cause 5: AssertionError: expected a rejection, but the call succeeded and returned {... 200 ..., 'jobId': '<uuid>'} for timeout={'attemptDurationSeconds': 59}; expected 400 ClientException "Error executing request, Exception : AttemptDurationSeconds in Timeout must be at least 60 seconds., RequestId: <request id>".

Cause 6: AssertionError: the call was expected to be REJECTED but SUCCEEDED, returning {... 200 ..., 'jobId': '<uuid>'}; expected 400 ClientException "Consumable resource arn:aws:batch:<region>:<account-id>:consumable-resource/<name> does not exist."

Cause 7: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 200 ..., 'computeEnvironmentArn': ...}; expected 400 ClientException "The following parameters can't be updated for Fargate compute environment: allocationStrategy, bidPercentage, ec2Configuration, ec2KeyPair, imageId, instanceRole, instanceTypes, launchTemplate, placementGroup, tags, type, updatePolicy, updateToLatestImageVersion."

Cause 8: AssertionError: expected the exact DISABLED-queue refusal message, got "Job queue arn:aws:batch:<region>:<account>:job-queue/<name> is DISABLED and can't accept new jobs"; expected "JobQueue <jobQueueArn> not ENABLED."

Cause 9: AssertionError: expected message '<jobQueueArn> does not exist.', got 'JobQueue <name> not found.'

Cause 10: AssertionError: expected message 'Cannot delete, found existing JobQueue relationship', got "Cannot delete a scheduling policy that's used in any job queues."

## Steps to Reproduce
Boot the emulator from this checkout and replay the behavioral suite, or by hand against it:

Cause 1: aws batch submit-job --job-name j --job-queue <enabled queue> --job-definition <jd> --array-properties size=3 -> NotImplemented; AWS answers 200 and creates 3 children.
Cause 2: create a MANAGED CE with computeResources.type FARGATE, wait for status VALID, then aws batch describe-compute-environments --compute-environments <name> -> no ecsClusterArn member.
Cause 3: submit any container job to an ENABLED queue backed by a VALID MANAGED FARGATE CE and poll describe-jobs -> the status never becomes SUCCEEDED or FAILED.
Cause 4: aws batch create-compute-environment --compute-environment-name ce --type MANAGED --state ENABLED --compute-resources '{"type":"FARGATE","maxvCpus":16,"subnets":[],"securityGroupIds":[]}' --service-role <AWSServiceRoleForBatch> -> 200; AWS answers 400 ClientException "Error executing request, Exception : Resource subnets are required., RequestId: <id>".
Cause 5: aws batch submit-job ... --timeout attemptDurationSeconds=59 -> 200 with a jobId; AWS refuses with the 60-second-minimum message.
Cause 6: aws batch submit-job ... --consumable-resource-properties-override '{"consumableResourceList":[{"consumableResource":"never-registered","quantity":1}]}' -> 200; AWS refuses "Consumable resource <arn> does not exist."
Cause 7: aws batch update-compute-environment --compute-environment <FARGATE CE> --compute-resources '{"instanceRole":"arn:aws:iam::<account>:instance-profile/bogus"}' -> 200; AWS refuses with the Fargate parameter list.
Cause 8: update a job queue to DISABLED, then submit-job to it -> message "Job queue <arn> is DISABLED and can't accept new jobs"; AWS sends "JobQueue <arn> not ENABLED."
Cause 9: aws batch get-job-queue-snapshot --job-queue <never-created name> -> "JobQueue <name> not found."; AWS sends "<arn> does not exist."
Cause 10: create a scheduling policy, attach it to a job queue, then aws batch delete-scheduling-policy --arn <policy arn> -> "Cannot delete a scheduling policy that's used in any job queues."; AWS sends "Cannot delete, found existing JobQueue relationship".

Then re-replay the check ids in trace_ids.
