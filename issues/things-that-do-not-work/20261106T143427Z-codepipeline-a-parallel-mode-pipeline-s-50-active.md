Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.concurrent-pipeline-executions-limit-exceeded
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.concurrent-pipeline-executions-limit-exceeded|_attempt_start#16|Timeout|resp = cp.start_pipeline_execution(name=name)
Issue-Commit: b3aa6e7333d27a7c3c5b409bc7409fef676e9fb0

# CodePipeline: a PARALLEL-mode pipeline's 50-active-execution ceiling is never reached, so ConcurrentPipelineExecutionsLimitExceededException and the rejected-start reconciliation are unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `b3aa6e7333d27a7c3c5b409bc7409fef676e9fb0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
ONE net-new root cause in the codepipeline execution plane's active-execution accounting. It is the only failing check in this replay that the suite report marks neither ALREADY FILED nor OWNED.

Related but NOT covering (name them when fixing; do not assume they close this check):
- 20261105T133309Z-codepipeline-a-lambda-invoke-action-s-job-result.md describes this same family in its Cause 2 and even lists codepipeline.concurrent-pipeline-executions-limit-exceeded among its trace ids, but the replay key it carries for that check (`run#16|Timeout|time.sleep(1.0)`) does not match this replay's failure, so the suite report does not mark the check OWNED and no open issue carries its current replay key. If that issue is still open, fold this cause into it rather than fixing twice — but this check must be re-replayed and seen green before either is closed.
- 20261103T082129Z-codepipeline-refusal-wording-3-net-new-causes-startpipelineexecution.md covers only the WORDING of ConcurrentPipelineExecutionsLimitExceededException on StartPipelineExecution and PutActionRevision (and its evidence shows the emulator does raise that code in a different, non-PARALLEL shape). This cause is about the PARALLEL-mode ceiling never becoming observable at all, and carries a check id that issue does not.

=== Cause 1 of 1: the per-pipeline active-execution ceiling is never enforced/retired in executionMode PARALLEL ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models ConcurrentPipelineExecutionsLimitExceededException on StartPipelineExecution (errorKind=client, httpError 400, httpErrorDefaulted), alongside ConflictException (409), PipelineNotFoundException (400) and ValidationException (400); the success output is a single pipelineExecutionId matching ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$. ListPipelineExecutions (paginated on nextToken/pipelineExecutionSummaries) is the independent read of that history. The quota itself is documented rather than modelled (AWS General Reference, "CodePipeline endpoints and quotas" -> "Active executions per pipeline" = 50, fixed and non-adjustable, all regions); the message text is recorded behavior only. Required behavior: for a V2 pipeline with executionMode PARALLEL, where every started execution stays non-terminal concurrently, sequential StartPipelineExecution calls are accepted while fewer than the ceiling of active executions exist, each returning a distinct pipelineExecutionId that is independently visible through ListPipelineExecutions; the call that would exceed the ceiling is refused HTTP 400 with error code ConcurrentPipelineExecutionsLimitExceededException and the message "The limit for concurrent pipeline executions has been exceeded. Please try again later." — that string is entirely fixed text with no variable parts, so nothing in it is taken from the request or from state. The refused call must create nothing: ListPipelineExecutions must report no summary for it. The accepted count must never exceed the ceiling, and as executions retire the count must fall again so later starts are admitted.
Emulator: the probe never reaches a refusal. The check stands up a versioned source bucket, an artifact bucket, a pipeline role, a Lambda whose handler sleeps long enough that every execution stays active, and a V2/PARALLEL pipeline with an S3 Source stage plus an Invoke/Lambda "Delay" stage (the CreatePipeline response assertion executionMode == "PARALLEL" is reached and passes). It then issues StartPipelineExecution sequentially up to the ceiling plus a small margin, pacing the calls ~1s apart and retrying ThrottlingException/TooManyRequestsException/RequestLimitExceeded, and stops at the suite's per-check ceiling with "Error: Check timed out after 60.0s" — no ConcurrentPipelineExecutionsLimitExceededException is ever returned, so neither the ceiling, the refusal's status/code/message, nor the "a rejected start created nothing" reconciliation through ListPipelineExecutions is reachable. Hypothesis (neither the model nor the recording states the mechanism): the emulator does not count non-terminal executions per pipeline when executionMode is PARALLEL, so every start is admitted and the probe exhausts its whole search range; a contributing possibility is that the Invoke/Lambda action is never driven, leaving executions in a state the counter does not treat as active. FIXER NOTE: part of this check's elapsed time is its own deliberate ~1s pacing between sequential starts plus Lambda/role setup, so the 60s budget is tight by construction — reproduce it directly with the steps below and judge the ceiling behavior itself, not only the timeout.
Why: a client that fills a PARALLEL-mode pipeline and relies on the documented quota refusal to back off instead sees unbounded acceptance, and a client matching that fixed throttling-class sentence before retrying never matches anything. With more than the ceiling admitted, the pipeline's own concurrency guarantee is wrong and every reader of the execution history (ListPipelineExecutions, GetPipelineState) reports executions AWS would never have created.
Fix: the active-execution accounting behind StartPipelineExecution in the codepipeline crate — a per-pipeline count of non-terminal executions that honours executionMode PARALLEL, the execution driver that must retire an execution so that count falls again, and the refusal construction (400 + ConcurrentPipelineExecutionsLimitExceededException + the fixed message) in the StartPipelineExecution handler, with the refused call making no state change visible to ListPipelineExecutions.
Traces: codepipeline.concurrent-pipeline-executions-limit-exceeded.

## Evidence
Verbatim from this replay's failing-check list under the "### codepipeline" heading:

  #### [mismatch] codepipeline.concurrent-pipeline-executions-limit-exceeded
  Error: Check timed out after 60.0s

The check is NOT marked ALREADY FILED and NOT marked OWNED in this report; it is the only such failure in the run (206 checks, 128 passed, 68 mismatched, 10 errored).

What the check does and expects (from the check source carried in the report):
- Creates a V2 pipeline with executionMode "PARALLEL", stages Source (category Source / owner AWS / provider S3 / version 1, PollForSourceChanges=false) and Delay (category Invoke / owner AWS / provider Lambda / version 1) whose function sleeps so executions stay active. The assertion create_resp["pipeline"]["executionMode"] == "PARALLEL" is reached and passes.
- Issues StartPipelineExecution(name=P) sequentially up to the documented ceiling plus a margin, pacing calls ~1s apart, retrying ThrottlingException/TooManyRequestsException/RequestLimitExceeded per call.
- Expects exactly one call to be refused with Error.Code == "ConcurrentPipelineExecutionsLimitExceededException" and Error.Message == "The limit for concurrent pipeline executions has been exceeded. Please try again later." (asserted as an exact, wholly fixed string), with no other error code appearing before the ceiling.
- Expects the accepted ids to be distinct, at least one accepted, the accepted count <= the 50-active-execution ceiling, every accepted id visible through the ListPipelineExecutions paginator, and len(listed) - len(accepted) <= 1 (slack for the pipeline's own auto-triggered execution) so the refused start demonstrably created nothing.

Actual: no refusal is observed within the budget; the check ends at the suite ceiling, so the error's status (400), code and fixed message are never produced and the rejected-start reconciliation is never evaluated. Bucket names, role names, function names, execution ids and the 840s Lambda sleep are per-run/implementation values that fill the scenario and are not themselves requirements; the 50-active-execution ceiling and the fixed message text are.

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline'), s3 = boto3.client('s3'), iam = boto3.client('iam'), lam = boto3.client('lambda').

1. Create an artifact bucket, and a versioned source bucket holding a zip at key 'src/app.zip'.
2. Create a Lambda execution role trusted by lambda.amazonaws.com with inline logs:* plus codepipeline:PutJobSuccessResult / PutJobFailureResult, then create a python3.12 function whose handler sleeps long enough that executions remain active for the whole probe and then calls cp.put_job_success_result(jobId=event['CodePipeline.job']['id']). Poll GetFunctionConfiguration until State == 'Active'.
3. Create a pipeline role trusted by codepipeline.amazonaws.com with s3:* and lambda:InvokeFunction.
4. cp.create_pipeline(pipeline={... 'pipelineType': 'V2', 'executionMode': 'PARALLEL', stages: Source = {category Source, owner AWS, provider S3, version '1'} with S3Bucket/S3ObjectKey/PollForSourceChanges='false' and outputArtifacts [SourceOutput]; Delay = {category Invoke, owner AWS, provider Lambda, version '1'} with configuration FunctionName=<fn>}). Confirm the response reports executionMode 'PARALLEL'.
5. Call cp.start_pipeline_execution(name=P) sequentially, pacing the calls ~1s apart and retrying ThrottlingException/TooManyRequestsException/RequestLimitExceeded, collecting each returned pipelineExecutionId. Expect the calls to be accepted while fewer than 50 executions are active and the NEXT one to be refused HTTP 400 with Error.Code 'ConcurrentPipelineExecutionsLimitExceededException' and Error.Message exactly 'The limit for concurrent pipeline executions has been exceeded. Please try again later.' — observe instead that acceptance continues past the ceiling and no refusal is ever returned. Note: no error code other than that one may appear before the ceiling is reached.
6. Paginate cp.list_pipeline_executions(pipelineName=P). Expect every accepted id present, the accepted ids distinct, their count <= 50, and at most one summary beyond the accepted ones (the pipeline's own auto-triggered execution) — the refused start must have created nothing.
7. Let the delay actions complete (or shorten the sleep) and start again: expect the admitted count to recover as executions retire, i.e. the counter must fall, not be a high-water mark.

Then re-replay codepipeline.concurrent-pipeline-executions-limit-exceeded.
