Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.op-contract-listJobs batch.op-contract-terminateJob batch.submit-job-progresses-to-runnable batch.job-submit-to-unknown-queue-fails batch.cancel-job-before-running-moves-to-failed batch.cancel-jobs-bulk-cancels-pending-job-and-leaves-terminal-job-frozen batch.job-dependency-holds-job-pending batch.job-queue-snapshot-reflects-pending-job batch.list-jobs-default-status-filter-is-running batch.terminate-runnable-job-forces-failed batch.terminate-terminal-job-is-idempotent batch.terminate-jobs-bulk-partitions-requested-ids-and-terminates-successful batch.register-job-definition-retry-attempts-above-maximum-fails batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected batch.register-fargate-job-definition-illegal-vcpu-memory-pair-fails batch.job-definition-environment-variable-empty-string-vs-absent-preserved batch.deploy-compute-environment-cloudformation-behaves
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.op-contract-listJobs|run#9|AssertionError|assert job["status"] == "RUNNABLE", f"job {job_id} must rest in RUNNABLE, got {job['status']!r}"
Issue-Key: batch.op-contract-terminateJob|run#7|AssertionError|assert job["status"] == "RUNNABLE", f"expected the job to rest in RUNNABLE, got {job['status']!r}"
Issue-Key: batch.submit-job-progresses-to-runnable|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-submit-to-unknown-queue-fails|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: batch.cancel-job-before-running-moves-to-failed|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: batch.cancel-jobs-bulk-cancels-pending-job-and-leaves-terminal-job-frozen|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-dependency-holds-job-pending|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-queue-snapshot-reflects-pending-job|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: batch.list-jobs-default-status-filter-is-running|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: batch.terminate-runnable-job-forces-failed|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: batch.terminate-terminal-job-is-idempotent|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: batch.terminate-jobs-bulk-partitions-requested-ids-and-terminates-successful|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: batch.register-job-definition-retry-attempts-above-maximum-fails|_client_error#9|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected|_client_error#15|AssertionError|raise AssertionError(
Issue-Key: batch.register-fargate-job-definition-illegal-vcpu-memory-pair-fails|_client_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: batch.job-definition-environment-variable-empty-string-vs-absent-preserved|run#12|AssertionError|assert "EMPTY_VAR" not in env_by_name, (
Issue-Key: batch.deploy-compute-environment-cloudformation-behaves|run#15|IndexError|jq = batch.describe_job_queues(jobQueues=[jq_name])["jobQueues"][0]
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# batch: submitted jobs are driven straight to FAILED instead of resting in RUNNABLE, RegisterJobDefinition accepts input AWS rejects, and a CFN-created job queue is invisible (6 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Six distinct root causes in the AWS Batch emulator's job plane and job-definition representation, all grounded in failing behavioral checks that the replay report does NOT mark ALREADY FILED/OWNED. Numbered; a fixer must close all six.

=== Cause 1 of 6: a submitted job never reaches/rests in RUNNABLE — it goes to FAILED ===
AWS: Smithy models SubmitJob POST /v1/submitjob -> 200 {jobId, jobArn, jobName}; JobStatus enum is SUBMITTED|PENDING|RUNNABLE|STARTING|RUNNING|SUCCEEDED|FAILED. Documented transition for a job with no unsatisfied dependsOn is SUBMITTED -> RUNNABLE (PENDING is entered only while a dependency is outstanding), and on an UNMANAGED compute environment there is nowhere to place the job, so it PARKS in RUNNABLE: it does not advance to STARTING and does not become terminal. Recorded behavior additionally shows an unplaced RUNNABLE job carries createdAt but no startedAt/stoppedAt and no attempts, and that CancelJob/CancelJobs on a job held PENDING by an unsatisfied dependency sets isCancelled=true while leaving status at PENDING.
Emulator: DescribeJobs reports the freshly submitted job as 'FAILED'. Two checks that reach the assertion prove it directly ('job <id> must rest in RUNNABLE, got FAILED'); because FAILED is terminal, every other job-plane check that polls for RUNNABLE/PENDING/RUNNING never observes it and the check is killed by the harness at 60s. The emulator's queue/CE provisioning itself works (CE and queue reach VALID), so the defect is isolated to the job state machine driven by SubmitJob/DescribeJobs.
Why: this is the core of Batch. Any client that submits a job and waits for it to become RUNNABLE/RUNNING, or that cancels/terminates a queued job, sees an immediate spurious terminal failure; dependency gating, queue snapshots and status-filtered ListJobs are all unobservable.
Fix: the job state machine in the Batch service state plus the SubmitJob/DescribeJobs/ListJobs handlers (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/submit_job.rs, describe_jobs.rs and the shared state module): a submitted job must enter SUBMITTED, move to RUNNABLE when its dependencies are satisfied (PENDING while they are not), and stay there when the backing CE is UNMANAGED, with no startedAt/stoppedAt/attempts until it is actually placed.
Traces: batch.op-contract-listJobs and batch.op-contract-terminateJob (both: got 'FAILED'); and, timing out at 60s while polling for the state that never arrives: batch.submit-job-progresses-to-runnable, batch.job-submit-to-unknown-queue-fails, batch.cancel-job-before-running-moves-to-failed, batch.cancel-jobs-bulk-cancels-pending-job-and-leaves-terminal-job-frozen, batch.job-dependency-holds-job-pending, batch.job-queue-snapshot-reflects-pending-job, batch.list-jobs-default-status-filter-is-running, batch.terminate-runnable-job-forces-failed, batch.terminate-terminal-job-is-idempotent, batch.terminate-jobs-bulk-partitions-requested-ids-and-terminates-successful.

=== Cause 2 of 6: RegisterJobDefinition accepts retryStrategy.attempts above the documented maximum ===
AWS: Smithy RetryStrategy.attempts documents 'The number of times to move a job to the RUNNABLE status. You can specify between 1 and 10 attempts.' Recorded behavior: a value above 10 is REFUSED (ClientException, HTTP 400); nothing is registered.
Emulator: returns HTTP 200 and a new revision (e.g. {'jobDefinitionName': 'sim2real-behav-jd-retrycap-...', 'jobDefinitionArn': 'arn:aws:batch:us-west-2:...:job-definition/...:2', 'revision': 2}).
Why: the emulator is overly permissive — a bad definition that real AWS rejects at registration is accepted, so the client's error path is never exercised and the fleet diverges at submit time instead.
Fix: input validation in the RegisterJobDefinition handler (.../src/ops/register_job_definition.rs) — range-check retryStrategy.attempts and refuse with ClientException 400.
Traces: batch.register-job-definition-retry-attempts-above-maximum-fails.

=== Cause 3 of 6: RegisterJobDefinition accepts a multi-node definition whose mainNode is >= numNodes ===
AWS: Smithy NodeProperties.mainNode documents 'Specifies the node index for the main node of a multi-node parallel job. This node index value must be fewer than the number of nodes.' Recorded behavior: mainNode >= numNodes is REFUSED (ClientException, HTTP 400).
Emulator: the RegisterJobDefinition call SUCCEEDS ('real AWS refuses this RegisterJobDefinition, but it SUCCEEDED and returned ...').
Why: an unsubmittable multi-node job definition is persisted; the failure surfaces far from its cause.
Fix: cross-member validation of NodeProperties in the RegisterJobDefinition handler.
Traces: batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected.

=== Cause 4 of 6: RegisterJobDefinition accepts an illegal Fargate vCPU/memory pair ===
AWS: for platformCapabilities=[FARGATE], resourceRequirements VCPU/MEMORY must be one of the allowed Fargate combinations (ResourceRequirement's modeled documentation enumerates the legal vCPU/memory pairs); an illegal pair is REFUSED with ClientException, HTTP 400, and no revision is created.
Emulator: HTTP 200 with a new revision (observed: revision 2 for 'sim2real-behav-jd-badsize-...').
Why: a Fargate job definition that can never be placed is accepted, hiding a configuration error AWS catches at registration.
Fix: Fargate vCPU/memory pair validation in the RegisterJobDefinition handler.
Traces: batch.register-fargate-job-definition-illegal-vcpu-memory-pair-fails.

=== Cause 5 of 6: an explicit empty-string environment variable is preserved instead of dropped ===
AWS: recorded behavior only — a KeyValuePair in containerProperties.environment whose value is the empty string is silently DROPPED by RegisterJobDefinition, so DescribeJobDefinitions does not report that name at all (indistinguishable from absent). KeyValuePair models no constraint on value, so this is a recorded representation rule, not a modeled one.
Emulator: DescribeJobDefinitions reports the variable with value '' ('expected real AWS to silently drop the explicit empty-string env var, but it was present').
Why: a client that uses the presence of a name as a signal (or diffs a round-tripped definition against what it sent) sees a variable AWS would have removed, so round-trip comparisons and idempotent re-registration diverge.
Fix: environment normalization on the write path of the RegisterJobDefinition handler (drop empty-valued pairs before the revision is stored).
Traces: batch.job-definition-environment-variable-empty-string-vs-absent-preserved.

=== Cause 6 of 6: a job queue created through CloudFormation is not visible to DescribeJobQueues ===
AWS: a CloudFormation stack containing AWS::Batch::ComputeEnvironment + AWS::Batch::JobQueue produces real Batch resources, so DescribeJobQueues(jobQueues=[<the queue created by the stack>]) returns exactly one jobQueue once the stack is CREATE_COMPLETE.
Emulator: DescribeJobQueues returns an empty jobQueues list for the stack's queue — the check dies with IndexError: list index out of range on ['jobQueues'][0]. Either the CFN provisioning does not create the Batch resource or it creates it under a name/identity DescribeJobQueues cannot resolve.
Why: Batch resources deployed via infrastructure-as-code are invisible to the Batch API, so no CFN-driven workflow can be emulated end to end.
Fix: the CloudFormation resource provider for AWS::Batch::JobQueue / AWS::Batch::ComputeEnvironment and the identity it registers in Batch service state (must be resolvable by the same short name/ARN DescribeJobQueues accepts).
Traces: batch.deploy-compute-environment-cloudformation-behaves.

## Evidence
From the simparity-behavioral-aws-v1 replay (batch 5/232 passed):

Cause 1 (direct): [mismatch] batch.op-contract-listJobs — AssertionError: job f025cd1f-a93f-489e-9b41-1213efebd7c6 must rest in RUNNABLE, got 'FAILED'. [mismatch] batch.op-contract-terminateJob — AssertionError: expected the job to rest in RUNNABLE, got 'FAILED'.
Cause 1 (consequential, all 'Error: Check timed out after 60.0s' while polling DescribeJobs for RUNNABLE/PENDING): batch.submit-job-progresses-to-runnable (waits _wait_job_status(..., {'RUNNABLE'}) after SubmitJob on an UNMANAGED CE whose CE and queue both reached VALID), batch.job-submit-to-unknown-queue-fails (times out on its positive control: SubmitJob to a real ENABLED queue must reach RUNNABLE), batch.cancel-job-before-running-moves-to-failed, batch.cancel-jobs-bulk-cancels-pending-job-and-leaves-terminal-job-frozen, batch.job-dependency-holds-job-pending, batch.job-queue-snapshot-reflects-pending-job, batch.list-jobs-default-status-filter-is-running, batch.terminate-runnable-job-forces-failed, batch.terminate-terminal-job-is-idempotent, batch.terminate-jobs-bulk-partitions-requested-ids-and-terminates-successful.
Cause 2: [mismatch] batch.register-job-definition-retry-attempts-above-maximum-fails — AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobDefinitionName': 'sim2real-behav-jd-retrycap-df15da02fa3a', 'jobDefinitionArn': 'arn:aws:batch:us-west-2:000000000000:job-definition/sim2real-behav-jd-retrycap-df15da02fa3a:2', 'revision': 2}. Model: RetryStrategy.attempts doc 'You can specify between 1 and 10 attempts'.
Cause 3: [mismatch] batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected — AssertionError: real AWS refuses this RegisterJobDefinition, but it SUCCEEDED and returned {...}. Model: NodeProperties.mainNode doc 'This node index value must be fewer than the number of nodes'.
Cause 4: [mismatch] batch.register-fargate-job-definition-illegal-vcpu-memory-pair-fails — AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobDefinitionName': 'sim2real-behav-jd-badsize-3f408940b83f', ..., 'revision': 2}.
Cause 5: [mismatch] batch.job-definition-environment-variable-empty-string-vs-absent-preserved — AssertionError: expected real AWS to silently drop the explicit empty-string env var, but it was present: ''.
Cause 6: [error] batch.deploy-compute-environment-cloudformation-behaves — IndexError: list index out of range at jq = batch.describe_job_queues(jobQueues=[jq_name])['jobQueues'][0].

Model facts used: smithy_query(operation="RegisterJobDefinition", shape="RetryStrategy,NodeProperties,KeyValuePair") — RegisterJobDefinition POST /v1/registerjobdefinition, 200 {jobDefinitionName, jobDefinitionArn, revision}, errors ClientException(400)/ServerException(500); attempts documented 1..10; mainNode documented < numNodes; KeyValuePair.value unconstrained.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite.
2. Cause 1: create an UNMANAGED compute environment, wait for status VALID; create an ENABLED job queue on it, wait for VALID; register a container job definition (image busybox, VCPU 1 / MEMORY 512); aws batch submit-job --job-name j --job-queue <jq> --job-definition <jdArn>; then poll aws batch describe-jobs --jobs <jobId>. Observe status 'FAILED' immediately instead of SUBMITTED -> RUNNABLE parked (and no startedAt/stoppedAt).
3. Cause 2: aws batch register-job-definition --job-definition-name d --type container --container-properties '{...}' --retry-strategy '{"attempts": 11}' -> HTTP 200 + revision instead of ClientException 400.
4. Cause 3: aws batch register-job-definition --type multinode --node-properties '{"numNodes":2,"mainNode":2,"nodeRangeProperties":[...]}' -> accepted instead of ClientException 400.
5. Cause 4: aws batch register-job-definition --type container --platform-capabilities FARGATE with resourceRequirements VCPU/MEMORY set to a pair Fargate does not allow -> HTTP 200 instead of ClientException 400.
6. Cause 5: aws batch register-job-definition --type container --container-properties with environment [{"name":"EMPTY_VAR","value":""}], then aws batch describe-job-definitions --job-definition-name <name> -> EMPTY_VAR is present with value '' instead of being absent.
7. Cause 6: deploy a CloudFormation stack with AWS::Batch::ComputeEnvironment + AWS::Batch::JobQueue, wait for CREATE_COMPLETE, then aws batch describe-job-queues --job-queues <stack queue name> -> empty jobQueues list.
