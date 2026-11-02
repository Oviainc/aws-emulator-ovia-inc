Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-cbnv-batch-get-malformed-requests
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-cbnv-batch-get-malformed-requests codebuild-cbnv-batch-get-name-case-and-duplicates codebuild-cbnv-list-projects-enum-casing-then-describe codebuild-cbnv-reference-batch-get-by-name codebuild-cbnv-stale-project-after-delete
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*].cache
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*].encryptionKey
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*].environment.environmentVariables
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*].environment.imagePullCredentialsType
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*].environment.privilegedMode
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*].queuedTimeoutInMinutes
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*].source.insecureSsl
Issue-Related: 20261102T084353Z-codebuild-does-not-resolve-or-validate-arn-addressed.md
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# BatchGetProjects omits default project members and duplicates requested names (2 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetProjects

## Description
TWO net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006) on the successful (HTTP 200) BatchGetProjects body. The open issue 20261029T175929Z-codebuild-response-bodies-omit-modeled-members-and-lose.md covers omitted members on OTHER operations (BatchGetBuildBatches, BatchDeleteBuilds, DeleteProject, GetResourcePolicy) and carries no replay key for the BatchGetProjects steps below, so both causes must be closed and these fixtures replayed. BOTH must be closed before this issue is resolved.

Cause 1 of 2: the Project structure returned by BatchGetProjects omits every member the service defaults rather than the client supplying.
AWS: Smithy BatchGetProjects -> projects (list of Project). CreateProject resolves and PERSISTS service defaults, and every subsequent read returns them, so a project created with only source/artifacts/environment/serviceRole still comes back carrying cache ({type: 'NO_CACHE'}), encryptionKey (the account's managed S3 key alias, 'arn:aws:kms:<region>:<account>:alias/aws/s3'), environment.environmentVariables (an empty list when none were given), environment.imagePullCredentialsType ('CODEBUILD'), environment.privilegedMode (false), queuedTimeoutInMinutes (the default queue timeout, 480) and source.insecureSsl (false). The member names and the default values are the requirement; the region and account in the key ARN come from the caller.
Emulator: HTTP 200 whose projects[0] omits all seven of those members - only what the CreateProject call explicitly set is echoed back.
Why: a client cannot read back a project's effective configuration. SDK models these as optional, so callers see null/None and either crash on unwrap or re-submit UpdateProject without them; configuration-drift and infrastructure-import tooling that diffs the returned Project against its template reports every defaulted field as missing.
Fix: the project state written by the CreateProject handler and the Project projection serialized by the BatchGetProjects handler - resolve service defaults at create time and serialize them on read.
Traces: codebuild-cbnv-batch-get-malformed-requests#task#8 (same cluster also covers codebuild-cbnv-batch-get-name-case-and-duplicates#task#5 and #task#6, codebuild-cbnv-list-projects-enum-casing-then-describe#task#6, codebuild-cbnv-reference-batch-get-by-name#task#3 and codebuild-cbnv-stale-project-after-delete#task#3).

Cause 2 of 2: a names list containing the same project twice yields two identical entries in `projects`.
AWS: BatchGetProjects(names @required ProjectNames @length 1-100) resolves the requested identifiers as a SET - a names list that repeats one project name returns exactly ONE Project entry for it; `projects` is not index-aligned with `names`.
Emulator: HTTP 200 with the project serialized TWICE - projects[0] and projects[1] are the same project (same name, same arn), one entry per requested name.
Why: a caller that batches identifiers from several sources (so duplicates are normal) gets a projects list longer than the distinct set it asked about; code that indexes or counts the result double-counts the project, and keying a map by name silently overwrites while any length assertion fails.
Fix: the BatchGetProjects handler's lookup loop - de-duplicate the resolved identifiers before building the projects list.
Traces: codebuild-cbnv-batch-get-name-case-and-duplicates#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (165 steps, 110 passed, 55 mismatched, score 66.7%). Neither cluster is owned by an open issue or blocked by a stub.

Cause 1 - codebuild-cbnv-batch-get-malformed-requests#task#8 BatchGetProjects, 200/200: $.projects[0].cache missing (expected {'type': 'NO_CACHE'}); $.projects[0].encryptionKey missing (expected 'arn:aws:kms:<region>:<account>:alias/aws/s3'); $.projects[0].environment.environmentVariables missing (expected []); $.projects[0].environment.imagePullCredentialsType missing (expected 'CODEBUILD'); $.projects[0].environment.privilegedMode missing (expected false); $.projects[0].queuedTimeoutInMinutes missing (expected 480); $.projects[0].source.insecureSsl missing (expected false). The identical cluster recurs on five further steps: codebuild-cbnv-batch-get-name-case-and-duplicates#task#5 and #task#6, codebuild-cbnv-list-projects-enum-casing-then-describe#task#6, codebuild-cbnv-reference-batch-get-by-name#task#3, codebuild-cbnv-stale-project-after-delete#task#3.
Cause 2 - codebuild-cbnv-batch-get-name-case-and-duplicates#task#4 BatchGetProjects, 200/200: $.projects[1] extra - the emulator returned a second, identical Project entry (same name and same 'arn:aws:codebuild:<region>:<account>:project/<name>') for the repeated name, where the recording has only projects[0]. (The same step also shows the Cause 1 omissions.)

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json - BatchGetProjects(names @required ProjectNames @length 1-100 -> projects: list of Project, projectsNotFound); Project members cache (ProjectCache), encryptionKey (NonEmptyString; doc: "If you don't specify a value, CodeBuild uses the managed CMK for Amazon S3"), queuedTimeoutInMinutes (TimeOut, range 5-480), source (ProjectSource with insecureSsl WrapperBoolean), environment (ProjectEnvironment with environmentVariables, privilegedMode - doc: "The default setting is false" - and imagePullCredentialsType ImagePullCredentialsType [CODEBUILD, SERVICE_ROLE]). The concrete default values are recorded behavior.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus) and read the codebuild divergences, or reproduce each cause directly:
3. Cause 1: CreateProject(name=<p>, source={type: NO_SOURCE, buildspec: <inline>}, artifacts={type: NO_ARTIFACTS}, environment={type: LINUX_CONTAINER, image: aws/codebuild/amazonlinux2-x86_64-standard:5.0, computeType: BUILD_GENERAL1_SMALL}, serviceRole=<role arn>), then BatchGetProjects(names=[<p>]) - expect projects[0] to carry cache, encryptionKey, queuedTimeoutInMinutes, source.insecureSsl and environment.{environmentVariables, privilegedMode, imagePullCredentialsType}; observe all seven absent.
4. Cause 2: BatchGetProjects(names=[<p>, <p>]) for that same project - expect exactly one entry in projects; observe two identical entries.
