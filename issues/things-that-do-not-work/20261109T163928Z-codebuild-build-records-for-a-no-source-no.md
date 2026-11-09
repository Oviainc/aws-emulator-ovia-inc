Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-no-source-build-exports-computed-variables
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-no-source-build-exports-computed-variables
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|StartBuild|2xx->2xx|$.build.artifacts.location
Issue-Key: codebuild|StartBuild|2xx->2xx|$.build.artifacts.type
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].artifacts.location
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].artifacts.type
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].buildStatus
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].environment.customEntrypoint
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].exportedEnvironmentVariables[name="ANSWER"]
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].logs
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*]
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*].contexts
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*].contexts[*].message
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*].contexts[*].statusCode
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*].durationInSeconds
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*].endTime
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*].phaseStatus
Issue-Key: codebuild|BatchGetBuilds|2xx->2xx|$.builds[*].phases[*].phaseType
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# CodeBuild Build records for a no-source NO_ARTIFACTS build are incomplete and the build fails instead of succeeding (2 causes)

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Two net-new root causes on the same CodeBuild surface: the Build record the emulator serves for a project with source type NO_SOURCE and artifacts type NO_ARTIFACTS. Both must be closed.

Cause 1 of 2: the Build returned by StartBuild (and read back by BatchGetBuilds) omits the `artifacts` block entirely for a NO_ARTIFACTS project.
AWS: StartBuild returns a Build whose `artifacts` member is always populated, and for a project whose artifacts type is NO_ARTIFACTS it is present with an empty `location` and the artifacts type reported as `no_artifacts` (lowercase, as CodeBuild serializes a build's artifact type on the wire). Smithy (CodeBuild): StartBuild output = { build: Build }; Build$artifacts targets BuildArtifacts { location (String), sha256sum, md5sum, overrideArtifactName, encryptionDisabled, artifactIdentifier, bucketOwnerAccess }. The model does not declare a `type` member on BuildArtifacts, so the `type` key is recorded behavior only: the recording shows the build's artifacts object carrying location "" and type "no_artifacts".
Emulator: `$.build.artifacts.location` and `$.build.artifacts.type` are both absent from the StartBuild response (status is 200, as AWS). The same two members are absent from `$.builds[0].artifacts` when the build is read back with BatchGetBuilds.
Why: a client that inspects build.artifacts to decide whether a build produced output (or to read artifacts.location before downloading it) gets a missing/None object instead of the documented empty-artifacts record, so it cannot distinguish 'this project produces no artifacts' from 'the API did not tell me'. Strongly typed SDKs deserialize artifacts as null and a direct field access raises.
Fix: the CodeBuild Build serializer / build-state construction for StartBuild and BatchGetBuilds (always emit BuildArtifacts, deriving location "" and type from the project's/override's artifacts configuration).
Traces: codebuild-no-source-build-exports-computed-variables#task#4 (StartBuild), and the same two keys in codebuild-no-source-build-exports-computed-variables#task#5 (BatchGetBuilds).

Cause 2 of 2: the build never completes its buildspec — BatchGetBuilds reports buildStatus FAILED with a command-execution error, a truncated and mislabelled phase sequence, no logs, and no exported environment variables.
AWS: a NO_SOURCE build whose buildspec is valid and whose commands succeed runs the full phase sequence (SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED) with each completed phase phaseStatus SUCCEEDED and a `contexts` list present (an empty context is reported as one entry with empty `message` and `statusCode`), ends buildStatus SUCCEEDED, carries `logs` (LogsLocation with groupName/streamName/cloudWatchLogsArn/deepLink for the group and stream CodeBuild creates during PROVISIONING), reports `environment.customEntrypoint` when the project sets one, and reports every variable the buildspec's `env/exported-variables` declares in `exportedEnvironmentVariables` as a {name, value} pair with the value the build computed. Smithy (CodeBuild): Build$buildStatus targets StatusType (SUCCEEDED | FAILED | FAULT | IN_PROGRESS | STOPPED | TIMED_OUT); Build$phases targets BuildPhases of BuildPhase { phaseType, phaseStatus, startTime, endTime, durationInSeconds, contexts }; Build$logs targets LogsLocation { groupName, streamName, deepLink, cloudWatchLogsArn, ... }; Build$exportedEnvironmentVariables targets ExportedEnvironmentVariables of ExportedEnvironmentVariable { name, value } — documented as available from the install phase onward; Build$environment targets ProjectEnvironment.
Emulator: the build ends buildStatus FAILED. Its phase list stops early (the recorded build has phases through COMPLETED; the emulator emits none of phases[6]..phases[10]), the phase at index 4 is reported as phaseType BUILD where AWS reports INSTALL, and that phase is phaseStatus FAILED with contexts[0].statusCode 'COMMAND_EXECUTION_ERROR' and contexts[0].message 'Error while executing command: export ANSWER=$((19 + 23)). Reason: exit status 127' where AWS reports phaseStatus SUCCEEDED and an empty context (message '' and statusCode ''). Several completed phases carry no `contexts` list at all, and phases[5] is missing phaseStatus/endTime/durationInSeconds. `builds[0].logs` is absent, `builds[0].environment.customEntrypoint` is absent, and `builds[0].exportedEnvironmentVariables` contains no entry for the variable the buildspec exports. Note the shell diagnostic: the emulator's command runner cannot execute an ordinary POSIX shell arithmetic export (exit status 127 = command not found), so no buildspec that computes a value can succeed.
Why: a client cannot run a build at all on this emulator — a correct buildspec reports FAILED, the phase sequence it inspects to locate the failure is wrong and incomplete, there is no log location to fetch output from, and the CodePipeline-facing contract of exported variables (the whole point of `exported-variables`) yields nothing. A caller relying on exportedEnvironmentVariables to pass a value to a later stage silently gets an empty list.
Fix: the CodeBuild build executor (buildspec command execution and its shell, the phase-sequence/phase-status recorder including `contexts`, and the terminal buildStatus) plus the Build state/serializer for `logs`, `environment.customEntrypoint` and `exportedEnvironmentVariables`. The open issue 20261109T141609Z-codebuild-codepipeline-ecr-3-net-new-causes-batchgetbuilds.md describes a similar missing-`logs` / build-never-succeeds symptom from the behavioral corpus, and several open issues describe builds that FAULT at PROVISIONING; none of them carries this fixture step's replay key, and the failure mode here is different (the build does start and fails inside a phase), so this cause is net-new.
Traces: codebuild-no-source-build-exports-computed-variables#task#5 (BatchGetBuilds).

## Evidence
Cause 1 — fixture codebuild-no-source-build-exports-computed-variables, step task#4, StartBuild, status 200 as expected:
  $.build.artifacts.location: missing (expected '', got None)
  $.build.artifacts.type: missing (expected 'no_artifacts', got None)
The same omission repeats on the read-back path at step task#5: $.builds[0].artifacts.location and $.builds[0].artifacts.type missing.

Cause 2 — fixture codebuild-no-source-build-exports-computed-variables, step task#5, BatchGetBuilds, status 200 as expected, body diverges:
  $.builds[0].buildStatus: expected 'SUCCEEDED', got 'FAILED'
  $.builds[0].phases[4].phaseType: expected 'INSTALL', got 'BUILD'
  $.builds[0].phases[4].phaseStatus: expected 'SUCCEEDED', got 'FAILED'
  $.builds[0].phases[4].contexts[0].statusCode: expected '', got 'COMMAND_EXECUTION_ERROR'
  $.builds[0].phases[4].contexts[0].message: expected '', got 'Error while executing command: export ANSWER=$((19 + 23)). Reason: exit status 127'
  $.builds[0].phases[2].contexts, $.builds[0].phases[3].contexts, $.builds[0].phases[5].contexts: missing (expected one entry with empty message and statusCode)
  $.builds[0].phases[5].phaseStatus / endTime / durationInSeconds: missing
  $.builds[0].phases[6] .. $.builds[0].phases[10]: missing (the phase sequence through COMPLETED is truncated)
  $.builds[0].logs: missing (expected a LogsLocation with groupName/streamName/cloudWatchLogsArn/deepLink for the build's log group and stream)
  $.builds[0].environment.customEntrypoint: missing (expected true)
  $.builds[0].exportedEnvironmentVariables[name="ANSWER"]: missing (expected the {name, value} pair the buildspec's exported-variables declares)
The recorded group/stream names, ARNs, timestamps and durations in that step are per-run values and are not the requirement; the requirement is that the members are present and shaped as AWS serializes them.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture codebuild-no-source-build-exports-computed-variables (steps task#4 and task#5).
3. Or by hand: CreateProject with source.type NO_SOURCE, artifacts.type NO_ARTIFACTS, a buildspec that declares env/exported-variables (e.g. exports ANSWER via a shell arithmetic expression) and an environment with a custom entrypoint; then StartBuild and inspect build.artifacts (Cause 1); then poll BatchGetBuilds on the returned build id and inspect builds[0].buildStatus, builds[0].phases (types, statuses, contexts, the full sequence through COMPLETED), builds[0].logs, builds[0].environment.customEntrypoint and builds[0].exportedEnvironmentVariables (Cause 2).
