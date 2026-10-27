Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.deploy-compute-environment-cloudformation-behaves
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.deploy-compute-environment-cloudformation-behaves|run#24|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# batch: a SAGEMAKER_TRAINING service job is never dispatched — DescribeServiceJob reports FAILED with an emulator-authored statusReason instead of running to SUCCEEDED

> _Note: this cause was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this was cause 1 of 2 of `20261026T235831Z-batch-a-cfn-deployed-sagemaker-training-service-job.md`. Cause 2 of that issue (ListJobsByConsumableResource keeping a job after it goes terminal) is fixed and that report now sits under `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so it is not recorded as working. Its `Issue-Key` moved with it. It is the same defect family as cause 8 of `20261024T120647Z-batch-overly-permissive-control-and-service-job-planes.md`, which is still open._

## Operation
SubmitServiceJob, DescribeServiceJob

## Description
AWS: Smithy `com.amazonaws.batch#SubmitServiceJob` binds POST /v1/submitservicejob (success 200, errors exactly `ClientException` 400 and `ServerException` 500) and returns {jobArn, jobName, jobId}; `serviceJobType` is the `ServiceJobType` enum covering the `SAGEMAKER_TRAINING` flavour, and `quotaShareName`/`schedulingPriority` are optional. `DescribeServiceJob` (POST /v1/describeservicejob) returns the required `status` (`ServiceJobStatus`: SUBMITTED | PENDING | RUNNABLE | SCHEDULED | STARTING | RUNNING | SUCCEEDED | FAILED) plus optional `statusReason`, `startedAt`, `stoppedAt`, `capacityUsage` and `attempts`. Recorded behavior: a service job submitted with a valid `serviceRequestPayload` to an ENABLED `SAGEMAKER_TRAINING` job queue backed by a VALID service environment and an existing quota share is dispatched by the service, progresses through non-terminal statuses and reaches terminal `SUCCEEDED`; the `statusReason` of a job that ends is produced by the dispatching service, never a statement about the emulator.

Emulator: `DescribeServiceJob` reports `FAILED` with `statusReason` 'This emulator does not run SageMaker AI, so the service job was never dispatched'. `services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs::advance_service_jobs` walks an accepted service job SUBMITTED -> RUNNABLE and then, one span later, into `SERVICE_JOB_FAILED` with `NO_SERVICE_JOB_RUNTIME_REASON`; a job whose requested capacity the queue's service environments publish no room for instead rests in RUNNABLE. So a placeable job is never RUNNING and never SUCCEEDED, and the fixed text of that reason exists nowhere in AWS.

Why: a client can stand up AWS::Batch::SchedulingPolicy + AWS::Batch::ServiceEnvironment + AWS::Batch::JobQueue + AWS::Batch::QuotaShare, see CREATE_COMPLETE and a VALID/ENABLED queue, and then every job submitted to that queue fails instantly. Code that waits for RUNNING/SUCCEEDED sees a spurious terminal failure, and failure-handling code keys off a reason string AWS never emits, so even the error path diverges. Quota-share capacity accounting and preemption stay unmeasurable because no job ever holds capacity.

The ONE blocker, for whoever picks this up: the terminal status AWS reports here is the outcome of a SageMaker TRAINING RUN — executing the container image the payload's `AlgorithmSpecification` names — and this platform has no container runtime. `vendor/engine/src/lib.rs` states it ("Containers and external processes are not available to a generated emulator yet"), and the service that would run the work says the same for the same reason: `services/service-com-amazonaws-sagemaker-sagemaker/src/state.rs`, `NO_TRAINING_CONTAINER_RUNTIME_REASON` — a SageMaker training job in this emulator ends in the modelled `Failed` state because no container runtime started its image and no model artifacts were written. Writing `SUCCEEDED`, a `startedAt`, an attempt or a `capacityUsage` for that run would be a status for work nothing performed, and re-labelling the reason would only re-word the same refusal, so neither is a fix. What is left to do in THIS crate, once a training run can actually happen (or once Batch can dispatch to a service whose training jobs complete): dispatch the accepted payload to the service that runs it, record the dispatched run's identifier as `latestAttempt.serviceResourceId`, hold the named quota share's and the service environment's capacity while the run is live (so `capacityUsage` and preemption become observable), and mirror the dispatched run's outcome and ITS reason into the service job's terminal `ServiceJobStatus`/`statusReason`. Service jobs submitted to a CloudFormation-provisioned queue must take the same path as ones submitted through the API — the CFN path itself already works (the stack reaches CREATE_COMPLETE and the queue comes back VALID/ENABLED).

Traces: batch.deploy-compute-environment-cloudformation-behaves

## Evidence
From the simparity-behavioral-ga-prod-v1 replay against the emulator built from this checkout:

```
  File "check_batch_deploy_compute_environment_cloudformation_behaves.py", line 91, in run
    assert final["status"] == "SUCCEEDED", (
  AssertionError: expected the CFN-deployed queue to run the job to SUCCEEDED, got 'FAILED' ('This emulator does not run SageMaker AI, so the service job was never dispatched')
```

The check first drove CreateStack over a template containing AWS::Batch::SchedulingPolicy, AWS::Batch::ServiceEnvironment, AWS::Batch::JobQueue and AWS::Batch::QuotaShare to CREATE_COMPLETE, then asserted describe_job_queues status=='VALID' and state=='ENABLED' (both passed), then submit_service_job(serviceJobType='SAGEMAKER_TRAINING', quotaShareName=<qs>, schedulingPriority=5) was ACCEPTED with a jobId, and describe_service_job polling (600 s budget, 5 s interval) saw the terminal status on the first probe.

Expected vs actual: status SUCCEEDED (after non-terminal SUBMITTED/RUNNABLE/RUNNING) vs status 'FAILED' with the emulator-authored `statusReason` above, which carries no request- or state-derived value.

## Steps to Reproduce
1. `aws cloudformation create-stack` with a template containing AWS::Batch::SchedulingPolicy (with a quotaSharePolicy), AWS::Batch::ServiceEnvironment (SAGEMAKER_TRAINING), AWS::Batch::JobQueue bound to that service environment and AWS::Batch::QuotaShare on that queue; wait for CREATE_COMPLETE (it is reached).
2. `aws batch describe-job-queues --job-queues <jq>` reports status VALID / state ENABLED.
3. `aws batch submit-service-job --job-name j --job-queue <jq> --service-job-type SAGEMAKER_TRAINING --service-request-payload <a TrainingJob payload> --quota-share-name <qs> --scheduling-priority 5` — accepted, 200 with a jobId.
4. Poll `aws batch describe-service-job --job-id <id>` -> status 'FAILED' with statusReason 'This emulator does not run SageMaker AI, so the service job was never dispatched', instead of progressing through RUNNING to SUCCEEDED.
