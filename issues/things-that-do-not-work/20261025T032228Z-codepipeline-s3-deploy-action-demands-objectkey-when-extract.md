Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.deploy-pipeline-via-cloudformation-behaves codepipeline.deregister-webhook-validation-rejected codepipeline.list-rule-executions-ordering codepipeline.poll-for-jobs-lease-expires-and-job-becomes-available-again codepipeline.put-action-revision-concurrency-limit-rejected codepipeline.update-action-type-not-found
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.deploy-pipeline-via-cloudformation-behaves|run#15|AssertionError|assert execution["status"] == "Succeeded", (
Issue-Key: codepipeline.deregister-webhook-validation-rejected|run#12|AssertionError|assert code_not_found == "WebhookNotFoundException", f"unknown name: expected WebhookNotFoundException, got {code_not_found}"
Issue-Key: codepipeline|ListRuleExecutions|NotImplemented
Issue-Key: codepipeline|AcknowledgeJob|InvalidNonceException
Issue-Key: codepipeline.put-action-revision-concurrency-limit-rejected|run#11|AssertionError|assert limit_code == "ConcurrentPipelineExecutionsLimitExceededException", (
Issue-Key: codepipeline.update-action-type-not-found|run#10|AssertionError|assert message == _not_found_message("ThirdParty", "Build", missing_provider, "1"), message
Issue-Commit: afc21d5623f1a8f7411b21bbb87a0dff6c2e7024

# CodePipeline: S3 deploy action demands ObjectKey when Extract=true, two operations answer NotImplemented, AcknowledgeJob refuses a valid outstanding nonce, PutActionRevision enforces no concurrency ceiling, UpdateActionType's not-found message diverges (6 causes)

> _Note: this issue was identified against the code base at commit `afc21d5623f1a8f7411b21bbb87a0dff6c2e7024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutActionRevision

## Description
SIX net-new root causes in the codepipeline crate, each grounded in a failing behavioral check this replay left unmarked (not ALREADY FILED, not OWNED). A fixer must close ALL SIX before submitting; each cause names its own failing check id. Causes 2 and 3 are partial implementations of operations whose full-stub issues are already resolved (20261022T173520Z-codepipeline-listruleexecutions-stub.md and the webhook stubs), so no scaffold-stub issue covers them. The execution-engine stall is filed separately in this same sweep; Cause 1 below is a DIFFERENT defect — the action runs and is refused on its declaration.

=== Cause 1 of 6: an S3 Deploy action is failed for a missing ObjectKey even though Extract is 'true' ===
AWS: Smithy models ActionDeclaration$configuration as a free-form ActionConfigurationMap, so the key requirements are the S3 deploy provider's: BucketName is required, Extract is required, and ObjectKey is required ONLY when Extract is 'false' (with Extract='true' the artifact is unzipped into the bucket and there is no single object key). Recorded behavior: a CloudFormation-declared pipeline whose Deploy action carries only {BucketName, Extract: 'true'} runs to completion and unzips the artifact into the deploy bucket.
Emulator: the action is rejected and the whole execution reaches 'Failed' with statusSummary 'action DeployAction in stage Deploy failed: an S3 deploy action must declare the configuration keys BucketName and ObjectKey' — ObjectKey is demanded unconditionally.
Why: the canonical S3 deploy shape (BucketName + Extract) is unusable, so every pipeline built from AWS's own templates or console defaults fails at Deploy, and the failure is reported as the user's configuration error when the declaration is valid.
Fix: the S3 deploy action runner's configuration validation in the codepipeline crate's execution plane — require ObjectKey only when Extract is absent/'false', and extract the artifact into BucketName when Extract is 'true'.
Traces: codepipeline.deploy-pipeline-via-cloudformation-behaves.

=== Cause 2 of 6: DeregisterWebhookWithThirdParty answers NotImplemented for a well-formed but unregistered webhookName ===
AWS: Smithy: DeregisterWebhookWithThirdParty$webhookName (length 1-100, pattern ^[A-Za-z0-9.@\-_]+$), errors ValidationException and WebhookNotFoundException — and nothing else. A name that satisfies the pattern but names no registered webhook is refused 400 WebhookNotFoundException.
Emulator: the pattern-violation path already answers ValidationException correctly, but the well-formed unknown name is answered with error code 'NotImplemented', which is not in the operation's modeled error set.
Why: a client cannot tell 'this webhook does not exist' (a terminal, handleable condition) from 'this emulator cannot do it'; NotImplemented is unmodelled, so botocore surfaces it as a generic ClientError and teardown/idempotent-deregister paths break.
Fix: the DeregisterWebhookWithThirdParty handler in the codepipeline crate's ops — resolve the name against the stored webhooks and raise WebhookNotFoundException when absent, instead of returning NotImplemented after validation.
Traces: codepipeline.deregister-webhook-validation-rejected.

=== Cause 3 of 6: ListRuleExecutions answers NotImplemented when the pipeline's stages declare condition rules ===
AWS: Smithy: ListRuleExecutions(pipelineName, filter, maxResults, nextToken) -> ruleExecutionDetails + nextToken, paginated, errors InvalidNextTokenException, PipelineExecutionNotFoundException, PipelineNotFoundException, ValidationException. For a pipeline whose stages declare conditions it returns the rule executions in evaluation order (an empty list before any rule has run).
Emulator: 'An error occurred (NotImplemented) when calling the ListRuleExecutions operation: operation ListRuleExecutions for a pipeline whose stages declare condition rules is scaffolded but not implemented in this emulator' — i.e. the operation is implemented only for pipelines WITHOUT conditions, which is precisely the case for which it carries no information.
Why: condition/rule evaluation is unobservable, and the refusal is an unmodelled error code rather than one of the four the operation declares, so no client branch handles it.
Fix: the ListRuleExecutions handler in the codepipeline crate's ops, plus the rule-execution records the execution plane must publish for a stage's conditions.
Traces: codepipeline.list-rule-executions-ordering.

=== Cause 4 of 6: AcknowledgeJob refuses an earlier, still-outstanding nonce once the job has been redelivered ===
AWS: Smithy: AcknowledgeJob(jobId, nonce) -> status (JobStatus), errors InvalidNonceException, JobNotFoundException, ValidationException. Recorded behavior: a job dispatched by PollForJobs and never acknowledged resurfaces with a FRESH nonce, and the earlier first-dispatch nonce is NOT invalidated by that redelivery — acknowledging with it succeeds and returns status 'InProgress'. Only after an acknowledgement closes the claim do the other outstanding nonces stop working, with InvalidNonceException (recorded message, an example: 'Invalid nonce for job id <jobId>').
Emulator: the redelivery works (the job resurfaces with a new nonce), but AcknowledgeJob with the earlier outstanding nonce is refused 400 InvalidNonceException ('Nonce <n> is not the nonce the newest poll published for job <id>') — only the newest nonce is accepted, the inverse of AWS's rule.
Why: a job worker that polled, then lost a race to a redelivery, can never acknowledge the job it holds, so the dispatched work is permanently unclaimable; and the invalidation order a client relies on (claim closes on first successful ack) is reversed.
Fix: the AcknowledgeJob handler and the job-nonce lifecycle in the codepipeline crate — keep every nonce issued for an unclaimed job valid, and invalidate the set only when one of them successfully claims the job.
Traces: codepipeline.poll-for-jobs-lease-expires-and-job-becomes-available-again.

=== Cause 5 of 6: PutActionRevision never enforces the per-pipeline active-execution ceiling ===
AWS: Smithy: PutActionRevision errors include ConcurrentPipelineExecutionsLimitExceededException (client, 400). AWS publishes a default 'active executions per pipeline' ceiling of 50 for PARALLEL-mode pipelines; once it is reached, a PutActionRevision whose new revision would trigger a further execution is refused with that error and creates no execution.
Emulator: 71 sequential PutActionRevision calls, each with a genuinely new revision on a PARALLEL/V2 pipeline, were all accepted — no call was ever refused ('never hit the concurrency ceiling after 71 executions; last error code=None').
Why: the documented back-pressure never arrives, so a client that depends on it to stop queueing work sees unbounded execution creation instead of a 400, and the emulator's execution store grows without bound.
Fix: the PutActionRevision handler in the codepipeline crate's ops — count the pipeline's in-flight (started/queued) executions against the per-pipeline ceiling the execution plane already defines for StartPipelineExecution and refuse with ConcurrentPipelineExecutionsLimitExceededException before minting another. A sibling check, codepipeline.concurrent-pipeline-executions-limit-exceeded, observes the same ceiling missing on StartPipelineExecution and is OWNED by 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md; that issue does not carry the PutActionRevision path or this check id.
Traces: codepipeline.put-action-revision-concurrency-limit-rejected.

=== Cause 6 of 6: UpdateActionType's ActionTypeNotFoundException message is not built by AWS's request-derived rule ===
AWS: Smithy: UpdateActionType(actionType: ActionTypeDeclaration) -> Unit, errors ActionTypeNotFoundException, RequestFailedException, ValidationException. The message is built FROM THE REQUEST's action-type id, naming each member: 'An action type with owner=[<owner>], category=[<category>], provider=[<provider>] and version=[<version>] doesn't exist. To create an action type, contact the CodePipeline service team.' (replayed against real AWS for both a fabricated ThirdParty id and the real AWS-owned Approval/AWS/Manual/1 id). That naming rule — not the one recorded instance — is the requirement.
Emulator: the status and the code are right (400 ActionTypeNotFoundException) but the message follows a different rule: "ActionType (Category='Build', Owner='ThirdParty', Provider='<provider>', Version='1') is not registered in account '000000000000'".
Why: the text is the only place the refusal names what was looked up; a client or operator matching AWS's wording (or the 'contact the CodePipeline service team' entitlement hint, which tells them the operation is unreachable in a non-partner account rather than that the id was mistyped) is misled. This is the same defect class as the resolved 20261023T080113Z-codepipeline-error-messages-are-not-built-by-aws.md, which covers PipelineNotFound/pipeline-version/PipelineExecutionNotFound/StageNotFound and not this message.
Fix: the UpdateActionType handler's ActionTypeNotFoundException construction in the codepipeline crate's ops — format the message from the request's actionType.id members in AWS's owner/category/provider/version order with the service-team sentence.
Traces: codepipeline.update-action-type-not-found.

## Evidence
All details are verbatim from the simparity-behavioral-ga-prod-v1 replay (102 checks, 47 passed; codepipeline 46/84). Pipeline, bucket, role, provider, job and execution ids are generated per run and are examples.

Cause 1 — codepipeline.deploy-pipeline-via-cloudformation-behaves [mismatch]: 'AssertionError: pipeline execution must reach Succeeded, got 'Failed': {... 'status': 'Failed', 'statusSummary': 'action DeployAction in stage Deploy failed: an S3 deploy action must declare the configuration keys BucketName and ObjectKey', 'artifactRevisions': [{'name': 'SourceArtifact'}], 'trigger': {'triggerType': 'StartPipelineExecution'}, 'executionMode': 'SUPERSEDED', 'executionType': 'STANDARD'}'. The stack's Deploy action declares exactly Configuration: {BucketName: !Ref DeployBucket, Extract: 'true'} and no ObjectKey; GetPipeline echoed the stages ['Source','Deploy'] and the Source artifact was produced, so only the deploy-action key check diverges. Expected: Succeeded, the Deploy stage's latestExecution Succeeded, and 'app.txt' unzipped into the deploy bucket.

Cause 2 — codepipeline.deregister-webhook-validation-rejected [mismatch]: 'AssertionError: unknown name: expected WebhookNotFoundException, got NotImplemented'. The preceding assertion (malformed name -> ValidationException with the pattern message) passed, so validation runs and only the resolution path is unimplemented.

Cause 3 — codepipeline.list-rule-executions-ordering [error]: 'botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListRuleExecutions operation: operation ListRuleExecutions for a pipeline whose stages declare condition rules is scaffolded but not implemented in this emulator'.

Cause 4 — codepipeline.poll-for-jobs-lease-expires-and-job-becomes-available-again [error]: 'botocore.errorfactory.InvalidNonceException: An error occurred (InvalidNonceException) when calling the AcknowledgeJob operation: Nonce 001bca2a-... is not the nonce the newest poll published for job 8049ffdb-...'. The check reached this line only because the redelivery it polls for (same jobId, different nonce) DID happen; AWS accepts the first-dispatch nonce here and returns status 'InProgress'.

Cause 5 — codepipeline.put-action-revision-concurrency-limit-rejected [mismatch]: 'AssertionError: never hit the concurrency ceiling after 71 executions; last error code=None'. The positive control passed (newRevision True plus a pipelineExecutionId), so each call did trigger an execution.

Cause 6 — codepipeline.update-action-type-not-found [mismatch]: the code assertion passed and the message assertion failed with the emulator's text: "ActionType (Category='Build', Owner='ThirdParty', Provider='sim2real-behav-ca-none-...', Version='1') is not registered in account '000000000000'", against AWS's "An action type with owner=[ThirdParty], category=[Build], provider=[<provider>] and version=[1] doesn't exist. To create an action type, contact the CodePipeline service team."

## Steps to Reproduce
Boot the emulator and point botocore at it; cp = client('codepipeline'), s3 = client('s3'), iam = client('iam'), cfn = client('cloudformation').

Cause 1: create source/artifact/deploy buckets and a pipeline role; cp.create_pipeline with Source = {category Source, owner AWS, provider S3} (configuration S3Bucket/S3ObjectKey) and Deploy = {category Deploy, owner AWS, provider S3} with configuration {BucketName: <deployBucket>, Extract: 'true'} and inputArtifacts=[{'name':'SourceArtifact'}]; put a zip at the source key; cp.start_pipeline_execution(name=P); poll cp.get_pipeline_execution -> expect 'Succeeded' and the zip's members unzipped into <deployBucket>; observe 'Failed' with statusSummary demanding ObjectKey. Re-replay codepipeline.deploy-pipeline-via-cloudformation-behaves.

Cause 2: cp.deregister_webhook_with_third_party(webhookName='bad name with spaces!') -> ValidationException (passes today); then cp.deregister_webhook_with_third_party(webhookName='sim2real-cp-nowebhook-xxxx') -> expect 400 WebhookNotFoundException; observe NotImplemented. Re-replay codepipeline.deregister-webhook-validation-rejected.

Cause 3: create a V2 pipeline whose stage declares conditions (beforeEntry/onSuccess rules), then cp.list_rule_executions(pipelineName=P) -> expect 200 with ruleExecutionDetails (possibly empty) and newest-first ordering; observe NotImplemented. Re-replay codepipeline.list-rule-executions-ordering.

Cause 4: create an S3-source + Custom-owner Build pipeline and a custom action type; cp.start_pipeline_execution; job1 = cp.poll_for_jobs(actionTypeId=..., maxBatchSize=1)['jobs'][0]; do NOT acknowledge; poll cp.poll_for_jobs until the same job id comes back with a different nonce; then cp.acknowledge_job(jobId=job1['id'], nonce=job1['nonce']) -> expect status 'InProgress'; observe 400 InvalidNonceException. Then cp.acknowledge_job with the SECOND nonce -> expect InvalidNonceException. Re-replay codepipeline.poll-for-jobs-lease-expires-and-job-becomes-available-again.

Cause 5: create a PARALLEL-mode V2 pipeline with an S3 source; call cp.put_action_revision(pipelineName=P, stageName='Source', actionName='SourceAction', actionRevision={'revisionId': f'rev-{i}', 'revisionChangeId': f'chg-{i}', 'created': now}) in a loop -> expect one call past the 50-active-execution ceiling to be refused 400 ConcurrentPipelineExecutionsLimitExceededException and to create no execution; observe 70+ consecutive successes. Re-replay codepipeline.put-action-revision-concurrency-limit-rejected.

Cause 6: cp.update_action_type(actionType={'id': {'category':'Build','owner':'ThirdParty','provider':'<never-created>','version':'1'}, 'executor': {...}, 'inputArtifactDetails': {...}, 'outputArtifactDetails': {...}}) -> expect 400 ActionTypeNotFoundException whose message is 'An action type with owner=[ThirdParty], category=[Build], provider=[<never-created>] and version=[1] doesn't exist. To create an action type, contact the CodePipeline service team.'; repeat with the real AWS-owned id (category 'Approval', owner 'AWS', provider 'Manual', version '1') -> the same code and the same message rule. Re-replay codepipeline.update-action-type-not-found.
