Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.job-claim-nonce-single-use-after-acknowledgement-rejected codepipeline.put-job-success-invalid-job-state-rejected codepipeline.put-job-success-result-wrong-job-state-rejected codepipeline.retry-stage-execution-stale-token-rejected codepipeline.stopping-an-execution-leaves-no-deploy-artifact-and-a-restop-is-rejected codepipeline.e2e-pipeline-create-run-approve-complete-teardown codepipeline.job-output-variables-precision-preserved codepipeline.webhook-url-rejects-a-wrong-hmac-signature-and-starts-no-execution
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|AcknowledgeJob|InvalidNonceException
Issue-Key: codepipeline.put-job-success-invalid-job-state-rejected|run#14|AssertionError|assert (code, message) == (
Issue-Key: codepipeline.put-job-success-result-wrong-job-state-rejected|run#12|AssertionError|assert error.get("Code") == "InvalidJobStateException", f"expected InvalidJobStateException, got {error!r}"
Issue-Key: codepipeline.retry-stage-execution-stale-token-rejected|_upload_output_artifact#9|KeyError|creds = job["data"]["artifactCredentials"]
Issue-Key: codepipeline.stopping-an-execution-leaves-no-deploy-artifact-and-a-restop-is-rejected|run#33|AssertionError|assert restop_message == expected, f"expected message {expected!r}, got {restop_message!r}"
Issue-Key: codepipeline.e2e-pipeline-create-run-approve-complete-teardown|run#27|AssertionError|assert mutate_code == (
Issue-Key: codepipeline|RetryStageExecution|NotLatestPipelineExecutionException
Issue-Key: codepipeline.webhook-url-rejects-a-wrong-hmac-signature-and-starts-no-execution|run#26|AssertionError|assert 200 <= status < 300, (
Issue-Commit: aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6

# CodePipeline job-worker lifecycle and refusal wording: 8 net-new root causes (AcknowledgeJob nonce window, PutJobSuccessResult states, JobData.artifactCredentials, Stop/Start/Retry refusals, unserved webhook URL)

> _Note: this issue was identified against the code base at commit `aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcknowledgeJob

## Description
EIGHT net-new root causes in the CodePipeline job-worker surface and in the refusal/representation surface around it. A fixer must close ALL EIGHT before submitting; each cause names its own failing check ids below.

None of these checks is marked ALREADY FILED or OWNED in this replay. Related but NOT covering: the open issue 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md lists some of these check ids, but it carries their OLD replay keys (a poll timeout); the engine now advances far enough that these checks reach the calls below and fail on concrete, different answers, so these causes are newly visible and are not the stall that issue describes.

=== Cause 1 of 8: AcknowledgeJob refuses a nonce the service itself issued, and never answers the post-completion ValidationException ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models AcknowledgeJob(jobId, nonce) -> {status: JobStatus} with errors InvalidNonceException, JobNotFoundException, ValidationException (all 400). Recorded behavior: the nonce PollForJobs hands back with a job is valid for the whole claim window — the first AcknowledgeJob answers status 'InProgress', and a REPEAT AcknowledgeJob with the SAME nonce while the job is still InProgress is idempotent and also answers 'InProgress'. The nonce stops being valid only once the job has left the window (i.e. after PutJobSuccessResult/PutJobFailureResult), and that call is refused with 400 ValidationException whose message is the fixed text "Job with id <jobId> has already been acknowledged" (the jobId is the value from the request). InvalidNonceException is reserved for a nonce that never matched the job at all.
Emulator: the acknowledgement of a nonce taken straight from the PollForJobs job is refused with 400 InvalidNonceException, message "Invalid nonce for job id <jobId>" — raised uncaught, so it hits either the first acknowledgement of the fresh claim or the idempotent repeat while the job is InProgress. Either way the emulator treats a still-valid nonce as unmatched, and the post-completion refusal class (ValidationException "... has already been acknowledged") is therefore unreachable.
Why: a job worker written against AWS cannot claim work. The documented retry pattern (re-acknowledge after a dropped response) becomes a hard error, and a client that distinguishes InvalidNonceException (bad nonce, drop the job) from ValidationException (already acknowledged, keep going) takes the wrong branch and abandons live jobs.
Fix: the AcknowledgeJob handler and the job-claim state it reads in the codepipeline crate — the nonce must be stored with the claim and accepted for as long as the job is awaiting acknowledgement or completion, and the handler must classify a post-completion acknowledgement as ValidationException with AWS's wording.
Traces: codepipeline.job-claim-nonce-single-use-after-acknowledgement-rejected.

=== Cause 2 of 8: PutJobSuccessResult's unacknowledged-job refusal message is emulator-authored ===
AWS: Smithy models PutJobSuccessResult with error InvalidJobStateException (400). For a job that PollForJobs dispatched but no worker has acknowledged, the refusal message is the fixed text "Job with id <jobId> has not been acknowledged" — the only variable part is the jobId from the request.
Emulator: the error CODE is right (InvalidJobStateException) but the message is rewritten: "Job with id <jobId> is in status Queued, so no result can be reported for it; only a job a worker has acknowledged and not yet completed accepts one". That text, including the internal status name 'Queued' it exposes, is not AWS's.
Why: workers and CI tooling that match on the refusal text (or surface it to operators) see a message no AWS runbook or log search recognises, and the emulator leaks an internal job-status vocabulary AWS does not publish for this error.
Fix: the PutJobSuccessResult handler's InvalidJobStateException construction in the codepipeline crate.
Traces: codepipeline.put-job-success-invalid-job-state-rejected.

=== Cause 3 of 8: a TERMINATED job is forgotten, so a second result on it is reported as not-found instead of a wrong-state refusal ===
AWS: a job that has already reached a terminal state through PutJobFailureResult/PutJobSuccessResult is still KNOWN. A later PutJobSuccessResult on it is refused 400 InvalidJobStateException with the fixed text "Job with id <jobId> has already terminated" (jobId from the request), and the action stays Failed. JobNotFoundException is for a jobId the account never had.
Emulator: after PutJobFailureResult drives the job terminal, PutJobSuccessResult on the same jobId answers JobNotFoundException with message "Job with id '<jobId>' does not exist." — the job record is dropped on termination, so the wrong error class (and a wrong message, with the id quoted and a trailing period AWS does not use here) is served.
Why: a worker that retries a result after a lost response cannot tell "this job is finished" from "this job never existed"; the former is benign and the latter normally means a configuration or account error, so retry logic and alerting both misfire.
Fix: retain terminated jobs in the codepipeline crate's job store (with their terminal status) and classify a result reported against one as InvalidJobStateException in the PutJobSuccessResult/PutJobFailureResult handlers.
Traces: codepipeline.put-job-success-result-wrong-job-state-rejected.

=== Cause 4 of 8: PollForJobs' JobData omits artifactCredentials, so a job worker cannot read or write artifacts ===
AWS: Smithy models JobData$artifactCredentials -> AWSSessionCredentials (@sensitive), documented as "an Amazon Web Services session credentials object ... temporary credentials that are issued by Amazon Web Services Secure Token Service (STS)". Every job dispatched for a custom action that has input or output artifacts carries it, alongside inputArtifacts/outputArtifacts and encryptionKey, because that is the only way the worker can reach the artifact store.
Emulator: the job returned by PollForJobs has no artifactCredentials member at all — the check's job["data"]["artifactCredentials"] raises KeyError: 'artifactCredentials'.
Why: the documented custom-action job-worker contract is unimplementable against the emulator: the worker has no credentials to download its inputArtifacts or upload its outputArtifacts, so every third-party/custom action integration breaks at the first artifact access rather than at a modelled error.
Fix: the PollForJobs (and PollForThirdPartyJobs) handler's JobData assembly in the codepipeline crate — it must mint and attach session credentials for the pipeline's artifact store.
Traces: codepipeline.retry-stage-execution-stale-token-rejected.

=== Cause 5 of 8: StopPipelineExecution's not-stoppable refusal message is emulator-authored ===
AWS: Smithy models StopPipelineExecution with PipelineExecutionNotStoppableException and DuplicatedStopRequestException (both 400). Stopping an execution that is already in a terminal state is refused with AWS's fixed text "The pipeline execution with ID '<pipelineExecutionId>' in '<pipelineName>' can't be stopped now. Either it does not exist in the pipeline, or the pipeline execution has no action executions in progress." — the only variable parts are the execution id and the pipeline name from the request.
Emulator: the message is replaced with "Pipeline execution <pipelineExecutionId> has already reached the terminal status Stopped and cannot be stopped", which also exposes the internal status value instead of AWS's documented either/or explanation.
Why: operators and tooling that key off AWS's wording (and its documented ambiguity between "unknown execution" and "nothing in progress") get text that matches no AWS documentation, and the emulator asserts a distinction AWS deliberately does not make in this message.
Fix: the StopPipelineExecution handler's refusal construction in the codepipeline crate.
Traces: codepipeline.stopping-an-execution-leaves-no-deploy-artifact-and-a-restop-is-rejected.

=== Cause 6 of 8: StartPipelineExecution's PipelineNotFoundException message reuses GetPipeline's wording ===
AWS: Smithy models PipelineNotFoundException on both GetPipeline and StartPipelineExecution, but the two operations word it differently. GetPipeline answers "Account '<accountId>' does not have a pipeline with name '<name>'"; StartPipelineExecution answers "Pipeline with name <name> does not exist in account <accountId>." (fixed text, with the requested pipeline name and the caller's account id substituted, name unquoted, trailing period).
Emulator: StartPipelineExecution on a deleted pipeline answers PipelineNotFoundException with GetPipeline's text, "Account '<accountId>' does not have a pipeline with name '<name>'". The code and status are right; only the per-operation message is wrong.
Why: a client that parses or logs the mutation path's message sees the read path's phrasing; message-level assertions in deployment tooling and test suites that distinguish the two paths fail against the emulator.
Fix: the StartPipelineExecution handler's not-found refusal in the codepipeline crate (it must not share GetPipeline's message template).
Traces: codepipeline.e2e-pipeline-create-run-approve-complete-teardown.

=== Cause 7 of 8: RetryStageExecution refuses the pipeline's own most recent execution as NotLatestPipelineExecutionException ===
AWS: Smithy models RetryStageExecution(pipelineName, stageName, pipelineExecutionId, retryMode) with NotLatestPipelineExecutionException (400), which applies only when the supplied pipelineExecutionId is genuinely not the pipeline's latest execution. Retrying FAILED_ACTIONS of a failed stage of the execution that was just started and is the latest is accepted, reuses the same pipelineExecutionId, and re-runs the failed action.
Emulator: retrying the stage of the execution the check itself started (and which is the pipeline's most recent execution) is refused 400 NotLatestPipelineExecutionException with "Pipeline execution <pipelineExecutionId> is not the latest execution of pipeline <pipelineName>, so its stages cannot be retried". Hypothesis (neither the model nor the recording states the mechanism): the latest-execution bookkeeping records some other execution (e.g. an auto-triggered one) as latest, so the caller's own execution never qualifies.
Why: retrying a failed stage — the primary recovery action for a pipeline — is impossible: every attempt is refused with an error the client cannot act on, since there is no later execution to retry instead.
Fix: the RetryStageExecution handler's latest-execution resolution in the codepipeline crate and the per-pipeline 'latest execution' state it consults.
Traces: codepipeline.job-output-variables-precision-preserved.

=== Cause 8 of 8: PutWebhook hands back a webhook URL the emulator does not serve ===
AWS: Smithy models PutWebhook -> ListWebhookItem, whose url is the endpoint that receives the provider's POST. A correctly signed payload POSTed to that url is accepted with a 2xx and triggers a pipeline execution; a wrongly signed one is refused — but the url itself always resolves to a serving endpoint.
Emulator: the url returned by PutWebhook (host '<region>.webhooks.aws', path /trigger with the webhook token and version in the query) is not routed by the emulator at all. A correctly signed POST to the announced url answers HTTP 404 with body {"__type":"UnknownService","message":"no emulated service is registered for '<region>'"} — the emulator announced an address other than the one it is reachable on.
Why: webhook-driven pipelines cannot be exercised at all, and worse, the failure is indistinguishable from a correct rejection: every negative assertion ("a wrong signature starts no execution") passes vacuously against an endpoint that refuses everything, so a client cannot tell a working webhook from a dead one.
Fix: the PutWebhook handler's url construction in the codepipeline crate (it must advertise the address the emulator is reached on) and the webhook ingest route that must accept and HMAC-verify that POST.
Traces: codepipeline.webhook-url-rejects-a-wrong-hmac-signature-and-starts-no-execution.

## Evidence
All quoted verbatim from the replay report; job ids, execution ids, pipeline names and tokens are generated per run and are examples, not requirements.

Cause 1 — codepipeline.job-claim-nonce-single-use-after-acknowledgement-rejected [error]:
  botocore.errorfactory.InvalidNonceException: An error occurred (InvalidNonceException) when calling the AcknowledgeJob operation: Invalid nonce for job id 3188bf75-cfad-4145-9052-3e82f93f044e
The check acknowledges with job["nonce"] exactly as PollForJobs returned it; expected status 'InProgress' both for the first acknowledgement and for the idempotent repeat while InProgress, and only after PutJobSuccessResult expected ('ValidationException', "Job with id <id> has already been acknowledged").

Cause 2 — codepipeline.put-job-success-invalid-job-state-rejected [mismatch]:
  AssertionError: unexpected refusal: ('InvalidJobStateException', 'Job with id f3454c25-6d45-4cee-b2b1-8cb5e2190051 is in status Queued, so no result can be reported for it; only a job a worker has acknowledged and not yet completed accepts one')
Expected: ('InvalidJobStateException', 'Job with id <id> has not been acknowledged').

Cause 3 — codepipeline.put-job-success-result-wrong-job-state-rejected [mismatch]:
  AssertionError: expected InvalidJobStateException, got {'Message': "Job with id 'f2a3c533-6520-485c-9b48-91105c8fe090' does not exist.", 'Code': 'JobNotFoundException'}
Expected: 400 InvalidJobStateException with message "Job with id <id> has already terminated" after PutJobFailureResult drove that job Failed.

Cause 4 — codepipeline.retry-stage-execution-stale-token-rejected [error]:
  creds = job["data"]["artifactCredentials"]
  KeyError: 'artifactCredentials'

Cause 5 — codepipeline.stopping-an-execution-leaves-no-deploy-artifact-and-a-restop-is-rejected [mismatch]:
  AssertionError: expected message "The pipeline execution with ID 'a139701f-5a67-4b89-9a9a-f603bf992e72' in 'sim2real-behav-cpstopart-735681a86864' can't be stopped now. Either it does not exist in the pipeline, or the pipeline execution has no action executions in progress.", got 'Pipeline execution a139701f-5a67-4b89-9a9a-f603bf992e72 has already reached the terminal status Stopped and cannot be stopped'

Cause 6 — codepipeline.e2e-pipeline-create-run-approve-complete-teardown [mismatch]:
  AssertionError: expected StartPipelineExecution on the deleted pipeline to fail not-found, got ('PipelineNotFoundException', "Account '000000000000' does not have a pipeline with name 'sim2real-cp-5a43ef977951'")
Expected tuple: ('PipelineNotFoundException', "Pipeline with name <name> does not exist in account <accountId>."). The preceding GetPipeline assertion on the "Account '<id>' does not have a pipeline with name '<name>'" wording PASSED, so only the mutation path's message diverges.

Cause 7 — codepipeline.job-output-variables-precision-preserved [error]:
  botocore.errorfactory.NotLatestPipelineExecutionException: An error occurred (NotLatestPipelineExecutionException) when calling the RetryStageExecution operation: Pipeline execution 231d01a2-d3c6-4019-8caf-609e55925350 is not the latest execution of pipeline sim2real-cp-outvars-e72da52671f5, so its stages cannot be retried
The retried execution id is the one StartPipelineExecution returned moments earlier for that same pipeline.

Cause 8 — codepipeline.webhook-url-rejects-a-wrong-hmac-signature-and-starts-no-execution [mismatch]:
  AssertionError: a correctly signed webhook payload must be accepted by 'https://us-west-2.webhooks.aws/trigger?t=3aa633d8-b1f1-4f55-b212-1a14f25b108d&v=1'; expected a 2xx, got HTTP 404 with body '{"__type":"UnknownService","message":"no emulated service is registered for \'us-west-2\'"}'. If this fails, every absence assertion below would pass against an endpoint that refuses everything.

## Steps to Reproduce
Boot the emulator and point botocore at it; cp = client('codepipeline'), s3 = client('s3'), iam = client('iam').

Shared setup for causes 1-4 and 7: create a versioned artifact bucket with a source object, a pipeline role trusted by codepipeline.amazonaws.com, a Custom-owner action type via CreateCustomActionType, and a pipeline with an S3 (or custom) Source stage plus a Custom-owner Build stage.

Cause 1: job = cp.poll_for_jobs(actionTypeId=<custom type>, maxBatchSize=1)['jobs'][0]; cp.acknowledge_job(jobId=job['id'], nonce=job['nonce']) -> expect {'status': 'InProgress'}; repeat the identical call while the job is InProgress -> expect {'status': 'InProgress'} again; then cp.put_job_success_result(jobId=job['id']) and repeat the acknowledge -> expect 400 ValidationException "Job with id <id> has already been acknowledged". Observe InvalidNonceException "Invalid nonce for job id <id>" instead.
Cause 2: on a freshly polled, NOT yet acknowledged job, call cp.put_job_success_result(jobId=job['id']) -> expect 400 InvalidJobStateException "Job with id <id> has not been acknowledged"; observe the emulator's "... is in status Queued, so no result can be reported for it ..." text.
Cause 3: acknowledge a job, cp.put_job_failure_result(jobId=job['id'], failureDetails={'type':'JobFailed','message':'seed failure'}), wait for the action to report Failed, then cp.put_job_success_result(jobId=job['id']) -> expect 400 InvalidJobStateException "Job with id <id> has already terminated"; observe 400 JobNotFoundException "Job with id '<id>' does not exist.".
Cause 4: inspect the PollForJobs job for a custom action that declares inputArtifacts/outputArtifacts -> expect job['data']['artifactCredentials'] with accessKeyId/secretAccessKey/sessionToken; observe the member is absent.
Cause 5: start an execution, cp.stop_pipeline_execution(pipelineName=P, pipelineExecutionId=E), wait for the terminal status, then stop it again -> expect the AWS not-stoppable message quoted above; observe the emulator's terminal-status sentence.
Cause 6: cp.create_pipeline(...), cp.delete_pipeline(name=P), then cp.start_pipeline_execution(name=P) -> expect PipelineNotFoundException "Pipeline with name <P> does not exist in account <account>."; observe GetPipeline's "Account '<account>' does not have a pipeline with name '<P>'".
Cause 7: start an execution whose stage action fails, then cp.retry_stage_execution(pipelineName=P, stageName=S, pipelineExecutionId=<that execution>, retryMode='FAILED_ACTIONS') -> expect it to be accepted and reuse the id; observe NotLatestPipelineExecutionException.
Cause 8: url = cp.put_webhook(webhook={...GITHUB_HMAC...})['webhook']['url']; POST a correctly signed JSON payload to that url -> expect a 2xx; observe HTTP 404 {"__type":"UnknownService",...}.
Then re-replay the eight check ids in trace_ids.
