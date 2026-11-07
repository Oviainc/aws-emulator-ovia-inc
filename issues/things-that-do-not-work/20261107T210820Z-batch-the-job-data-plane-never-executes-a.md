Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.batch-job-runs-to-succeeded batch.job-runs-to-succeeded-with-exit-code-and-status-sequence batch.failing-job-reports-real-nonzero-exit-code batch.job-parameter-substitution-and-container-override-precedence batch.compute-environment-disabled-withholds-placement-until-reenabled batch.concurrent-consumable-resource-allocation-enforces-capacity batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs batch.fair-share-quota-weight-orders-job-dispatch batch.job-queue-snapshot-reports-accurate-running-and-runnable-counts batch.quota-share-enforced-server-side-on-job-scheduling batch.get-job-queue-snapshot-unknown-queue-rejected batch.array-job-children-receive-distinct-array-index batch.job-dependency-releases-on-success-and-blocks-on-parent-failure batch.batch-array-n-to-n-dependency-gates-only-matching-child-index batch.job-attempt-duration-timeout-forces-failed batch.job-attempt-duration-timeout-terminates-job batch.job-attempt-duration-timeout-terminates-running-job batch.job-timeout-attribute-triggers-timeout-failure batch.job-retry-strategy-evaluate-on-exit-controls-attempt-count batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries batch.e2e-service-job-lifecycle-through-queue-snapshot batch.job-queue-snapshot-includes-service-job-entries batch.jq-snapshot-and-list-jobs-status-filter batch.fair-share-scheduling-enforces-quota-allocation-server-side batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job batch.job-queue-snapshot-caps-front-of-queue-at-100 batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.batch-job-runs-to-succeeded|run#12|AssertionError|assert status == "SUCCEEDED", (
Issue-Key: batch.job-runs-to-succeeded-with-exit-code-and-status-sequence|run#8|AssertionError|assert job["status"] == "SUCCEEDED", (
Issue-Key: batch.failing-job-reports-real-nonzero-exit-code|run#12|AssertionError|assert fc.get("exitCode") == 17, (
Issue-Key: batch.job-parameter-substitution-and-container-override-precedence|run#11|AssertionError|assert default_job["status"] == "SUCCEEDED", (
Issue-Key: batch.compute-environment-disabled-withholds-placement-until-reenabled|_placed_control#11|AssertionError|raise AssertionError(
Issue-Key: batch.concurrent-consumable-resource-allocation-enforces-capacity|_warm_up_execution_role#12|AssertionError|raise AssertionError(f"warm-up job {job_id} FAILED: {reason!r}")
Issue-Key: batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs|run#21|AssertionError|assert any(n == 1 for n in concurrent_running), (
Issue-Key: batch.fair-share-quota-weight-orders-job-dispatch|_poll_job_status#13|AssertionError|raise AssertionError(
Issue-Key: batch.job-queue-snapshot-reports-accurate-running-and-runnable-counts|_poll_until#22|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last observed: {last!r})")
Issue-Key: batch.quota-share-enforced-server-side-on-job-scheduling|_poll_until#27|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last}")
Issue-Key: batch.get-job-queue-snapshot-unknown-queue-rejected|_poll_until#21|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last observed: {last!r})")
Issue-Key: batch.array-job-children-receive-distinct-array-index|run#11|AssertionError|assert summary.get("SUCCEEDED") == 1, (
Issue-Key: batch.job-dependency-releases-on-success-and-blocks-on-parent-failure|run#10|AssertionError|assert held["status"] == "PENDING", (
Issue-Key: batch.batch-array-n-to-n-dependency-gates-only-matching-child-index|run#18|AssertionError|assert job["status"] == "PENDING", (
Issue-Key: batch.job-attempt-duration-timeout-forces-failed|run#12|AssertionError|assert "timed out" in reason.lower() or "timeout" in reason.lower(), (
Issue-Key: batch.job-attempt-duration-timeout-terminates-job|run#14|AssertionError|assert "timeout" in reason or "timed out" in reason, f"unexpected statusReason: {final.get('statusReason')!r}"
Issue-Key: batch.job-attempt-duration-timeout-terminates-running-job|run#18|AssertionError|raise AssertionError(
Issue-Key: batch.job-timeout-attribute-triggers-timeout-failure|run#16|AssertionError|assert running["status"] == "RUNNING", f"job never reached RUNNING before terminating: {running['status']}/{running.get('statusReason')}"
Issue-Key: batch.job-retry-strategy-evaluate-on-exit-controls-attempt-count|run#13|AssertionError|assert len(rattempts) == 2, (
Issue-Key: batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries|run#10|KeyError|u_attempts = unmatched["attempts"]
Issue-Key: batch.e2e-service-job-lifecycle-through-queue-snapshot|_poll_until#11|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last}")
Issue-Key: batch.job-queue-snapshot-includes-service-job-entries|_poll_until#9|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last}")
Issue-Key: batch.jq-snapshot-and-list-jobs-status-filter|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: batch.fair-share-scheduling-enforces-quota-allocation-server-side|_wait_until#24|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Key: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job|_poll_until#29|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last value={last!r}")
Issue-Key: batch.job-queue-snapshot-caps-front-of-queue-at-100|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs|_poll_until#21|Timeout|raise AssertionError(f"condition not met within {timeout_s}s (last observed: {last!r})")
Issue-Commit: e68df329f2ba7326cf40e2227a2656e4e66e8fc6

# batch: the job data plane never executes a job — placed jobs go SUBMITTED→FAILED with an emulator-authored reason, and dependsOn, timeout, retryStrategy, array statusSummary and GetJobQueueSnapshot are all unimplemented (6 causes)

> _Note: this issue was identified against the code base at commit `e68df329f2ba7326cf40e2227a2656e4e66e8fc6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Six net-new root causes in the AWS Batch job data plane, all grounded in failing behavioral checks the suite listed under `### batch` and none of them marked ALREADY FILED or OWNED. Related open issues exist for other service tokens and other symptoms and do NOT cover these checks: 20261104T041055Z-batch-array-jobs-unimplemented-managed-ces-have-no.md (array submission / MANAGED CEs / refusal contracts), 20261107T153156Z-batch-logs-a-placed-batch-job-ends-failed.md (batch__logs), 20261106T090000Z-batch-s3-a-placed-job-s-container-is-never-run.md (batch__s3), 20261105T005839Z-batch-a-job-submitted-to-a-cloudformation-deployed.md and 20261106T061928Z-batch-a-job-submitted-to-a-cloudformation-deployed-d662c3.md (one CFN-deployed queue scenario).

=== Cause 1 of 6: a placed job is failed outright instead of being run; no RUNNABLE/STARTING/RUNNING, no exitCode ===
AWS: Smithy: JobDetail.status targets JobStatus enum [SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED]; a job submitted to an ENABLED queue with a VALID compute environment advances SUBMITTED -> (PENDING) -> RUNNABLE -> STARTING -> RUNNING -> SUCCEEDED when its container exits 0, and -> FAILED when it exits non-zero. JobDetail.container is a ContainerDetail whose `exitCode` is documented "The exit code returned upon completion" and whose `reason` is "A short (255 max characters) human-readable string to provide additional details for a running or stopped container". JobDetail.statusReason is "A short, human-readable string to provide more details for the current status of the job" and the model enumerates the failure reasons AWS actually emits for an unplaceable job: CAPACITY:INSUFFICIENT_INSTANCE_CAPACITY, MISCONFIGURATION:COMPUTE_ENVIRONMENT_MAX_RESOURCE, MISCONFIGURATION:JOB_RESOURCE_REQUIREMENT, MISCONFIGURATION:SERVICE_ROLE_PERMISSIONS. A successful job's status trace never contains FAILED, and a job that fails on a non-zero exit reports that integer in container.exitCode.
Emulator: every submitted container job settles as status 'FAILED' with the trace ['SUBMITTED','FAILED'] — it never reports RUNNABLE, STARTING or RUNNING — and both JobDetail.statusReason and container.reason are set to the emulator-authored sentence "This emulator does not run containers, so the job's container was never started". container.exitCode is absent (None) even where the recording has a real process status (expected 17, got None). This is not one of AWS's reason strings and no such sentence exists in the API at all.
Why: a client cannot run a single Batch workload: waiters on RUNNING/SUCCEEDED never fire, status machines see an immediate terminal FAILED, and any retry/alarm logic keyed on container.exitCode or on AWS's documented CAPACITY:/MISCONFIGURATION: reason prefixes matches nothing. Every scheduling feature layered above execution (consumable resources, fair-share dispatch ordering, CE enable/disable placement) is unobservable because no job ever occupies capacity.
Fix: the job-execution/state-machine component behind SubmitJob — the placement and attempt lifecycle that should advance a placed job through RUNNABLE/STARTING/RUNNING to a terminal state and record the container's exit status, plus the DescribeJobs serializer that must then populate container.exitCode. The sentinel statusReason must be removed; where a job genuinely cannot be placed, the reason must be one of the modeled CAPACITY:/MISCONFIGURATION: strings.
Traces: batch.batch-job-runs-to-succeeded, batch.job-runs-to-succeeded-with-exit-code-and-status-sequence, batch.failing-job-reports-real-nonzero-exit-code, batch.job-parameter-substitution-and-container-override-precedence, batch.compute-environment-disabled-withholds-placement-until-reenabled, batch.concurrent-consumable-resource-allocation-enforces-capacity, batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs, batch.fair-share-quota-weight-orders-job-dispatch, batch.job-queue-snapshot-reports-accurate-running-and-runnable-counts, batch.quota-share-enforced-server-side-on-job-scheduling, batch.get-job-queue-snapshot-unknown-queue-rejected.

=== Cause 2 of 6: an array parent's arrayProperties.statusSummary never counts a succeeding child, and children do not receive distinct indices ===
AWS: Smithy: ArrayPropertiesDetail.statusSummary is "A summary of the number of array job children in each available job status. This parameter is returned for parent array jobs", and ArrayPropertiesDetail.index is "The job index within the array that's associated with this job". Each child runs with AWS_BATCH_JOB_ARRAY_INDEX set to its own index, so an array of size N whose per-index command succeeds leaves statusSummary with that many SUCCEEDED children and 0 FAILED.
Emulator: the parent's statusSummary reports {'SUCCEEDED': 0, 'PENDING': 0, 'STARTING': 0, 'RUNNING': 0, 'SUBMITTED': 0, 'FAILED': 2, 'RUNNABLE': 0} — every child goes straight to FAILED, so no index-dependent work ever runs and the SUCCEEDED bucket stays 0.
Why: a client fanning work out over array indices sees the whole array fail and can never read a per-index result; statusSummary, the documented way to track array progress, reports a count that contradicts the children's work.
Fix: the array-job child spawning and per-child index injection in the SubmitJob/array handler, and the parent statusSummary aggregation that rolls child statuses up into arrayProperties.
Traces: batch.array-job-children-receive-distinct-array-index.

=== Cause 3 of 6: dependsOn is never enforced — a dependent job is not held in PENDING ===
AWS: Smithy: SubmitJob takes dependsOn (JobDependencyList of JobDependency{jobId, type}) and JobDetail.dependsOn is "A list of job IDs that this job depends on"; ArrayJobDependency supports N_TO_N and SEQUENTIAL. A job whose dependency has not reached SUCCEEDED is held in PENDING and only becomes RUNNABLE when the dependency succeeds; if the dependency fails, the dependent job is failed and never run. For an N_TO_N array dependency the child at index i waits specifically on the parent's child at index i.
Emulator: the dependent job is never held. A plain dependsOn job is reported 'FAILED' while its parent is unfinished (expected PENDING), and an N_TO_N array child is reported 'SUBMITTED' while its parent has not finished (expected PENDING).
Why: dependency graphs silently collapse — downstream jobs start (or fail) before their inputs exist, so a client's ordering guarantee is lost and the PENDING state a client polls for is never observable.
Fix: the dependency gate in the job scheduler/state machine — dependsOn evaluation (including the N_TO_N and SEQUENTIAL index pairing) must hold the job in PENDING until the dependency resolves.
Traces: batch.job-dependency-releases-on-success-and-blocks-on-parent-failure, batch.batch-array-n-to-n-dependency-gates-only-matching-child-index.

=== Cause 4 of 6: timeout.attemptDurationSeconds is never enforced and no timeout statusReason is produced ===
AWS: Smithy: JobTimeout.attemptDurationSeconds — "The job timeout time (in seconds) that's measured from the job attempt's startedAt timestamp. After this time passes, Batch terminates your jobs if they aren't finished. The minimum value for the timeout is 60 seconds." AWS terminates the over-running attempt and the job ends FAILED with a statusReason stating the attempt exceeded its timeout (the text names the timeout condition; the duration value comes from the request).
Emulator: the job never reaches RUNNING at all, and the terminal statusReason is the sentinel "This emulator does not run containers, so the job's container was never started" instead of any timeout text — the check asserting a reason containing "timeout"/"timed out" sees that sentence, and the running-job variants report FAILED where RUNNING was a precondition.
Why: a client cannot distinguish "my job exceeded its attempt timeout" from any other failure; timeout-driven retry and alerting keyed on the timeout reason never triggers, and the timeout itself is not applied so a long-running job would never be terminated.
Fix: the attempt-timeout enforcement in the job attempt lifecycle (terminate the attempt at startedAt + attemptDurationSeconds) and the statusReason it sets on that path.
Traces: batch.job-attempt-duration-timeout-forces-failed, batch.job-attempt-duration-timeout-terminates-job, batch.job-attempt-duration-timeout-terminates-running-job, batch.job-timeout-attribute-triggers-timeout-failure.

=== Cause 5 of 6: DescribeJobs reports no `attempts` list, and retryStrategy/evaluateOnExit is never applied ===
AWS: Smithy: JobDetail.attempts targets AttemptDetails — "A list of job attempts that are associated with this job" — each AttemptDetail carrying container, startedAt, stoppedAt and statusReason. RetryStrategy.attempts is "The number of times to move a job to the RUNNABLE status. You can specify between 1 and 10 attempts. If the value of attempts is greater than one, the job is retried on failure the same number of attempts as the value", and EvaluateOnExit documents that "If none of the listed conditions match, then the job is retried" — an onExitCode condition with action RETRY and attempts=2 therefore yields exactly 2 AttemptDetail entries, and an unmatched condition still retries.
Emulator: `attempts` is absent from the DescribeJobs response — reading it raises KeyError: 'attempts' — and where it is defaulted the job records 0 attempts for a retryStrategy of attempts=2 with onExitCode 3 -> RETRY. No retry is performed.
Why: a client has no per-attempt record at all (no attempt start/stop time, no attempt container detail or reason), so attempt-level diagnosis is impossible; and a job that AWS would retry is failed on its first and only pass, so retry-based resilience silently disappears.
Fix: the attempt recording in the job state/DescribeJobs serializer (always emit `attempts` with one AttemptDetail per attempt once an attempt exists) and the retryStrategy/evaluateOnExit evaluation in the job lifecycle that re-queues the job to RUNNABLE.
Traces: batch.job-retry-strategy-evaluate-on-exit-controls-attempt-count, batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries.

=== Cause 6 of 6: GetJobQueueSnapshot always answers an empty snapshot ===
AWS: Smithy: GetJobQueueSnapshotResponse.frontOfQueue is FrontOfQueueDetail whose `jobs` holds "The Amazon Resource Names (ARNs) of the first 100 RUNNABLE jobs in a named job queue. For first-in-first-out (FIFO) job queues, jobs are ordered based on their submission time. For fair-share scheduling (FSS) job queues, jobs are ordered based on their job priority and share usage" (each FrontOfQueueJobSummary carrying jobArn and earliestTimeAtPosition); `frontOfQuotaShares` is "The first RUNNABLE job in each quota share"; `queueUtilization` reports "The job queue's capacity utilization, including total usage and breakdown per given share". A queue holding RUNNABLE container jobs or service jobs therefore reports them, capped at 100, ordered and deduplicated per array parent, with non-empty totalCapacityUsage.
Emulator: the snapshot is permanently empty for every queue — HTTP 200 with {'frontOfQueue': {'jobs': [], 'lastUpdatedAt': <ms>}, 'queueUtilization': {'totalCapacityUsage': [], 'lastUpdatedAt': <ms>}} and, where the quota-share member is emitted, 'frontOfQuotaShares': {'quotaShares': {}} — while jobs sit queued (and while service jobs are RUNNABLE). Checks polling for any entry exhaust their 90s/120s budget, and the front-of-queue cap/order checks run to the 180s harness timeout. (The lastUpdatedAt epochs above are per-run values, not requirements.)
Why: the only API for inspecting queue depth, dispatch order and capacity usage reports an idle queue no matter what is enqueued, so a client cannot observe backlog, priority/fair-share ordering or utilization — and autoscaling or dashboards built on it read zero load under full load.
Fix: the GetJobQueueSnapshot handler and the queue state it reads — it must project the queue's RUNNABLE jobs (container and service jobs) into frontOfQueue.jobs capped at 100 and deduplicated per array parent in the queue's scheduling order, populate frontOfQuotaShares for quota-share queues, and compute queueUtilization.totalCapacityUsage from placed jobs.
Traces: batch.e2e-service-job-lifecycle-through-queue-snapshot, batch.job-queue-snapshot-includes-service-job-entries, batch.jq-snapshot-and-list-jobs-status-filter, batch.fair-share-scheduling-enforces-quota-allocation-server-side, batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job, batch.job-queue-snapshot-caps-front-of-queue-at-100, batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs.

## Evidence
Suite: 159/265 passed; service `batch` 152/232 (66%), 80 failed. The 27 failures below were neither ALREADY FILED nor OWNED.

Cause 1 — expected a job to reach RUNNING/SUCCEEDED and report its exit code; got an immediate terminal FAILED with a non-AWS reason:
  AssertionError: DATAPLANE_UNIMPLEMENTED: job <id> ended in 'FAILED' (expected SUCCEEDED); reason="This emulator does not run containers, so the job's container was never started"
  AssertionError: job <id> ended 'FAILED', expected SUCCEEDED; statusReason="This emulator does not run containers, so the job's container was never started" trace=['SUBMITTED', 'FAILED']
  AssertionError: expected container.exitCode == 17 (the real process status), got None
  AssertionError: the default parameter code=0 must produce exit 0, got 'FAILED' reason="This emulator does not run containers, ..."
  AssertionError: the control job <id> on the ENABLED compute environment FAILED before placement: "This emulator does not run containers, ..."
  AssertionError: warm-up job <id> FAILED: "This emulator does not run containers, ..."
  AssertionError: at least one observation must have shown a job RUNNING ... trace was [('SUBMITTED','SUBMITTED'), ('FAILED','FAILED')]
  AssertionError: job <id> reached unexpected status FAILED: This emulator does not run containers, so the job's container was never started
  AssertionError: condition not met within 120.0s (last observed: {'<id>': 'FAILED', '<id>': 'FAILED', '<id>': 'FAILED'})
  AssertionError: condition not met within 90.0s (last observed: 'FAILED')
  Expected status sequence (AWS): SUBMITTED -> RUNNABLE -> STARTING -> RUNNING -> SUCCEEDED. Actual: SUBMITTED -> FAILED.

Cause 2 — array parent statusSummary:
  Expected: exactly 1 SUCCEEDED child. Actual: {'SUCCEEDED': 0, 'PENDING': 0, 'STARTING': 0, 'RUNNING': 0, 'SUBMITTED': 0, 'FAILED': 2, 'RUNNABLE': 0}

Cause 3 — dependsOn:
  Expected: a job with an unsatisfied dependsOn is held in PENDING. Actual: 'FAILED'.
  Expected: a dependent array child whose N_TO_N parent has not finished is held in PENDING. Actual: 'SUBMITTED'.

Cause 4 — attempt timeout:
  Expected: statusReason naming the attempt timeout. Actual: "This emulator does not run containers, so the job's container was never started".
  Expected: job RUNNING before the attempt timeout fires. Actual: FAILED with that same sentinel reason.

Cause 5 — attempts / retryStrategy:
  Expected: JobDetail.attempts present with one AttemptDetail per attempt. Actual: KeyError: 'attempts' (member absent).
  Expected: onExitCode 3 -> RETRY with attempts=2 records exactly 2 attempts. Actual: 0 attempts; container.reason = "This emulator does not run containers, ...".

Cause 6 — GetJobQueueSnapshot:
  Expected: frontOfQueue.jobs lists the queue's RUNNABLE jobs (first 100, ordered, deduplicated per array parent); frontOfQuotaShares lists the first RUNNABLE job per quota share; queueUtilization.totalCapacityUsage non-empty under load.
  Actual (HTTP 200): {'frontOfQueue': {'jobs': [], 'lastUpdatedAt': <ms>}, 'queueUtilization': {'totalCapacityUsage': [], 'lastUpdatedAt': <ms>}}; with the FSS member present: 'frontOfQuotaShares': {'quotaShares': {}}. Polls exhausted at 90s/120s; the cap-at-100 and front-of-queue-ordering checks hit the 180s harness timeout.

## Steps to Reproduce
Preconditions: an IAM Fargate task execution role, a MANAGED/FARGATE compute environment polled to VALID, an ENABLED job queue polled to VALID, and a registered Fargate job definition (image public.ecr.aws/docker/library/busybox:1.36).

Cause 1: SubmitJob with containerOverrides command ['/bin/sh','-c','exit 0'], then poll DescribeJobs. Observe status goes SUBMITTED -> FAILED with statusReason "This emulator does not run containers, so the job's container was never started"; RUNNABLE/STARTING/RUNNING never appear. Repeat with 'exit 17' and observe container.exitCode absent instead of 17.

Cause 2: SubmitJob with arrayProperties.size=2 and a command keyed on AWS_BATCH_JOB_ARRAY_INDEX, then DescribeJobs on the parent and read arrayProperties.statusSummary: FAILED=2, SUCCEEDED=0.

Cause 3: SubmitJob job A (long-running), then SubmitJob job B with dependsOn=[{jobId: A}] and DescribeJobs B immediately — B is not PENDING. Repeat with an array parent and a child job carrying dependsOn=[{jobId: parent, type: 'N_TO_N'}] and arrayProperties.size matching — the child reports SUBMITTED, not PENDING.

Cause 4: SubmitJob with timeout={'attemptDurationSeconds': 60} and a command that sleeps longer, then poll DescribeJobs — the job never reaches RUNNING and the terminal statusReason is the no-container-runtime sentinel rather than a timeout reason.

Cause 5: SubmitJob with retryStrategy={'attempts': 2, 'evaluateOnExit': [{'onExitCode': '3', 'action': 'RETRY'}]} and command 'exit 3', then DescribeJobs — the response has no `attempts` member (KeyError), and no retry occurs.

Cause 6: SubmitJob (and SubmitServiceJob to a service queue) so jobs are queued, then call GetJobQueueSnapshot on that queue repeatedly — frontOfQueue.jobs, frontOfQuotaShares.quotaShares and queueUtilization.totalCapacityUsage stay empty for the whole polling window. Submitting more than 100 jobs to check the cap, or an array parent to check deduplication, never yields a non-empty frontOfQueue.
