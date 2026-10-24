Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.delete-quota-share-referenced-by-active-scheduling-policy-fails batch.create-quota-share-negative-weight-value-rejected batch.jq-service-queue-rejects-compute-environment-order batch.service-job-submit-to-disabled-queue-fails batch.submit-service-job-missing-quota-share-name-rejected batch.submit-service-job-missing-required-quota-share-name-rejected batch.terminate-service-job-without-permission-denied batch.job-queue-tag-unicode-value-round-trip batch.service-job-managed-execution-reaches-succeeded batch.e2e-fair-share-quota-lifecycle-affects-dispatch-and-teardown batch.deploy-compute-environment-cloudformation-behaves
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.delete-quota-share-referenced-by-active-scheduling-policy-fails|run#9|AssertionError|assert code == "ClientException", f"expected ClientException, got {code}"
Issue-Key: batch.create-quota-share-negative-weight-value-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: batch.jq-service-queue-rejects-compute-environment-order|_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: batch.service-job-submit-to-disabled-queue-fails|_client_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: batch.submit-service-job-missing-quota-share-name-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: batch.submit-service-job-missing-required-quota-share-name-rejected|_error#21|AssertionError|raise AssertionError(
Issue-Key: batch.terminate-service-job-without-permission-denied|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: batch.job-queue-tag-unicode-value-round-trip|_error#21|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: batch.service-job-managed-execution-reaches-succeeded|run#28|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: batch.e2e-fair-share-quota-lifecycle-affects-dispatch-and-teardown|run#36|AssertionError|assert high_final["status"] == "RUNNING", (
Issue-Key: batch|CreateStack|ValidationError
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# batch: overly-permissive control and service-job planes — DeleteQuotaShare is a 501 stub, five refusals AWS makes are never made, service jobs never dispatch, and CFN rejects two Batch resource types (9 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitServiceJob

## Description
NINE distinct root causes in the AWS Batch emulator, all grounded in failing checks of the simparity-behavioral-ga-prod-v1 replay that the report does NOT mark ALREADY FILED or OWNED. Each cause is numbered; a fixer must close all nine before submitting.

Related open issues that do NOT carry these replay keys and do NOT cover these causes: '20261022T221856Z-batch-submitted-jobs-are-driven-straight-to-failed' (container job plane / RegisterJobDefinition validation; its cause 6 concerns a CFN-created AWS::Batch::JobQueue being invisible, which is a different failure from cause 9 below — the stack now never gets created at all) and '20261023T081835Z-batch-missing-and-spurious-refusals-plus-stale-describe' (wire corpus, control-plane describe/delete state).

=== Cause 1 of 9: DeleteQuotaShare answers NotImplemented instead of its modeled behavior ===
AWS: Smithy com.amazonaws.batch#DeleteQuotaShare binds POST /v1/deletequotashare, takes required quotaShareArn, returns an empty 200 and models exactly ClientException (httpError 400) and ServerException (500) — NotImplemented is not a response this operation can produce. Recorded behavior for deleting a quota share that is still ENABLED: HTTP 400, error code ClientException, message (example) 'Quota Share must be DISABLED before deleting.'
Emulator: the call fails with error code 'NotImplemented' ('operation ... is scaffolded but not implemented'), so neither the refusal path nor the DISABLED-then-delete success path exists.
Why: a client cannot delete a quota share at all, and the guard that protects an in-use quota share is unobservable; teardown of any quota-share-based fair-share setup is impossible and the modeled ClientException branch is dead code.
Fix: implement the DeleteQuotaShare handler (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/delete_quota_share.rs) plus its state removal: refuse with ClientException 400 while the quota share is ENABLED, and delete it once DISABLED.
Traces: batch.delete-quota-share-referenced-by-active-scheduling-policy-fails ('expected ClientException, got NotImplemented').

=== Cause 2 of 9: CreateQuotaShare accepts a negative capacityLimits.maxCapacity ===
AWS: Smithy CreateQuotaShare (POST /v1/createquotashare) requires quotaShareName, jobQueue, capacityLimits (QuotaShareCapacityLimits), resourceSharingConfiguration and preemptionConfiguration, and models ClientException (400). The model places no @range on maxCapacity, so the bound is recorded behavior only: a capacityLimits entry with a negative maxCapacity is refused with HTTP 400 / ClientException and no quota share is created.
Emulator: HTTP 200 — the quota share is created and an ARN returned for capacityLimits [{maxCapacity: -5, capacityUnit: 'ml.m6i.large'}].
Why: a nonsensical capacity becomes live state that the scheduler must then interpret; the client's validation error path is never exercised and the misconfiguration surfaces far from its cause (or silently starves the share).
Fix: input validation in the CreateQuotaShare handler — range-check every capacityLimits[].maxCapacity and refuse with ClientException 400.
Traces: batch.create-quota-share-negative-weight-value-rejected.

=== Cause 3 of 9: CreateJobQueue accepts computeEnvironmentOrder on a SERVICE-type job queue ===
AWS: Smithy CreateJobQueue models jobQueueType (JobQueueType enum), computeEnvironmentOrder (ComputeEnvironmentOrders) and serviceEnvironmentOrder (ServiceEnvironmentOrders) as independent optional members with errors ClientException (400); the mutual exclusion is recorded behavior only — a service-type job queue must carry serviceEnvironmentOrder and supplying computeEnvironmentOrder instead/as well is refused with HTTP 400 / ClientException, and no queue is created.
Emulator: HTTP 200 with jobQueueName/jobQueueArn — the illegal queue is created.
Why: the two queue flavours stop being distinguishable; a client can build a service queue pointing at compute environments that can never place its jobs, and discovers it only when submissions never run.
Fix: cross-member validation in the CreateJobQueue handler (jobQueueType vs computeEnvironmentOrder/serviceEnvironmentOrder) raising ClientException 400.
Traces: batch.jq-service-queue-rejects-compute-environment-order.

=== Cause 4 of 9: SubmitServiceJob accepts a submission to a DISABLED job queue ===
AWS: Smithy SubmitServiceJob binds POST /v1/submitservicejob with required jobName, jobQueue, serviceRequestPayload and serviceJobType, and models ClientException (400). Recorded behavior: submitting to a job queue whose state is DISABLED is refused with HTTP 400 / ClientException and no job is created (the check's positive control submits the same job to an ENABLED queue and is accepted).
Emulator: HTTP 200 with a jobArn/jobId minted on the DISABLED queue.
Why: DISABLED is the documented way to drain a queue. If submissions are still accepted, drain and maintenance procedures silently do nothing, and jobs accumulate on a queue the operator believes is closed.
Fix: the SubmitServiceJob handler must resolve the target queue and refuse with ClientException 400 when its state is not ENABLED.
Traces: batch.service-job-submit-to-disabled-queue-fails.

=== Cause 5 of 9: SubmitServiceJob accepts an omitted quotaShareName on a queue bound to a quotaSharePolicy ===
AWS: Smithy SubmitServiceJob.quotaShareName is an unconstrained optional String, so the requirement is conditional and recorded behavior only: when the target job queue's schedulingPolicyArn names a policy with a quotaSharePolicy, a SubmitServiceJob that omits quotaShareName is refused server-side with HTTP 400 / ClientException and no job is created; the same submission WITH quotaShareName is accepted on that queue (both checks carry that positive control).
Emulator: HTTP 200 with a jobArn/jobId for the submission that omits quotaShareName.
Why: quota accounting is bypassed — a job lands on a quota-managed queue attached to no share, so capacity reservation and preemption cannot be modelled, and a client's required-field error path never fires.
Fix: the SubmitServiceJob handler's cross-resource validation — look up the queue's scheduling policy and require quotaShareName (and that it names an existing share on that queue) when the policy is a quotaSharePolicy.
Traces: batch.submit-service-job-missing-quota-share-name-rejected, batch.submit-service-job-missing-required-quota-share-name-rejected.

=== Cause 6 of 9: TerminateServiceJob performs no authorization check — an unauthorized principal succeeds ===
AWS: Smithy TerminateServiceJob binds POST /v1/terminateservicejob with required jobId and reason and an empty 200 output. The authorization refusal is recorded behavior only (the model declares only ClientException/ServerException): a caller whose credentials are not permitted to terminate the job is REFUSED, not served; the check asserts only that the call is rejected and never reached its code assertion, so the exact code/status comes from the recording rather than from this replay.
Emulator: HTTP 200 with an empty body — the restricted principal's TerminateServiceJob succeeds and the job is terminated.
Why: a least-privilege policy cannot be tested against the emulator at all. Code that relies on Batch refusing a termination from a restricted role appears to work and then fails (or, worse, is shipped without the guard) in production.
Fix: the authorization gate in front of the TerminateServiceJob handler (identity/permission evaluation for batch:TerminateServiceJob) — refuse the restricted caller instead of executing the termination.
Traces: batch.terminate-service-job-without-permission-denied.

=== Cause 7 of 9: TagResource accepts a tag value outside AWS's tag character class ===
AWS: Smithy TagResource binds POST /v1/tags/{resourceArn} with required tags (TagrisTagsMap, @length 1..50) and models ClientException (400). The value charset is recorded behavior only: the boundary is the character class, not the byte width — a value made of unicode letters from any script plus the standard allowed punctuation round-trips byte-identically, while a value containing a character outside that class (an emoji) is REFUSED with HTTP 400 / ClientException.
Emulator: HTTP 200 with an empty body — the emoji tag value is accepted on a job queue ARN.
Why: the emulator admits tag values AWS will not store, so tagging code validated against it fails on real AWS; conversely nothing proves the emulator keeps the legal unicode-letter value byte-identical.
Fix: tag-value charset validation in the shared Batch tag-write path (TagResource, and the tags member of the create operations) — refuse out-of-class characters with ClientException 400 while leaving unicode letters intact.
Traces: batch.job-queue-tag-unicode-value-round-trip.

=== Cause 8 of 9: a submitted service job is never dispatched — it is driven to FAILED with an emulator-specific reason ===
AWS: Smithy SubmitServiceJob returns 200 {jobArn, jobName, jobId} and ServiceJobType covers the SAGEMAKER_TRAINING flavour; recorded behavior is that a service job submitted to an ENABLED service-type queue backed by a VALID service environment is dispatched by the service and progresses to a terminal SUCCEEDED (and, while it holds its capacity unit, rests in RUNNING).
Emulator: DescribeServiceJob reports status 'FAILED' with statusReason 'This emulator does not run SageMaker AI, so the service job was never dispatched'. No job ever becomes RUNNING or SUCCEEDED, so neither managed execution nor fair-share dispatch across quota shares can be observed.
Why: the whole service-job plane is unusable. Any client that submits a service job and waits for RUNNING/SUCCEEDED sees an immediate spurious terminal failure, and the statusReason is emulator text that exists nowhere in AWS, so even failure handling diverges. Fair-share/quota behaviour (which job holds the single capacity unit) is unmeasurable.
Fix: the service-job state machine in the Batch service state plus the SubmitServiceJob/DescribeServiceJob handlers — drive a submitted service job SUBMITTED -> RUNNABLE -> RUNNING -> SUCCEEDED against the service environment's capacity limits instead of failing it at submit time with a 'not dispatched' reason.
Traces: batch.service-job-managed-execution-reaches-succeeded ('service job did not reach SUCCEEDED purely via server orchestration'), batch.e2e-fair-share-quota-lifecycle-affects-dispatch-and-teardown ("teamHigh's job must be dispatched and hold the unit (RUNNING ...): FAILED").

=== Cause 9 of 9: CloudFormation does not recognize AWS::Batch::ServiceEnvironment or AWS::Batch::QuotaShare ===
AWS: both resource types are supported CloudFormation resources, so a template containing AWS::Batch::ServiceEnvironment and AWS::Batch::QuotaShare is accepted by CreateStack and provisions real Batch resources (recorded behavior; the Batch Smithy model carries the corresponding CreateServiceEnvironment and CreateQuotaShare operations).
Emulator: CreateStack is refused outright with HTTP 400, error code 'ValidationError', message 'Template format error: Unrecognized resource types: [AWS::Batch::ServiceEnvironment, AWS::Batch::QuotaShare]', so the stack is never created and no Batch resource is provisioned.
Why: Batch resources deployed by infrastructure-as-code cannot be emulated — the stack fails at submit time, so no CFN-driven Batch workflow (the normal way these resources are created) can run end to end. This is a different failure from cause 6 of '20261022T221856Z-batch-submitted-jobs-are-driven-straight-to-failed', where the stack was created and only the job queue was invisible to DescribeJobQueues.
Fix: register the AWS::Batch::ServiceEnvironment and AWS::Batch::QuotaShare resource providers in the CloudFormation resource-type registry and wire them to the Batch CreateServiceEnvironment/CreateQuotaShare state so the created resources are resolvable by the Batch API.
Traces: batch.deploy-compute-environment-cloudformation-behaves.

## Evidence
From the simparity-behavioral-ga-prod-v1 replay against the emulator built from this checkout (36/117 passed; batch 35/107, 72 failed). None of the clusters below is marked ALREADY FILED or OWNED in the report.

Cause 1 — [mismatch] batch.delete-quota-share-referenced-by-active-scheduling-policy-fails: AssertionError: expected ClientException, got NotImplemented (at `assert code == "ClientException"` immediately after `batch.delete_quota_share(quotaShareArn=qs_arn)`; the preceding DescribeQuotaShare asserting state=='ENABLED' passed, so the quota share existed).
Cause 2 — [mismatch] batch.create-quota-share-negative-weight-value-rejected: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'quotaShareName': 'sim2real-batch-qs-...', 'quotaShareArn': 'arn:aws:batch:us-west-2:<acct>:job-queue/sim2real-batch-jq-.../quota-share/sim2real-batch-qs-...'} for capacityLimits=[{'maxCapacity': -5, 'capacityUnit': 'ml.m6i.large'}].
Cause 3 — [mismatch] batch.jq-service-queue-rejects-compute-environment-order: AssertionError: expected a rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobQueueName': 'sim2real-behav-sjq-...', 'jobQueueArn': 'arn:aws:batch:...:job-queue/sim2real-behav-sjq-...'}.
Cause 4 — [mismatch] batch.service-job-submit-to-disabled-queue-fails: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobArn': 'arn:aws:batch:...:service-job/<uuid>', 'jobName': 'sjob-rejected-...', 'jobId': '<uuid>'}.
Cause 5 — [mismatch] batch.submit-service-job-missing-quota-share-name-rejected and [mismatch] batch.submit-service-job-missing-required-quota-share-name-rejected: both 'expected the call to be rejected, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200 ..., 'jobArn': 'arn:aws:batch:...:service-job/<uuid>', 'jobId': '<uuid>'}'; in the second the queue's policy was created with quotaSharePolicy={'idleResourceAssignmentStrategy': 'FIFO'} and the service environment reached VALID before the submit.
Cause 6 — [mismatch] batch.terminate-service-job-without-permission-denied: AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-length': '2', ...}}} for restricted_batch.terminate_service_job(jobId=<job_id>, reason=...).
Cause 7 — [mismatch] batch.job-queue-tag-unicode-value-round-trip: AssertionError: expected a rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-length': '2', ...}}} for batch.tag_resource(resourceArn=<job queue arn>, tags={'Environment': <emoji value>}).
Cause 8 — [mismatch] batch.service-job-managed-execution-reaches-succeeded: AssertionError: service job did not reach SUCCEEDED purely via server orchestration: status='FAILED' reason='This emulator does not run SageMaker AI, so the service job was never dispatched'. [mismatch] batch.e2e-fair-share-quota-lifecycle-affects-dispatch-and-teardown: AssertionError: teamHigh's job must be dispatched and hold the unit (RUNNING, inside its 600s sleep): FAILED / This emulator does not run SageMaker AI, so the service job was never dispatched.
Cause 9 — [error] batch.deploy-compute-environment-cloudformation-behaves: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Batch::ServiceEnvironment, AWS::Batch::QuotaShare].

Model facts used: smithy_query(model_path='services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json', query='operation', operation='DeleteQuotaShare,CreateQuotaShare,CreateJobQueue,SubmitServiceJob,TerminateServiceJob,TagResource,CreateServiceEnvironment') — every one binds POST /v1/<op> with success 200 and declares exactly ClientException (httpError 400) and ServerException (500); DeleteQuotaShare requires quotaShareArn and returns an empty output; CreateQuotaShare requires quotaShareName/jobQueue/capacityLimits/resourceSharingConfiguration/preemptionConfiguration with no @range on capacity; CreateJobQueue carries jobQueueType plus both computeEnvironmentOrder and serviceEnvironmentOrder as optional members; SubmitServiceJob.quotaShareName is an optional unconstrained String; TerminateServiceJob requires jobId+reason with an empty output; TagResource.tags is TagrisTagsMap @length 1..50 with no value pattern. Every value-level rule above is therefore labelled recorded behavior only.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite and read the batch section.
2. Cause 1: create a service environment, a scheduling policy, a job queue on them and a quota share (ENABLED); aws batch delete-quota-share --quota-share-arn <arn> -> error code NotImplemented instead of 400 ClientException 'Quota Share must be DISABLED before deleting.'
3. Cause 2: aws batch create-quota-share --quota-share-name q --job-queue <jq arn> --capacity-limits maxCapacity=-5,capacityUnit=ml.m6i.large --resource-sharing-configuration strategy=RESERVE --preemption-configuration inSharePreemption=ENABLED -> HTTP 200 instead of 400 ClientException.
4. Cause 3: aws batch create-job-queue with a SERVICE-flavoured queue (serviceEnvironmentOrder) that ALSO passes --compute-environment-order -> HTTP 200 instead of 400 ClientException.
5. Cause 4: create a service-type job queue, aws batch update-job-queue --state DISABLED, then aws batch submit-service-job onto it -> HTTP 200 with a jobId instead of 400 ClientException.
6. Cause 5: aws batch create-scheduling-policy --quota-share-policy idleResourceAssignmentStrategy=FIFO; attach it to a service job queue; aws batch submit-service-job WITHOUT --quota-share-name -> HTTP 200 instead of 400 ClientException (the same call with --quota-share-name must still succeed).
7. Cause 6: submit a service job, then call TerminateServiceJob with credentials that lack batch:TerminateServiceJob -> HTTP 200 instead of an authorization refusal.
8. Cause 7: aws batch tag-resource --resource-arn <job queue arn> --tags Environment=<a value containing an emoji> -> HTTP 200 instead of 400 ClientException (a value of unicode letters must still be accepted and round-trip byte-identically).
9. Cause 8: create a SAGEMAKER_TRAINING service environment (wait VALID) and a service job queue, aws batch submit-service-job, then poll aws batch describe-service-job --job-id <id> -> status 'FAILED' with statusReason 'This emulator does not run SageMaker AI, so the service job was never dispatched', instead of progressing to RUNNING and SUCCEEDED.
10. Cause 9: aws cloudformation create-stack with a template containing AWS::Batch::ServiceEnvironment and AWS::Batch::QuotaShare -> 400 ValidationError 'Unrecognized resource types' instead of a stack that reaches CREATE_COMPLETE.
