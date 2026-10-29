Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.delete-referenced-compute-environment-fails batch.service-environment-delete-while-attached-to-queue-fails batch.delete-scheduling-policy-attached-to-queue-fails batch.describe-scheduling-policies-contract batch.list-quota-shares-invalid-filter-rejected batch.op-contract-update-scheduling-policy batch.jq-create-with-unknown-scheduling-policy-fails batch.submit-job-with-inactive-definition-fails batch.service-job-lifecycle-state-transitions batch.service-job-lifecycle-transitions batch.submit-service-job-progresses-to-running batch.e2e-service-job-lifecycle-through-queue-snapshot batch.e2e-service-job-quota-share-consumption-flow batch.fair-share-scheduling-enforces-quota-allocation-server-side batch.job-queue-snapshot-includes-service-job-entries batch.job-queue-snapshot-reflects-quota-share-usage batch.jq-snapshot-and-list-jobs-status-filter batch.quota-share-enforced-server-side-on-job-scheduling
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.delete-referenced-compute-environment-fails|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: batch.service-environment-delete-while-attached-to-queue-fails|run#8|AssertionError|assert message == "Cannot delete, found existing JobQueue relationship.", (
Issue-Key: batch.delete-scheduling-policy-attached-to-queue-fails|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: batch.describe-scheduling-policies-contract|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.list-quota-shares-invalid-filter-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.op-contract-update-scheduling-policy|run#17|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: batch.jq-create-with-unknown-scheduling-policy-fails|_client_error#11|AssertionError|raise AssertionError(
Issue-Key: batch.submit-job-with-inactive-definition-fails|run#16|AssertionError|assert message == f"JobDefinition {jd_arn} is not in ACTIVE status.", f"expected the exact INACTIVE-definition refusal message, got {message!r}"
Issue-Key: batch.service-job-lifecycle-state-transitions|run#15|AssertionError|raise AssertionError(
Issue-Key: batch.service-job-lifecycle-transitions|run#10|AssertionError|assert final["status"] == "SUCCEEDED", f"job ended with status {final['status']}: {final.get('statusReason')}"
Issue-Key: batch.submit-service-job-progresses-to-running|run#17|AssertionError|raise AssertionError(f"service job unexpectedly went FAILED before RUNNING: {exc}") from exc
Issue-Key: batch.e2e-service-job-lifecycle-through-queue-snapshot|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: batch.e2e-service-job-quota-share-consumption-flow|_wait_for#15|Timeout|time.sleep(interval_s)
Issue-Key: batch.fair-share-scheduling-enforces-quota-allocation-server-side|_wait_until#24|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-queue-snapshot-includes-service-job-entries|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-queue-snapshot-reflects-quota-share-usage|_wait_for#17|Timeout|time.sleep(interval_s)
Issue-Key: batch.jq-snapshot-and-list-jobs-status-filter|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: batch.quota-share-enforced-server-side-on-job-scheduling|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# batch: 8 net-new root causes — relationship-delete and range-validation refusal text is emulator-authored, CreateJobQueue accepts an unknown scheduling policy, and no service job is ever dispatched so ten snapshot/fair-share checks reach no verdict

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitServiceJob

## Description
Eight further independent root causes in AWS Batch, none marked ALREADY FILED or OWNED in the replay report. Cause 8 is the same family as the open issue 20261027T090000Z-batch-a-sagemaker-training-service-job-is-never-dispatched.md (which owns only batch.deploy-compute-environment-cloudformation-behaves) and the open 20261024T120647Z-batch-overly-permissive-control-and-service-job-planes.md; neither carries any of the ten check ids listed under it, so it is filed here with its own replay keys. Cause 6 is adjacent to the resolved 20261024T191619Z-batch-updateschedulingpolicy-refusal-messages-diverge-from-aws-missing.md, which fixed other UpdateSchedulingPolicy messages but not the computeReservation range message.

Cause 1 of 8 — the "existing JobQueue relationship" refusal appends the referencing queue's name.
AWS: Smithy: DeleteComputeEnvironment and DeleteServiceEnvironment declare ClientException (400) / ServerException (500) and no message text, so this is recorded behavior only. The recordings show ClientException whose message is a CLOSED, fully fixed sentence with no variable part at all: "Cannot delete, found existing JobQueue relationship." (terminating period included). The referencing queue is deliberately not named.
Emulator: answers 'Cannot delete, found existing JobQueue relationship: <jobQueueName>' — the same sentence with its period replaced by a colon and the referencing queue appended, for both DeleteComputeEnvironment and DeleteServiceEnvironment.
Why: a client matching the refusal on AWS's exact text never recognises the refusal, and the emulator leaks a resource name into a message AWS keeps opaque.
Fix: the message string in the referenced-resource guard of the DeleteComputeEnvironment and DeleteServiceEnvironment handlers.
Traces: batch.delete-referenced-compute-environment-fails, batch.service-environment-delete-while-attached-to-queue-fails.

Cause 2 of 8 — DeleteSchedulingPolicy's attached-to-queue refusal uses a different sentence entirely.
AWS: Smithy: DeleteSchedulingPolicy declares ClientException/ServerException with no message text; recorded behavior only. The recording shows the SAME relationship sentence as cause 1, "Cannot delete, found existing JobQueue relationship" — fixed text, no variable part.
Emulator: answers "Cannot delete a scheduling policy that's used in any job queues." — an emulator-authored sentence that shares no text with AWS's.
Why: a client cannot handle all three relationship refusals (compute environment, service environment, scheduling policy) with one matcher the way AWS's uniform wording allows.
Fix: the message string in the DeleteSchedulingPolicy handler's in-use guard.
Traces: batch.delete-scheduling-policy-attached-to-queue-fails.

Cause 3 of 8 — DescribeSchedulingPolicies' arns-count refusal renders the generic framework constraint message.
AWS: Smithy: DescribeSchedulingPolicies.arns -> StringList is a required list carrying no length trait, so the 1..100 bound is recorded behavior only. Recorded response: ClientException with "Error executing request, Exception : Number of scheduling policies should be between 1 and 100., RequestId: <this call's request id>". Fixed text is everything but the request id; the limits 1 and 100 are part of the fixed text.
Emulator: answers 'arns accepts at most 100 entries, not 101' — the request-validation framework's own generic phrasing, with no envelope and no request id, and it names the submitted count, which AWS's message never does.
Why: the refusal is unrecognisable to a client keyed on AWS's text, and the absent envelope makes it look like a different error class than the one Batch actually raises.
Fix: the arns-count validation in the DescribeSchedulingPolicies handler must raise Batch's own ClientException message instead of deferring to the generic list-length constraint renderer.
Traces: batch.describe-scheduling-policies-contract.

Cause 4 of 8 — ListQuotaShares' maxResults range refusal renders the generic framework constraint message.
AWS: Smithy: ListQuotaShares.maxResults -> Integer, no range trait modelled (the operation is @paginated with pageSize maxResults), so recorded behavior only. Recorded response: ClientException "Error executing request, Exception : Max Results should be greater than or equal to 1 and less than or equal to 100., RequestId: <this call's request id>". Fixed text is everything but the request id; the bounds 1 and 100 belong to the fixed text and the submitted value is NOT echoed.
Emulator: answers 'maxResults 0 must be between 1 and 100' — generic phrasing, no envelope, no request id, and it echoes the submitted value.
Why: same as cause 3 — an unrecognisable message for a modelled pagination refusal.
Fix: the maxResults validation in the ListQuotaShares handler.
Traces: batch.list-quota-shares-invalid-filter-rejected.

Cause 5 of 8 — UpdateSchedulingPolicy's computeReservation range refusal renders the generic framework constraint message.
AWS: Smithy: UpdateSchedulingPolicy.fairsharePolicy -> FairsharePolicy; the computeReservation bound is recorded behavior only. Recorded response: ClientException "Error executing request, Exception : Scheduling policy compute reservation must be within valid range [0, 99]., RequestId: <this call's request id>". Fixed text is everything but the request id, including the bracketed range.
Emulator: answers 'fairsharePolicy computeReservation must be between 0 and 99, got 150' — generic phrasing, no envelope, no request id, and it echoes the submitted value.
Why: same as causes 3 and 4; and because three different validators all bypass Batch's envelope, no client can rely on the envelope at all.
Fix: the fairsharePolicy.computeReservation validation in the UpdateSchedulingPolicy handler.
Traces: batch.op-contract-update-scheduling-policy.

Cause 6 of 8 — CreateJobQueue accepts a schedulingPolicyArn that does not exist.
AWS: Smithy: CreateJobQueue.schedulingPolicyArn is an optional String; the referential check is recorded behavior only. Recorded response for a well-formed scheduling-policy ARN that names no existing policy: HTTP 400 ClientException with message "SchedulingPolicy <the schedulingPolicyArn from the request> not found." — fixed text "SchedulingPolicy " and " not found.", variable part the ARN taken from the request. No queue is created.
Emulator: returns HTTP 200 and mints the queue ({'jobQueueName': ..., 'jobQueueArn': ...}) bound to a scheduling policy that does not exist.
Why: a queue referencing a nonexistent fair-share policy is accepted into state, so its fair-share scheduling can never behave as configured and the misconfiguration is never reported at create time — the one moment the client could fix it.
Fix: resolve schedulingPolicyArn in the CreateJobQueue handler's validation before any state is written, and leave no queue behind when it does not resolve.
Traces: batch.jq-create-with-unknown-scheduling-policy-fails.

Cause 7 of 8 — SubmitJob's INACTIVE-job-definition refusal merges the not-found and not-ACTIVE classes.
AWS: Smithy: SubmitJob.jobDefinition is a required String; message text is recorded behavior only. The recording shows ClientException with "JobDefinition <the jobDefinition ARN of the deregistered revision> is not in ACTIVE status." — fixed text "JobDefinition " and " is not in ACTIVE status.", variable part the definition's ARN (the ARN form, not the name:revision form).
Emulator: answers 'JobDefinition name <name>:<revision> does not exist or is not in ACTIVE status.' — a merged sentence that names the definition by name:revision instead of by ARN and cannot distinguish a definition that was deregistered from one that never existed.
Why: a client that submits against a deregistered revision is told the definition may not exist, so it re-registers (creating a new revision) instead of reactivating or correcting the revision it meant.
Fix: the job-definition status guard in the SubmitJob handler — the not-found and not-ACTIVE refusals need distinct messages, and the ACTIVE one must name the definition by ARN.
Traces: batch.submit-job-with-inactive-definition-fails.

Cause 8 of 8 — a service job is never dispatched, so no reader observes RUNNABLE/RUNNING and ten checks reach no verdict.
AWS: Smithy: SubmitServiceJob returns a jobArn/jobId/jobName and DescribeServiceJob reports a status; GetJobQueueSnapshot reports frontOfQueue.jobs; ListServiceJobs filters by jobStatus. Recorded behavior: a service job submitted to a VALID/ENABLED SAGEMAKER_TRAINING queue progresses SUBMITTED -> RUNNABLE -> RUNNING -> SUCCEEDED, appears in GetJobQueueSnapshot's frontOfQueue while RUNNABLE, disappears from it once terminal, and a terminated job appears under ListServiceJobs(jobStatus=FAILED).
Emulator: SubmitServiceJob is accepted with HTTP 200, then DescribeServiceJob reports status 'FAILED' with the emulator-authored statusReason 'This emulator does not run SageMaker AI, so the service job was never dispatched'. The job therefore never becomes RUNNABLE or RUNNING: three checks fail on that status/statusReason outright, and seven more poll GetJobQueueSnapshot / DescribeServiceJob / ListServiceJobs for a progression that can never arrive and run out the harness's 60s deadline, so the fair-share, quota-share-consumption and queue-snapshot contracts are not merely wrong, they are unobservable.
Why: every client of the service-job plane sees an immediate, unexplained FAILED; no fair-share or quota-share enforcement can be exercised; and a statusReason that names the emulator leaks the emulation into a field clients log and parse.
Fix: service-job dispatch and state progression in the batch crate's service-job state machine (SubmitServiceJob -> scheduler -> DescribeServiceJob/GetJobQueueSnapshot/ListServiceJobs projections), plus removal of the emulator-authored statusReason.
Traces: batch.service-job-lifecycle-state-transitions, batch.service-job-lifecycle-transitions, batch.submit-service-job-progresses-to-running, batch.e2e-service-job-lifecycle-through-queue-snapshot, batch.e2e-service-job-quota-share-consumption-flow, batch.fair-share-scheduling-enforces-quota-allocation-server-side, batch.job-queue-snapshot-includes-service-job-entries, batch.job-queue-snapshot-reflects-quota-share-usage, batch.jq-snapshot-and-list-jobs-status-filter, batch.quota-share-enforced-server-side-on-job-scheduling.

## Evidence
Cause 1: expected message 'Cannot delete, found existing JobQueue relationship.', got 'Cannot delete, found existing JobQueue relationship: sim2real-behav-jq-<id>' (DeleteComputeEnvironment) and 'Cannot delete, found existing JobQueue relationship: sim2real-behav-sjq-<id>' (DeleteServiceEnvironment).
Cause 2: expected 'Cannot delete, found existing JobQueue relationship', got "Cannot delete a scheduling policy that's used in any job queues."
Cause 3: expected 'Error executing request, Exception : Number of scheduling policies should be between 1 and 100., RequestId: <id>', got 'arns accepts at most 100 entries, not 101'.
Cause 4: expected 'Error executing request, Exception : Max Results should be greater than or equal to 1 and less than or equal to 100., RequestId: <id>', got 'maxResults 0 must be between 1 and 100'.
Cause 5: expected 'Error executing request, Exception : Scheduling policy compute reservation must be within valid range [0, 99]., RequestId: <id>', got 'fairsharePolicy computeReservation must be between 0 and 99, got 150'.
Cause 6: CreateJobQueue(jobQueueType='SAGEMAKER_TRAINING', schedulingPolicyArn='arn:aws:batch:<region>:<account>:scheduling-policy/<name that does not exist>') returned HTTP 200 {'jobQueueName': 'sim2real-behav-sjq-<id>', 'jobQueueArn': '...'}; expected ClientException 'SchedulingPolicy <arn> not found.' and no queue left behind.
Cause 7: expected 'JobDefinition <jobDefinitionArn> is not in ACTIVE status.', got 'JobDefinition name sim2real-behav-jd-<id>:1 does not exist or is not in ACTIVE status.'
Cause 8: DescribeServiceJob reported status 'FAILED' with statusReason 'This emulator does not run SageMaker AI, so the service job was never dispatched' (batch.service-job-lifecycle-transitions, batch.service-job-lifecycle-state-transitions, batch.submit-service-job-progresses-to-running); the seven snapshot/fair-share/quota-consumption checks all ended 'Check timed out after 60.0s' while polling GetJobQueueSnapshot for the submitted job in frontOfQueue (timeout_s=90) or DescribeServiceJob for RUNNING.

## Steps to Reproduce
Replay the behavioral suite against the emulator and run exactly the checks in trace_ids.
1. Cause 1: create a compute environment and a job queue referencing it, then DeleteComputeEnvironment; separately create a SAGEMAKER_TRAINING service environment and a queue referencing it, then DeleteServiceEnvironment. Read Error.Message on both.
2. Cause 2: create a scheduling policy, attach it to a job queue via CreateJobQueue(schedulingPolicyArn=...), then DeleteSchedulingPolicy; read Error.Message.
3. Cause 3: DescribeSchedulingPolicies with 101 ARNs; read Error.Message.
4. Cause 4: ListQuotaShares(jobQueue=<a VALID queue>, maxResults=0); read Error.Message.
5. Cause 5: UpdateSchedulingPolicy(arn=<policy>, fairsharePolicy={'computeReservation': 150, ...}); read Error.Message.
6. Cause 6: CreateJobQueue(jobQueueType='SAGEMAKER_TRAINING', serviceEnvironmentOrder=[...], schedulingPolicyArn='arn:aws:batch:<region>:<account>:scheduling-policy/<absent name>'); expect ClientException and DescribeJobQueues to find nothing.
7. Cause 7: RegisterJobDefinition, DeregisterJobDefinition it, then SubmitJob against that revision; read Error.Message.
8. Cause 8: create a SAGEMAKER_TRAINING service environment and ENABLED queue, SubmitServiceJob, then poll DescribeServiceJob for RUNNABLE/RUNNING and GetJobQueueSnapshot for the job in frontOfQueue; also poll ListServiceJobs(jobStatus=FAILED) after TerminateServiceJob.
