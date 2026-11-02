Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.create-quota-share-missing-required-field-rejected batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected batch.submit-job-to-disabled-queue-fails batch.service-job-submit-to-disabled-queue-fails batch.submit-service-job-missing-quota-share-name-rejected batch.submit-service-job-missing-required-quota-share-name-rejected batch.terminate-service-job-without-permission-denied batch.service-job-lifecycle-state-transitions batch.service-job-managed-execution-reaches-succeeded batch.e2e-service-job-lifecycle-through-queue-snapshot batch.jq-snapshot-and-list-jobs-status-filter
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.create-quota-share-missing-required-field-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected|run#18|AssertionError|assert err.get("Message") == expected, f"expected {expected!r}, got {err.get('Message')!r}"
Issue-Key: batch.submit-job-to-disabled-queue-fails|run#19|AssertionError|assert message == f"JobQueue {jq_arn} not ENABLED.", (
Issue-Key: batch.service-job-submit-to-disabled-queue-fails|_client_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: batch.submit-service-job-missing-quota-share-name-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: batch.submit-service-job-missing-required-quota-share-name-rejected|_error#22|AssertionError|raise AssertionError(
Issue-Key: batch.terminate-service-job-without-permission-denied|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: batch.service-job-lifecycle-state-transitions|run#17|AssertionError|raise AssertionError(
Issue-Key: batch.service-job-managed-execution-reaches-succeeded|run#29|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: batch.e2e-service-job-lifecycle-through-queue-snapshot|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: batch.jq-snapshot-and-list-jobs-status-filter|_poll_until#9|Timeout|last = probe()
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# batch: 7 net-new root causes — three refusal messages are emulator-authored, three refusals AWS makes are never made, and no service job is ever dispatched

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitServiceJob

## Description
SEVEN distinct root causes in the AWS Batch emulator, each grounded in failing checks of the simparity-behavioral-aws-v1 replay that the report marks neither ALREADY FILED nor OWNED. Each cause is numbered; a fixer must close all seven before submitting.

Related open issues that do NOT carry these replay keys and therefore do not cover these checks (named here so the overlap is visible, not because they close anything): '20261024T120647Z-batch-overly-permissive-control-and-service-job-planes.md', '20261029T173319Z-batch-8-net-new-root-causes-relationship-delete.md' and '20261027T090000Z-batch-a-sagemaker-training-service-job-is-never-dispatched.md'. The replay keys of the eleven checks listed in trace_ids have drifted away from the keys those issues carry (the check bodies moved, and the timeout checks now report a bare harness timeout rather than a keyed poll frame), so the suite report does not attribute any of them to those issues and they are re-filed here with their current grounding. Causes 4, 5, 6 and 7 are the same defect FAMILIES those issues describe; if a fixer closes them there, these checks are the ones that must be replayed to prove it.

=== Cause 1 of 7: CreateQuotaShare's empty-capacityLimits refusal message is emulator-authored ===
AWS: Smithy com.amazonaws.batch#CreateQuotaShare binds POST /v1/createquotashare (success 200) and models exactly ClientException (httpError 400) and ServerException (500); capacityLimits -> QuotaShareCapacityLimits is a required list carrying no @length trait, so the non-empty rule and its wording are recorded behavior only. Recorded refusal for capacityLimits=[]: HTTP 400, error code ClientException, Error.Message = 'Error executing request, Exception : capacityLimits is required and cannot be empty., RequestId: <this call's request id>'. Fixed text: the whole envelope 'Error executing request, Exception : ' + 'capacityLimits is required and cannot be empty.' + ', RequestId: '; the only variable part is the request id of the call.
Emulator: same status and code, but Error.Message = 'Error executing request, Exception : capacityLimits must name at least one capacity limit., RequestId: <id>' — the envelope is right and the sentence inside it is the emulator's own prose.
Why: a client that matches Batch's refusal text (or logs/alerts on it) never recognises the missing-required-member refusal, and the sentence AWS uses to tell the caller the member is REQUIRED is replaced by one about capacity-limit cardinality, which points the caller at a different fix.
Fix: the capacityLimits validation message in the CreateQuotaShare handler (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/create_quota_share.rs).
Traces: batch.create-quota-share-missing-required-field-rejected.

=== Cause 2 of 7: RegisterJobDefinition's mainNode-out-of-range refusal message is emulator-authored and echoes the submitted values ===
AWS: Smithy RegisterJobDefinition binds POST /v1/registerjobdefinition and models exactly ClientException (400) / ServerException (500); nodeProperties -> NodeProperties carries mainNode and numNodes as plain required integers with no @range and no cross-member constraint, so the 'mainNode < numNodes' rule and its wording are recorded behavior only. Recorded refusal: HTTP 400, ClientException, Error.Message = 'Error executing request, Exception : Main node index is out of range, must be less than numNodes., RequestId: <this call's request id>'. Fixed text is everything but the request id; AWS does NOT echo the submitted mainNode or numNodes values.
Emulator: Error.Message = 'Error executing request, Exception : nodeProperties.mainNode <mainNode> must be fewer than nodeProperties.numNodes <numNodes>, RequestId: <id>' — a different sentence, prefixed with member paths, missing the terminating period, and interpolating the two submitted integers that AWS keeps out of the message.
Why: the refusal is unrecognisable to any client keyed on AWS's text, and callers that scrape the message learn a different shape of fact (member paths plus values) than AWS publishes, so code written against the emulator breaks on real AWS.
Fix: the multi-node nodeProperties validation message in the RegisterJobDefinition handler (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/register_job_definition.rs).
Traces: batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected.

=== Cause 3 of 7: SubmitJob's DISABLED-queue refusal message does not name the queue ARN in AWS's sentence ===
AWS: Smithy SubmitJob binds POST /v1/submitjob with required jobName/jobQueue/jobDefinition and models exactly ClientException (400) / ServerException (500); the queue-state guard and its wording are recorded behavior only. Recorded refusal when the target job queue's state is DISABLED: HTTP 400, ClientException, Error.Message = 'JobQueue <the resolved job queue ARN> not ENABLED.' — fixed text 'JobQueue ' and ' not ENABLED.', the single variable part being the queue's ARN taken from the emulator's own state for the queue the request names.
Emulator: the refusal happens (right class and status) but the message is "Job queue <job queue ARN> is DISABLED and can't accept new jobs" — an emulator-authored sentence that shares no fixed text with AWS's, states the state name instead of the ENABLED precondition, and carries no terminating period.
Why: a client that distinguishes a drained queue from other ClientException refusals by message text cannot do so; retry/drain tooling written against AWS's 'not ENABLED.' wording silently treats the refusal as unknown.
Fix: the queue-state guard's message in the SubmitJob handler (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/submit_job.rs).
Traces: batch.submit-job-to-disabled-queue-fails.

=== Cause 4 of 7: SubmitServiceJob accepts a submission to a DISABLED job queue ===
AWS: Smithy SubmitServiceJob binds POST /v1/submitservicejob with required jobName, jobQueue, serviceRequestPayload and serviceJobType, success 200 {jobArn, jobName, jobId}, errors exactly ClientException (400) / ServerException (500). The queue-state precondition is recorded behavior only: a submission to a job queue whose state is DISABLED is refused with HTTP 400 / ClientException and no job is created; the check's positive control submits the identical job to an ENABLED queue and is accepted.
Emulator: HTTP 200 with a freshly minted jobArn/jobId/jobName on the DISABLED queue (observed: 'jobArn': 'arn:aws:batch:<region>:<account>:service-job/<uuid>').
Why: DISABLED is the documented way to drain a service job queue. If submissions are still accepted, drain and maintenance procedures silently do nothing and work accumulates on a queue the operator believes is closed; the client's refusal path is never exercised.
Fix: resolve the target queue and refuse with ClientException 400 when its state is not ENABLED, in the SubmitServiceJob handler's validation (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/submit_service_job.rs).
Traces: batch.service-job-submit-to-disabled-queue-fails.

=== Cause 5 of 7: SubmitServiceJob accepts an omitted quotaShareName on a queue bound to a quotaSharePolicy ===
AWS: Smithy SubmitServiceJob.quotaShareName is an optional unconstrained String, so the requirement is conditional and recorded behavior only: when the target job queue's schedulingPolicyArn names a scheduling policy carrying a quotaSharePolicy, a SubmitServiceJob that omits quotaShareName is refused server-side with HTTP 400 / ClientException and no job is created, while the same submission WITH quotaShareName is accepted on that queue (both checks carry that positive control).
Emulator: HTTP 200 with a jobArn/jobId for the submission that omits quotaShareName.
Why: quota accounting is bypassed — a job lands on a quota-managed queue attached to no share, so capacity reservation and preemption cannot be modelled at all, and the caller's required-field error path never fires.
Fix: cross-resource validation in the SubmitServiceJob handler — resolve the queue's scheduling policy and require quotaShareName (naming an existing share on that queue) when that policy is a quotaSharePolicy.
Traces: batch.submit-service-job-missing-quota-share-name-rejected, batch.submit-service-job-missing-required-quota-share-name-rejected.

=== Cause 6 of 7: TerminateServiceJob performs no authorization check, so an unauthorized principal succeeds ===
AWS: Smithy TerminateServiceJob binds POST /v1/terminateservicejob with required jobId and reason and an empty 200 output, modelling only ClientException/ServerException, so the authorization refusal is recorded behavior only: a caller whose credentials carry no permission for batch:TerminateServiceJob is REFUSED rather than served, and the job it names is left untouched; the same call from a permitted principal terminates the job. The check asserts only that the call is rejected, so the exact code/status for this refusal comes from the recording and not from this replay.
Emulator: HTTP 200 with an empty body (content-length 2) for the restricted principal — the termination is executed.
Why: a least-privilege policy cannot be exercised against the emulator at all. Code that relies on Batch refusing a termination from a restricted role appears to work here and then fails (or, worse, ships without the guard) against real AWS.
Fix: the authorization gate in front of the TerminateServiceJob handler (identity/permission evaluation for batch:TerminateServiceJob) — refuse the restricted caller instead of performing the termination.
Traces: batch.terminate-service-job-without-permission-denied.

=== Cause 7 of 7: a submitted service job is never dispatched — it is driven to FAILED with an emulator-authored statusReason, so the snapshot/list projections of a live job are unobservable ===
AWS: Smithy SubmitServiceJob returns 200 {jobArn, jobName, jobId}; DescribeServiceJob returns the required status (ServiceJobStatus: SUBMITTED | PENDING | RUNNABLE | SCHEDULED | STARTING | RUNNING | SUCCEEDED | FAILED) plus optional statusReason/startedAt/stoppedAt/capacityUsage/attempts; GetJobQueueSnapshot returns frontOfQueue; ListServiceJobs filters by jobStatus. Recorded behavior: a service job submitted to a VALID/ENABLED SAGEMAKER_TRAINING queue backed by a VALID service environment progresses through non-terminal statuses to SUCCEEDED; a job asking for more capacity than the environment publishes RESTS in RUNNABLE, is listed in GetJobQueueSnapshot.frontOfQueue.jobs and under ListServiceJobs(jobStatus=RUNNABLE) while it waits, leaves frontOfQueue once terminal, and appears under ListServiceJobs(jobStatus=FAILED) after TerminateServiceJob. A statusReason, when present, is produced by the dispatching service and never states anything about the emulator.
Emulator: SubmitServiceJob is accepted 200, then DescribeServiceJob reports status 'FAILED' with statusReason 'This emulator does not run SageMaker AI, so the service job was never dispatched'. No job ever becomes RUNNING or SUCCEEDED, and a capacity-blocked job never becomes visible at the front of its queue, so the two snapshot/status-filter checks poll GetJobQueueSnapshot and DescribeServiceJob for a progression that can never arrive and run out the harness's 60 s budget ('Check timed out after 60.0s') without reaching a verdict.
Why: the whole service-job plane is unusable. Any client that submits a service job and waits for RUNNABLE/RUNNING/SUCCEEDED sees an immediate, spurious terminal failure; failure-handling code keys off a reason string that exists nowhere in AWS, so even the error path diverges; and queue-snapshot, status-filter, fair-share and quota-share capacity behaviour cannot be observed because no job ever holds capacity or sits in a queue.
Fix: the service-job state machine in the Batch service state (services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs, advance_service_jobs / NO_SERVICE_JOB_RUNTIME_REASON) together with the SubmitServiceJob, DescribeServiceJob, GetJobQueueSnapshot and ListServiceJobs projections — dispatch an accepted job against the service environment's capacity limits, hold a capacity-blocked job in RUNNABLE and surface it at the front of its queue and under the RUNNABLE status filter, and drop the emulator-authored statusReason.
Traces: batch.service-job-lifecycle-state-transitions, batch.service-job-managed-execution-reaches-succeeded, batch.e2e-service-job-lifecycle-through-queue-snapshot, batch.jq-snapshot-and-list-jobs-status-filter.

## Evidence
From the simparity-behavioral-aws-v1 replay against the emulator built from this checkout (150/265 passed; batch 143/232, 89 failed). None of the clusters below is marked ALREADY FILED or OWNED in the report.

Cause 1 — [mismatch] batch.create-quota-share-missing-required-field-rejected:
  AssertionError: expected message 'Error executing request, Exception : capacityLimits is required and cannot be empty., RequestId: <id>', got 'Error executing request, Exception : capacityLimits must name at least one capacity limit., RequestId: <id>' (same request id in both; only the sentence differs).

Cause 2 — [mismatch] batch.register-multi-node-job-definition-main-node-at-or-above-num-nodes-rejected:
  AssertionError: expected 'Error executing request, Exception : Main node index is out of range, must be less than numNodes., RequestId: <id>', got 'Error executing request, Exception : nodeProperties.mainNode 2 must be fewer than nodeProperties.numNodes 2, RequestId: <id>' (the 2/2 are this run's submitted values, which AWS's message does not carry).

Cause 3 — [mismatch] batch.submit-job-to-disabled-queue-fails:
  AssertionError: expected the DISABLED-queue refusal message to name the queue ARN as 'JobQueue <jq_arn> not ENABLED.', got "Job queue arn:aws:batch:<region>:<account>:job-queue/<queue name> is DISABLED and can't accept new jobs".

Cause 4 — [mismatch] batch.service-job-submit-to-disabled-queue-fails:
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobArn': 'arn:aws:batch:<region>:<account>:service-job/<uuid>', 'jobName': 'sjob-rejected-<id>', 'jobId': '<uuid>'} — raised at _client_error() after the queue had been driven to DISABLED.

Cause 5 — [mismatch] batch.submit-service-job-missing-quota-share-name-rejected and [mismatch] batch.submit-service-job-missing-required-quota-share-name-rejected:
  both 'expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobArn': 'arn:aws:batch:<region>:<account>:service-job/<uuid>', 'jobName': '<name>', 'jobId': '<uuid>'}' for a SubmitServiceJob with no quotaShareName onto a queue whose scheduling policy carries a quotaSharePolicy.

Cause 6 — [mismatch] batch.terminate-service-job-without-permission-denied:
  AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-length': '2', ...}}} for terminate_service_job(jobId=<job id>, reason=...) issued with the unprivileged principal's credentials.

Cause 7 — [mismatch] batch.service-job-lifecycle-state-transitions:
  AssertionError: service job ended FAILED ...: 'This emulator does not run SageMaker AI, so the service job was never dispatched'.
  [mismatch] batch.service-job-managed-execution-reaches-succeeded:
  AssertionError: service job did not reach SUCCEEDED purely via server orchestration: status='FAILED' reason='This emulator does not run SageMaker AI, so the service job was never dispatched'.
  [mismatch] batch.e2e-service-job-lifecycle-through-queue-snapshot and [mismatch] batch.jq-snapshot-and-list-jobs-status-filter:
  'Check timed out after 60.0s'. Both submit a SAGEMAKER_TRAINING service job asking for InstanceCount=2 against a service environment whose capacityLimits publish maxCapacity=1 (so AWS holds it at RUNNABLE), then poll describe_service_job for RUNNABLE and get_job_queue_snapshot for the job's jobArn in frontOfQueue.jobs; neither condition is ever reachable because the job is driven to FAILED and never enters the queue's front-of-queue projection, so the whole check exhausts the harness budget without reaching a verdict.

Model facts used: smithy_query(model_path='services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json', query='operation', operation='CreateQuotaShare,RegisterJobDefinition,SubmitJob,SubmitServiceJob,TerminateServiceJob,DescribeServiceJob,GetJobQueueSnapshot,ListServiceJobs'). Every one binds POST /v1/<op> with success 200 and declares exactly ClientException (httpError 400) and ServerException (500). CreateQuotaShare requires quotaShareName/jobQueue/capacityLimits/resourceSharingConfiguration/preemptionConfiguration, with no @length on capacityLimits. RegisterJobDefinition's nodeProperties carries no range or cross-member constraint. SubmitJob requires jobName/jobQueue/jobDefinition with no modelled queue-state rule. SubmitServiceJob requires jobName/jobQueue/serviceRequestPayload/serviceJobType and leaves quotaShareName an optional unconstrained String. TerminateServiceJob requires jobId+reason and returns an empty output. DescribeServiceJob's status is the required ServiceJobStatus enum with the eight values listed above; GetJobQueueSnapshot returns frontOfQueue; ListServiceJobs takes an optional jobStatus filter. Every value-level rule and every message above is therefore labelled recorded behavior only.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite and read the `### batch` section of the report.
2. Cause 1: create a service environment and a SAGEMAKER_TRAINING job queue on it, then `aws batch create-quota-share --quota-share-name q --job-queue <jq arn> --capacity-limits '[]' --resource-sharing-configuration ... --preemption-configuration ...` -> 400 ClientException whose message says 'capacityLimits must name at least one capacity limit.' instead of 'capacityLimits is required and cannot be empty.' inside the same 'Error executing request, Exception : ..., RequestId: ...' envelope.
3. Cause 2: `aws batch register-job-definition --type multinode --node-properties numNodes=2,mainNode=2,nodeRangeProperties=[...]` -> 400 ClientException whose message is 'nodeProperties.mainNode 2 must be fewer than nodeProperties.numNodes 2' instead of 'Main node index is out of range, must be less than numNodes.'
4. Cause 3: create a compute environment + job queue, `aws batch update-job-queue --job-queue <jq> --state DISABLED`, wait for VALID, then `aws batch submit-job --job-queue <jq> --job-definition <jd> --job-name j` -> 400 ClientException "Job queue <arn> is DISABLED and can't accept new jobs" instead of 'JobQueue <arn> not ENABLED.'
5. Cause 4: create a SAGEMAKER_TRAINING service environment and service job queue, `aws batch update-job-queue --job-queue <jq> --state DISABLED`, then `aws batch submit-service-job --job-queue <jq> --service-job-type SAGEMAKER_TRAINING --service-request-payload <payload> --job-name j` -> HTTP 200 with a jobId instead of 400 ClientException; the same submit onto an ENABLED queue must still succeed.
6. Cause 5: `aws batch create-scheduling-policy --quota-share-policy idleResourceAssignmentStrategy=FIFO`, attach it to a service job queue via --scheduling-policy-arn, create a quota share on that queue, then `aws batch submit-service-job` WITHOUT --quota-share-name -> HTTP 200 instead of 400 ClientException (the same call WITH --quota-share-name must still succeed).
7. Cause 6: submit a service job, then call `aws batch terminate-service-job --job-id <id> --reason r` with credentials holding no batch:TerminateServiceJob permission -> HTTP 200 with an empty body instead of an authorization refusal, and the job is terminated.
8. Cause 7: create a SAGEMAKER_TRAINING service environment with capacityLimits maxCapacity=1 (wait VALID) and an ENABLED SAGEMAKER_TRAINING job queue; `aws batch submit-service-job` with a payload whose ResourceConfig.InstanceCount=2; then poll `aws batch describe-service-job --job-id <id>` -> status 'FAILED' with statusReason 'This emulator does not run SageMaker AI, so the service job was never dispatched' instead of resting in RUNNABLE; `aws batch get-job-queue-snapshot --job-queue <jq>` never lists the job's jobArn under frontOfQueue.jobs, and `aws batch list-service-jobs --job-queue <jq> --job-status RUNNABLE` never lists it.
