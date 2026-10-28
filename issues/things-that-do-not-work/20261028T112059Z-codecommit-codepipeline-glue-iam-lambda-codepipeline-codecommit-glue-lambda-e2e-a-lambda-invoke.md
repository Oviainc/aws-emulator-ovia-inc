Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__codepipeline__glue__iam__lambda.refarch-glue-deploy-reports-success-before-the-glue-run-finishes
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-deploy-reports-success-before-the-glue-run-finishes|run#24|AssertionError|assert deploy['status'] == 'Succeeded', (
Issue-Commit: 33c144d0be02a64a45778f2b1067b870ff3dcbed

# codepipeline (codecommit/glue/lambda e2e): a Lambda-invoke Deploy action that reported job success terminates 'Failed', so a green CodeCommit→Lambda→Glue pipeline is never green

> _Note: this issue was identified against the code base at commit `33c144d0be02a64a45778f2b1067b870ff3dcbed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutJobSuccessResult

## Description
SERVICE ATTRIBUTION NOTE: the grounding check id is `codecommit__codepipeline__glue__iam__lambda.refarch-glue-deploy-reports-success-before-the-glue-run-finishes`. Its leading token is a multi-service compound, not a single service token, so the per-service bootstrap gate could not place it by exact equality; it is filed under that compound token (the convention this repo already uses for multi-service issues such as `logs__s3`, `ec2__iam`, `codecommit__events__sqs`). Every component service (codecommit, codepipeline, glue, iam, lambda) is bootstrapped and this group does answer (4/5 of its checks pass), so the finding is a fidelity divergence, not a bootstrap gap. The divergence itself is in the CodePipeline execution engine.

Cause 1 of 1: a CodePipeline V2 `Invoke`/`Lambda` action whose function reports success through PutJobSuccessResult is still recorded as `Failed`.

AWS: For a V2 pipeline with stage `Source` (actionTypeId category Source, owner AWS, provider CodeCommit, version 1, configuration RepositoryName/BranchName/PollForSourceChanges=false) feeding stage `Deploy` (category Invoke, owner AWS, provider Lambda, version 1, configuration FunctionName/UserParameters), AWS invokes the named function with the `CodePipeline.job` envelope and leaves the action `InProgress` until the job is reported. When the function calls `codepipeline:PutJobSuccessResult(jobId=<the envelope's job id>)` the action execution moves to `Succeeded`, and with no further stages the pipeline execution moves to `Succeeded`. Smithy: `PutJobSuccessResult` input `jobId` is required with pattern `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, modelled errors `InvalidJobStateException`, `JobNotFoundException`, `OutputVariablesSizeExceededException`, `ValidationException` (all client, HTTP 400); `ActionExecutionStatus` = `InProgress | Abandoned | Succeeded | Failed`; `ActionExecutionDetail.status` is read back through `ListActionExecutions`, and `ActionExecutionDetail.output.executionResult` (`ActionExecutionResult`) is where a provider's `externalExecutionId`, `externalExecutionSummary` and `errorDetails` are reported. The invoked function in this scenario only reaches its `PutJobFailureResult` branch when its own body raises — it calls `glue.start_job_run(JobName=<UserParameters.glue_job_name>)` then `pipeline.put_job_success_result(jobId=job['id'])` inside a try, and the except branch reports `{"type": "JobFailed", "message": str(e), "externalExecutionId": context.aws_request_id}`.

Emulator: the Source action of the seeded execution does reach `Succeeded` and resolves `output.executionResult.externalExecutionId` to the seeded commit id (those assertions pass), so the pipeline starts and the first stage is fine. The `Deploy` action then reaches a TERMINAL status of `Failed`: `ListActionExecutions` reports `status == 'Failed'` for stageName `Deploy` where AWS reports `'Succeeded'` (replay: "AssertionError: the Deploy action reports 'Failed', expected Succeeded"). Because the action is terminal-Failed rather than stuck `InProgress`, the engine did run the invoke and did take a job result — the result it took was a failure. The replay surfaces only the status: the emulator records nothing in the action's `output.executionResult.externalExecutionSummary`/`errorDetails` that names what failed, which is itself part of the divergence (AWS reports the `failureDetails.message` the function supplied there). Hypothesis (neither the model nor the recording states which call was refused): one of the two calls the invoked function makes on the caller's behalf — `glue:StartJobRun` for the job created moments earlier, or `codepipeline:PutJobSuccessResult` with the job id carried in the `CodePipeline.job` envelope (e.g. an envelope job id that does not satisfy the modelled uuid pattern, or a job the engine no longer holds, yielding `ValidationException`/`JobNotFoundException`/`InvalidJobStateException`) — is refused, driving the handler into its `PutJobFailureResult` branch and the action to `Failed`.

Why: a client that wires any custom-job deploy stage (the standard `Invoke`/`Lambda` action — the mechanism behind every "run my own step in the pipeline" pattern) sees the pipeline go red even though its function reported success, and gets no detail saying why. The pipeline's own red/green signal is therefore inverted for custom-job actions: a success path cannot be observed at all, so no CI/CD scenario built on Invoke/Lambda actions can be exercised, and the stronger fidelity claim this check exists to make (that the pipeline reports Succeeded BEFORE the Glue run it launched resolves, i.e. that green is not ETL-green) can never be reached.

Fix: the CodePipeline execution engine's `Invoke`/`Lambda` action executor and the custom-job lifecycle it exposes — the job envelope it hands to the invoked function (the `CodePipeline.job` id must be a value `PutJobSuccessResult`/`PutJobFailureResult` accept per the modelled pattern, and must still be held as an in-flight job when the function answers), the `PutJobSuccessResult` handler's transition of the owning action execution to `Succeeded` and of the execution to `Succeeded`, and the propagation of a reported `failureDetails` into `ActionExecutionDetail.output.executionResult.errorDetails`/`externalExecutionSummary` so a failed job is diagnosable. The dependency calls the invoked function makes on the way (Lambda Invoke of the launcher, Glue `StartJobRun` for an existing job with a passed role) belong in their own handlers if the refusal originates there.

Related but NOT covering this: open issues `20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md` (actions that never LEAVE InProgress — here the action is terminal), `20261024T014332Z-codebuild-codepipeline-ecr-codepipeline-build-stages-fail-and-record-no-action.md` (CodeBuild build stages recording no action executions at all — here the action execution is recorded, with the wrong verdict, and no CodeBuild action is involved) and `20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md` (control-plane declaration/transition validation). None of them names the Invoke/Lambda custom-job success path.

Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-deploy-reports-success-before-the-glue-run-finishes.

## Evidence
Replay of check `codecommit__codepipeline__glue__iam__lambda.refarch-glue-deploy-reports-success-before-the-glue-run-finishes` [mismatch]:

  File "check_codecommit__codepipeline__glue__iam__lambda_refarch_glue_deploy_reports_success_before_the_glue_run_finishes.py", line 80, in run
    assert deploy['status'] == 'Succeeded', (
  AssertionError: the Deploy action reports 'Failed', expected Succeeded. The finding is that this stage reports success unconditionally, so a Failed Deploy here means the launcher DID relay the Glue outcome and the operator would have seen the problem in the pipeline

Expected (AWS): ListActionExecutions reports, for the seeded pipeline execution, stageName `Deploy` with `status` `Succeeded` and `output.executionResult.externalExecutionId` equal to the invoked function's name; the pipeline execution then reports `Succeeded`.
Actual (emulator): the same action reaches terminal `status` `Failed`. The preceding assertions pass, i.e. stageName `Source` reports `Succeeded` with `output.executionResult.externalExecutionId` == the commit id returned by codecommit PutFile, so the execution did start and the source did resolve. No failure cause is observable: the emulator exposes no `output.executionResult.errorDetails`/`externalExecutionSummary` carrying the `failureDetails.message` the function would have supplied.

Model grounding (services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json):
- PutJobSuccessResult: input {jobId (required, pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$), currentRevision, continuationToken, executionDetails, outputVariables}; errors InvalidJobStateException, JobNotFoundException, OutputVariablesSizeExceededException, ValidationException (client, 400).
- ActionExecutionStatus enum: InProgress, Abandoned, Succeeded, Failed.
- ActionExecutionDetail.status / .output.executionResult (ActionExecutionResult: externalExecutionId, externalExecutionSummary, externalExecutionUrl, errorDetails, logStreamARN), read via ListActionExecutions (paginated on nextToken/actionExecutionDetails).

The values above that come from one run (the pipeline/function/job/repository names, the commit id, the execution id) are placeholders filled from the request and the emulator's own state; the requirement is the status/field semantics, not those literals.

## Steps to Reproduce
1. Create a CodeCommit repository.
2. Create an S3 artifact bucket and put `etl.py` into it.
3. Create a Glue role plus a Glue job (`Command={Name: glueetl, ScriptLocation: s3://<bucket>/etl.py}`, GlueVersion 4.0, 2 x G.1X workers).
4. Create a Lambda execution role allowing glue:StartJobRun/GetJobRun/GetJob, codepipeline:PutJobSuccessResult/PutJobFailureResult and iam:PassRole on the Glue role, attach AWSLambdaBasicExecutionRole, and create a python3.12 function whose handler reads `event["CodePipeline.job"]`, calls `glue.start_job_run(JobName=json.loads(config["UserParameters"])["glue_job_name"])`, then `codepipeline.put_job_success_result(jobId=job["id"])`, and on any exception calls `put_job_failure_result(jobId=..., failureDetails={type: JobFailed, message: str(e), externalExecutionId: context.aws_request_id})`. Wait for State == Active.
5. CreatePipeline (pipelineType V2, S3 artifactStore on the bucket) with stage Source = CodeCommit source action on branch `main` (PollForSourceChanges=false, outputArtifacts [src]) and stage Deploy = Invoke/Lambda action with configuration {FunctionName: <launcher>, UserParameters: {"glue_job_name": ..., "glue_role": ...}} and inputArtifacts [src].
6. codecommit PutFile on branch `main` to seed the branch, then StartPipelineExecution.
7. Poll ListActionExecutions until the `Deploy` action reaches `Succeeded` or `Failed`.

Observed: the Deploy action reaches `Failed` (and no errorDetails/externalExecutionSummary explains why). Expected: `Succeeded`, with `output.executionResult.externalExecutionId` equal to the launcher function name, and the pipeline execution reporting `Succeeded`.
