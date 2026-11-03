Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.cbnv-project-environment-defaults-applied
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.cbnv-project-environment-defaults-applied|run#19|AssertionError|assert proj.get("artifacts") == {
Issue-Commit: f6e0cb845b411895ca6ad8a253200948fbac9f34

# codebuild: CreateProject does not derive the ProjectArtifacts defaults (name/packaging/encryptionDisabled), so BatchGetProjects returns a bare artifacts block (1 cause)

> _Note: this issue was identified against the code base at commit `f6e0cb845b411895ca6ad8a253200948fbac9f34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProject

## Description
Cause 1 of 1: CreateProject stores the caller's `artifacts` block verbatim and never fills in the server-derived artifact defaults, so every later read of the project (BatchGetProjects) reports only the member the caller sent.

AWS: When a project is created with `artifacts={"type":"CODEPIPELINE"}` (and `source={"type":"CODEPIPELINE"}`, which is the one source type CreateProject accepts with no buildspec), CodeBuild materializes the artifact configuration server-side and BatchGetProjects reports `artifacts` with four members: `type` as sent, `name` derived from the project's own name (i.e. `name` == the `name` passed to CreateProject), `packaging` defaulted to `NONE`, and `encryptionDisabled` defaulted to `false`. Smithy: `ProjectArtifacts` members `type` (ArtifactsType, required), `name` (String), `packaging` (ArtifactPackaging, documented as "NONE ... This is the default if packaging is not specified"), `encryptionDisabled` (WrapperBoolean) — the model declares no `@default` for `name`/`packaging`/`encryptionDisabled`, so the derivation itself is recorded behavior only: the recording shows AWS returning all four members for a project created with `type` alone. The sibling defaults the same check asserts (source `insecureSsl:false`, `cache {type:NO_CACHE}`, `badge {badgeEnabled:false}`, `projectVisibility PRIVATE`, `timeoutInMinutes 60`, `queuedTimeoutInMinutes 480`, the `:alias/aws/s3` encryptionKey, `environment.privilegedMode false`, `imagePullCredentialsType CODEBUILD`, `environmentVariables []`) are all already correct in the emulator — only the artifacts block is unpopulated.

Emulator: BatchGetProjects returns `artifacts={'type': 'CODEPIPELINE'}` — `name`, `packaging` and `encryptionDisabled` are absent entirely. The reply is a 200 with the correct shape otherwise; no error code or message is involved, so the divergence is purely the missing derived members. Replay detail: "AssertionError: artifacts={'type': 'CODEPIPELINE'}, expected a name CodeBuild derived from the project name '<project>'" (the project name shown is this run's generated name and is not part of the requirement; the requirement is that `artifacts.name` equals whatever name the request used).

Why: a client that reads back a project to discover where its build output will be named/packaged — CodePipeline integrations, IaC drift detection (CloudFormation/Terraform comparing desired vs actual state), and any tooling that keys off `artifacts.name` — sees a block with no name and no packaging. Drift checkers report a permanent diff because the field they expect AWS to fill is never returned, and code that reads `artifacts['name']` raises KeyError instead of getting the project name.

Fix: the CreateProject handler's artifact normalization — the derived defaults must be applied when the project record is built and persisted (so that both the CreateProject response and every later BatchGetProjects read carry them), not synthesized only in one serializer path. UpdateProject must apply the same derivation so a project updated with a bare `artifacts` block does not lose the members again.

Traces: codebuild.cbnv-project-environment-defaults-applied (mismatch).

## Evidence
Check: codebuild.cbnv-project-environment-defaults-applied

Request (CreateProject): name=<project>, source={"type":"CODEPIPELINE"}, artifacts={"type":"CODEPIPELINE"}, environment={type:LINUX_CONTAINER, image:aws/codebuild/amazonlinux2-x86_64-standard:5.0, computeType:BUILD_GENERAL1_SMALL}, serviceRole=<codebuild-assumable role arn>. No buildspec.

Then BatchGetProjects(names=[<project>]).

Expected (AWS): projects[0].artifacts == {"type": "CODEPIPELINE", "name": "<the project name passed to CreateProject>", "packaging": "NONE", "encryptionDisabled": false}

Actual (emulator): projects[0].artifacts == {"type": "CODEPIPELINE"}

Failure text from the replay:
  File "check_codebuild_cbnv_project_environment_defaults_applied.py", line 131, in run
    assert proj.get("artifacts") == {...}
  AssertionError: artifacts={'type': 'CODEPIPELINE'}, expected a name CodeBuild derived from the project name 'sim2real-cbnv-dflt-16c3da017108'

Everything asserted before this line passed on the emulator (environment echo, privilegedMode=false, imagePullCredentialsType=CODEBUILD, environmentVariables=[], timeoutInMinutes=60, queuedTimeoutInMinutes=480, encryptionKey ending in :alias/aws/s3, badge={badgeEnabled:false}, projectVisibility=PRIVATE, cache={type:NO_CACHE}, created==lastModified, source=={type:CODEPIPELINE, insecureSsl:false}), which isolates the defect to the artifacts block.

Smithy (com-amazonaws-codebuild-codebuild-20161006): ProjectArtifacts.packaging doc — "NONE: CodeBuild creates in the output bucket a folder that contains the build output. This is the default if packaging is not specified." No @default traits are modeled for name/packaging/encryptionDisabled; the four-member read-back is recorded behavior.

## Steps to Reproduce
1. Create an IAM role assumable by codebuild.amazonaws.com.
2. codebuild:CreateProject with name=<any valid project name>, source={"type":"CODEPIPELINE"}, artifacts={"type":"CODEPIPELINE"}, environment={"type":"LINUX_CONTAINER","image":"aws/codebuild/amazonlinux2-x86_64-standard:5.0","computeType":"BUILD_GENERAL1_SMALL"}, serviceRole=<that role arn>, and no buildspec.
3. codebuild:BatchGetProjects(names=[<that name>]).
4. Inspect projects[0].artifacts. Emulator returns {"type":"CODEPIPELINE"}; AWS returns {"type":"CODEPIPELINE","name":"<that name>","packaging":"NONE","encryptionDisabled":false}.
