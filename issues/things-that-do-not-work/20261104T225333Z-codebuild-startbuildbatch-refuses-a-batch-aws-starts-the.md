Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.build-batch-environment-variables-preserved
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|StartBuildBatch|InvalidInputException
Issue-Commit: 3ec1d1c73a5c071becaf65f18e9f0690660fa546

# codebuild: StartBuildBatch refuses a batch AWS starts — the batch-enablement gate ignores buildBatchConfigOverride, so the whole batch surface is unreachable

> _Note: this issue was identified against the code base at commit `3ec1d1c73a5c071becaf65f18e9f0690660fa546`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuildBatch

## Description
ONE root cause in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006).

Cause 1 of 1: StartBuildBatch answers a 400 InvalidInputException for a request real CodeBuild accepts, because its batch-enablement gate is a test of the STORED project alone and never considers the buildBatchConfigOverride the same request carries.

AWS: Smithy StartBuildBatch(projectName @required NonEmptyString @length min 1; buildBatchConfigOverride ProjectBuildBatchConfig; environmentVariablesOverride EnvironmentVariables — the model puts NO @length on that list; buildspecOverride String; ...) -> StartBuildBatchOutput{buildBatch: BuildBatch}. The only modeled errors are InvalidInputException and ResourceNotFoundException, both errorKind client, httpError 400 (defaulted). The model states no precondition that the project carry a stored buildBatchConfig: it models buildBatchConfigOverride precisely so a start call can state the batch configuration itself, exactly as every other `*Override` member overlays the stored project for that one batch. Recorded behavior for this call: HTTP 200 carrying a BuildBatch for a NO_SOURCE project created WITHOUT buildBatchConfig, started with buildspecOverride carrying a `batch: build-list:` section, environmentVariablesOverride, and buildBatchConfigOverride={serviceRole: <role arn>} — i.e. AWS resolves the effective batch configuration from the request's override on top of the project's and mints the batch.

Emulator: HTTP 400 InvalidInputException with the message "BatchConfig must be configured on the project to use the BuildBatch feature" (wholly fixed text — nothing is interpolated from the request or from state; it is crate::state::PROJECT_NOT_BATCH_ENABLED_MESSAGE). No BuildBatch is returned or persisted. The gate is unconditional on the stored project and runs ahead of idempotency-token resolution, so a repeat of an already-accepted start is refused on the same ground.

Why: a client that starts a batch build the way AWS allows is told its input is invalid and never receives a buildBatch id, so everything downstream of the start is unreachable — BatchGetBuildBatches, ListBuildBatches/ListBuildBatchesForProject, environmentVariablesOverride preservation on the started batch, StopBuildBatch and DeleteBuildBatch. A caller cannot distinguish "this project may not be batch-built" from "your request was malformed", and retry logic keyed on InvalidInputException loops forever on a request AWS would have accepted. The behavioral check this is grounded in dies on the very first StartBuildBatch, so none of its representation assertions (empty, unicode, PLAINTEXT and PARAMETER_STORE environment variables coming back byte-identically) is ever evaluated.

Fix: the pre-insert validation in the StartBuildBatch handler. The batch-enablement gate must be satisfied by the EFFECTIVE batch configuration — the request's buildBatchConfigOverride merged over the project's stored buildBatchConfig — and must keep answering 400 InvalidInputException with that fixed message only when NEITHER the project nor the request states a batch configuration. The timeout-window helper and the message constant live in the crate's state module and are shared with the stored-config check, so the merge must happen before either window check. The emulator-only environmentVariablesOverride ceiling declared in the same module must not refuse a count AWS accepts (the model bounds that list not at all).

Related open issue that does NOT carry this check's replay key: 20261104T221742Z-codebuild-startbuildbatch-refuses-a-batch-aws-starts-returning.md describes the same gate from the WIRE corpus (fixture codebuild-build-batch-environment-variables-preserved#setup#6, key codebuild|StartBuildBatch|2xx->4xx|status). It is grounded only in that fixture step; this entry records the behavioral check id so the behavioral replay of the same cause is gated too. A fixer closing one should re-replay both. Other open codebuild issues that do not cover this call: 20261029T204615Z-codebuild-accepts-four-refusals-aws-rejects-and-builds.md (a batch that WAS created never fanning out into sub-builds) and 20261025T062553Z-codebuild-3-of-the-6-divergence-causes-remain-open.md (batch EXECUTION needing a contained shell runtime).

Traces: behavioral check codebuild.build-batch-environment-variables-preserved.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 80/190 passed (42.1%), 102 mismatched, 8 errored, 0 skipped, bootstrap_signal false. Of the 110 failing checks, 109 are marked OWNED by an existing open issue; this is the only surviving one.

### codebuild
#### [error] codebuild.build-batch-environment-variables-preserved
  botocore.errorfactory.InvalidInputException: An error occurred (InvalidInputException) when calling the StartBuildBatch operation: BatchConfig must be configured on the project to use the BuildBatch feature

The check had already, successfully: put an SSM String parameter, created an IAM role trusting codebuild.amazonaws.com with an inline logs:* + ssm:GetParameters policy, and created a CodeBuild project (NO_SOURCE, inline buildspec carrying a `batch: build-list: - identifier: build1` section, LINUX_CONTAINER / BUILD_GENERAL1_SMALL, serviceRole=<role arn>) — with no buildBatchConfig, because AWS does not require one when the start call supplies buildBatchConfigOverride. It then called:

  start_build_batch(projectName=<project>, buildspecOverride=<batch buildspec>,
                    environmentVariablesOverride=[{EMPTY_VAR,'',PLAINTEXT},
                                                  {UNICODE_VAR,'héllo-世界-🚀',PLAINTEXT},
                                                  {PARAM_VAR,<param name>,PARAMETER_STORE}],
                    buildBatchConfigOverride={'serviceRole': <role arn>})

Expected (recorded AWS): HTTP 200 with buildBatch.id, whose environment.environmentVariables carries the three overrides byte-identically.
Actual (emulator): HTTP 400 InvalidInputException, message "BatchConfig must be configured on the project to use the BuildBatch feature" — fixed text, no interpolated values. The check errored out at the start call, so batch_get_build_batches was never reached.

Handler path in the current checkout: services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build_batch.rs:321-325 — `if project.build_batch_config.is_none() { return Err(invalid_input(crate::state::PROJECT_NOT_BATCH_ENABLED_MESSAGE.to_string())); }`, inside the READ PASS that has just cloned the stored project (lines 299-315). The request's override is consulted only at lines 281-287, and only to validate its timeout window via crate::state::build_batch_config_violation — its mere presence never satisfies the gate at 321. Lines 260-275 hold the emulator-only environmentVariablesOverride ceiling (crate::state::MAX_ENVIRONMENT_VARIABLES).

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — StartBuildBatch input (buildBatchConfigOverride -> ProjectBuildBatchConfig, environmentVariablesOverride -> EnvironmentVariables with no length constraint), output StartBuildBatchOutput{buildBatch: BuildBatch}, errors InvalidInputException / ResourceNotFoundException (client, httpError 400 defaulted). Nothing in the model conditions the operation on a stored project buildBatchConfig.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Run the behavioral suite and read the single non-owned failure: codebuild.build-batch-environment-variables-preserved.
3. Reproduce directly:
   a. iam.create_role(trust: codebuild.amazonaws.com / sts:AssumeRole) + inline policy allowing logs:CreateLogGroup, logs:CreateLogStream, logs:PutLogEvents, ssm:GetParameters.
   b. codebuild.create_project(name=P, source={type:NO_SOURCE, buildspec:<spec with a `batch: build-list:` section>}, artifacts={type:NO_ARTIFACTS}, environment={LINUX_CONTAINER, aws/codebuild/amazonlinux2-x86_64-standard:5.0, BUILD_GENERAL1_SMALL}, serviceRole=<role arn>) — deliberately WITHOUT buildBatchConfig.
   c. codebuild.start_build_batch(projectName=P, buildspecOverride=<same spec>, environmentVariablesOverride=[{name:'EMPTY_VAR',value:'',type:'PLAINTEXT'}], buildBatchConfigOverride={'serviceRole': <role arn>}).
      Expect HTTP 200 with a buildBatch; observe HTTP 400 InvalidInputException "BatchConfig must be configured on the project to use the BuildBatch feature".
4. Control A: repeat 3c with no buildBatchConfigOverride and no stored buildBatchConfig — this is the one case that must stay a 400 with that exact fixed message.
5. Control B: create_project WITH buildBatchConfig={'timeoutInMins': 60} and run the same start_build_batch — must be 200.
6. After the fix, batch_get_build_batches(ids=[<returned id>]) must report environment.environmentVariables containing the overridden name/value/type triples byte-identically (including the empty value and the unicode value), and the batch must be stoppable and deletable.
