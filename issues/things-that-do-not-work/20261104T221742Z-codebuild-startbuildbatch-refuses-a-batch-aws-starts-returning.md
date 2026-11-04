Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-build-batch-environment-variables-preserved
Issue-Step: setup#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-build-batch-environment-variables-preserved
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|StartBuildBatch|2xx->4xx|status
Issue-Commit: 40434d85add499d02eca6ac15c85c4cfa7b8d96f

# CodeBuild StartBuildBatch refuses a batch AWS starts, returning 400 instead of 200 with a buildBatch

> _Note: this issue was identified against the code base at commit `40434d85add499d02eca6ac15c85c4cfa7b8d96f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuildBatch

## Description
ONE root cause in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006): StartBuildBatch REFUSES a request real CodeBuild accepts, answering HTTP 400 where the recording has HTTP 200 and a BuildBatch. Because the refused call is a SETUP step of the fixture, no batch build is ever minted and nothing the fixture goes on to check (that a batch build preserves the environment variables it was started with) can be observed at all.

Cause 1 of 1: StartBuildBatch answers 400 for a project/request pair AWS starts a batch for.
AWS: Smithy StartBuildBatch(projectName @required NonEmptyString @length min 1, buildBatchConfigOverride ProjectBuildBatchConfig, environmentVariablesOverride EnvironmentVariables, ...) -> StartBuildBatchOutput{buildBatch: BuildBatch}; the only modeled errors are InvalidInputException and ResourceNotFoundException, both client, httpError 400 (defaulted). The model puts NO @length on EnvironmentVariables and no @range on ProjectBuildBatchConfig$timeoutInMins, so neither member is bounded by the modeled constraint walk. Recorded behavior for this step: HTTP 200 carrying a BuildBatch for the named, existing project -- i.e. AWS treats the request as a well-formed start of a batch build and mints the batch.
Emulator: HTTP 400 for the same request, so no BuildBatch is returned or persisted. The replay records only the status for this cluster ('status: expected 200, got 400'), so the error code and message the emulator sent are not in the replay output; what IS fixed is that a 400 on this operation can only be one of the handler's two modeled errors, and the only refusals the handler can reach before it inserts the batch are: (a) the batch-enablement gate, InvalidInputException 'BatchConfig must be configured on the project to use the BuildBatch feature' -- wholly fixed text, nothing interpolated -- raised when the STORED project carries no buildBatchConfig; (b) InvalidInputException 'TimeoutInMins must be between 5 and 2160' -- fixed text with both bounds read from the window -- for a ProjectBuildBatchConfig timeoutInMins outside 5..2160, applied to BOTH the request's buildBatchConfigOverride and the project's stored buildBatchConfig; (c) InvalidInputException "Value at 'environmentVariablesOverride' failed to satisfy constraint: Member must have length less than or equal to 1000", an emulator-only ceiling the model does not state; (d) ResourceNotFoundException 'Project cannot be found: <project ARN>' (fixed text 'Project cannot be found: ' plus the ARN built from the request's projectName and the caller's region).
Hypothesis: the reachable one here is (a). The gate is written as a test of the STORED project alone -- it refuses whenever project.buildBatchConfig is absent and never considers the buildBatchConfigOverride the same request may carry, even though the handler validates that override's timeout a few lines earlier. AWS resolves the batch configuration for the started batch from the request's override on top of the project's, so a project with no stored buildBatchConfig is still startable as a batch when the request supplies one, and the fixed-text 'BatchConfig must be configured...' refusal belongs only to the case where NEITHER the project nor the request states a batch configuration. The same gate also runs ahead of idempotency-token resolution, so a repeat of an already-accepted batch start is refused on the same ground.
Why: a client that starts a batch build the way AWS allows is told its input is invalid and gets no buildBatch id, so the whole batch surface downstream of the start -- BatchGetBuildBatches, ListBuildBatches/ListBuildBatchesForProject, environmentVariablesOverride preservation, DeleteBuildBatch -- is unreachable. A caller cannot distinguish 'this project may not be batch-built' from 'your request was wrong', and retry/error-handling code keyed on InvalidInputException loops on a request AWS would have accepted.
Fix: the StartBuildBatch handler's pre-insert validation in services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build_batch.rs -- the batch-enablement gate must be satisfied by the effective batch configuration (the request's buildBatchConfigOverride merged over the project's stored buildBatchConfig) rather than by the stored project alone, and must stay a 400 InvalidInputException with that fixed message only when neither source states one. The timeout window helper and the message constants live in src/state.rs (build_batch_config_violation, PROJECT_NOT_BATCH_ENABLED_MESSAGE); the emulator-only environment-variable ceiling MAX_ENVIRONMENT_VARIABLES is also declared there and must not refuse a count AWS accepts.
Traces: codebuild-build-batch-environment-variables-preserved#setup#6.

Related open issues that do NOT cover this step (none of them carries this replay key): 20261029T204615Z-codebuild-accepts-four-refusals-aws-rejects-and-builds.md (its cause 6 is about a started batch never fanning out into sub-builds, i.e. a batch that WAS created; it is grounded in behavioral check ids only), 20261025T062553Z-codebuild-3-of-the-6-divergence-causes-remain-open.md (batch builds needing a contained shell runtime, i.e. batch EXECUTION) and 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md (the opposite polarity: refusals AWS issues that the emulator accepts). This divergence is the start call itself being refused.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 165 steps, 125 passed, 37 mismatched, 3 blocked, score 75.8%; emulator_up true (/__admin/health 200, data listener accepting), bootstrap_signal false. Twenty-one divergence clusters were reported; twenty are owned by existing open issues, and this is the one net-new cluster (empty owned_by, empty stub_path).

Cluster: service=codebuild | StartBuildBatch | status=200->400 | keys=status
  fixture_id: codebuild-build-batch-environment-variables-preserved
  step_key:   setup#6
  verdict:    mismatch
  expected_status: 200 (recorded AWS: StartBuildBatchOutput with a buildBatch)
  actual_status:   400
  detail:     "status: expected 200, got 400"
  affected:   codebuild-build-batch-environment-variables-preserved#setup#6 (affected_count 1)

The replay detail for this cluster is the status alone -- the emulator's error code and message are NOT part of the recorded evidence, so they are not quoted here as observed. The candidate 400 paths were read out of the handler instead: services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build_batch.rs lines 260-275 (environmentVariablesOverride ceiling, emulator-only), 281-287 (buildBatchConfigOverride timeout window), 299-315 (project not found -> ResourceNotFoundException 'Project cannot be found: <arn>'), 321-325 (project.build_batch_config.is_none() -> InvalidInputException crate::state::PROJECT_NOT_BATCH_ENABLED_MESSAGE), 329-335 (stored buildBatchConfig timeout window), 549-560 (idempotency-token fingerprint mismatch). Supporting constants: src/state.rs:383 MAX_ENVIRONMENT_VARIABLES = 1000 (its own doc comment says "THIS CEILING IS THE EMULATOR'S, NOT AWS'S ... so a request real CodeBuild would accept is refused here"), src/state.rs:3909-3913 BATCH_TIMEOUT_MIN_MINUTES = 5 / BATCH_TIMEOUT_MAX_MINUTES = 2160, src/state.rs:3924-3939 build_batch_config_violation -> "TimeoutInMins must be between 5 and 2160", src/state.rs:3951-3952 PROJECT_NOT_BATCH_ENABLED_MESSAGE = "BatchConfig must be configured on the project to use the BuildBatch feature". src/ops/create_project.rs:339 confirms CreateProject does persist input.build_batch_config, so a project created WITH a batch config is not losing it in state.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json -- StartBuildBatch input/output signature and its two modeled errors (InvalidInputException, ResourceNotFoundException; client, httpError 400 defaulted), plus shapes EnvironmentVariable/EnvironmentVariables (no @length on the list) and ProjectBuildBatchConfig (timeoutInMins targets the unconstrained WrapperInt).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and read the codebuild cluster 'service=codebuild|StartBuildBatch|status=200->400|keys=status'.
3. Replay fixture codebuild-build-batch-environment-variables-preserved up to step setup#6: the step issues StartBuildBatch for the project the fixture's earlier setup steps created and expects HTTP 200 with a buildBatch; the emulator answers HTTP 400 and the fixture's remaining environment-variable assertions never run.
4. Reproduce directly: CreateProject for a NO_SOURCE project with an inline buildspec and NO buildBatchConfig, then StartBuildBatch(projectName=<it>, environmentVariablesOverride=[{name:'K', value:'v', type:'PLAINTEXT'}]) -- expect HTTP 200 with a BuildBatch whose environment.environmentVariables carries the override; observe HTTP 400.
5. Repeat step 4 with a buildBatchConfigOverride={'timeoutInMins': 60} on the StartBuildBatch call -- AWS starts the batch; the emulator still refuses because the gate at src/ops/start_build_batch.rs:321 tests only the stored project's buildBatchConfig.
6. Control: CreateProject WITH buildBatchConfig={'timeoutInMins': 60} and run the same StartBuildBatch -- this must also be a 200, and the 'BatchConfig must be configured on the project to use the BuildBatch feature' refusal must remain only for the case where neither the project nor the request states a batch configuration.
7. Confirm with BatchGetBuildBatches(ids=[<returned batch id>]) that the started batch reports the environment variables the start call overrode.
