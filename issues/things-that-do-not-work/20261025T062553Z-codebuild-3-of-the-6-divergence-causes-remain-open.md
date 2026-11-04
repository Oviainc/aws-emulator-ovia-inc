Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.e2e-build-batch-retry-and-cleanup-flow codebuild.command-execution-lifecycle-in-sandbox codebuild.command-execution-lifecycle-reaches-terminal-state codebuild.e2e-sandbox-command-execution-after-stop-rejected codebuild.sandbox-lifecycle-active-to-terminated codebuild.sandbox-lifecycle-transitions codebuild.deploy-fleet-list-reflects-fleet
Issue-Key: codebuild.e2e-build-batch-retry-and-cleanup-flow|_poll_batch_status#9|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.command-execution-lifecycle-in-sandbox|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.command-execution-lifecycle-reaches-terminal-state|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.e2e-sandbox-command-execution-after-stop-rejected|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.sandbox-lifecycle-active-to-terminated|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.sandbox-lifecycle-transitions|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: codebuild|CreateStack|ValidationError
Issue-Commit: cfbb9a1e24eec7d8c98af7d6e9e62a0e8013f018

# CodeBuild: 3 of the 6 divergence causes are still open — batch builds and sandbox command executions both need a contained shell runtime, and AWS::CodeBuild::Fleet needs the CloudFormation provisioner split first

> _Split out of `issues/things-that-now-work/20261025T062553Z-codebuild-6-further-divergences-startbuildbatch-takes-no-input.md`, whose causes 1, 2 and 6 are fixed. This report carries the replay keys that do NOT pass, re-filed here so they are not recorded as working. Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._

## Operation
StartCommandExecution

## Description
Three of the parent report's six causes are untouched, and each is BLOCKED rather than
unimplemented: two of them on one capability this emulator does not have, and the third
on a file the merge gate refuses to let a fix edit. What the parent fix DID land, so a
later fixer does not redo it: `StartBuildBatch` now refuses a project that carries no
`buildBatchConfig` and a `ProjectBuildBatchConfig` whose `timeoutInMins` is outside
5..2160 (shared as `crate::state::build_batch_config_violation`, applied by
`CreateProject`, `UpdateProject` and `StartBuildBatch`), `PutResourcePolicy` enforces the
per-kind read-only action allowlist, and `StartCommandExecution`'s refusal for a sandbox
whose session is not live states CodeBuild's own sentence ("Command execution cannot be
started if sandbox status is STOPPED", with the status read off the stored record) — so
the negative half of the sandbox checks is in place and waiting only for the positive
control in cause 2 below.

Cause 1 of 3: batch builds never execute — every batch ends `FAULT` at
`DOWNLOAD_BATCHSPEC`, so no batch reaches a terminal status a caller can act on and
`RetryBuildBatch` / `StopBuildBatch` / `DeleteBuildBatch` are never exercised on one.
AWS: a batch build's status is the aggregate of the builds its batch specification
declares, and `FAULT` is reserved for a failure of the build system itself — so a batch
whose constituent build exits non-zero reaches `buildBatchStatus` `FAILED`;
`RetryBuildBatch` then restarts that same batch id in place, `StopBuildBatch` moves the
retried attempt to `STOPPED` (or `FAILED`), and once the batch is complete
`DeleteBuildBatch` removes it (`statusCode` `BATCH_DELETED`), after which
`BatchGetBuildBatches` reports the id in `buildBatchesNotFound` and `RetryBuildBatch`
answers 400 `ResourceNotFoundException`.
Emulator: `StoredBuildBatch::settle`
(`services/service-com-amazonaws-codebuild-codebuild-20161006/src/state.rs`) ends every
batch in `FAULT` carrying `BATCH_SPEC_NOT_DOWNLOADED_MESSAGE`, which is in neither
terminal set the check polls, so the poll cannot settle.
Why it is BLOCKED, and on what: expanding the buildspec's `batch:` section into build
groups needs no container — the buildspec is stored inline on the project and is plain
YAML — but the status the suite grades (`FAILED`, because a constituent build's command
exits 1) is the OUTCOME OF RUNNING that build's buildspec commands. Reporting `FAILED`
without anything having run would be a status for work nothing performed, and aggregating
today's `FAULT` builds still yields `FAULT`. It is therefore the same single missing
capability cause 2 names.
Traces: codebuild.e2e-build-batch-retry-and-cleanup-flow.

Cause 2 of 3: sandbox command executions never reach `SUCCEEDED` or `FAILED` — every
execution ends `FAULT` with no `exitCode` and no `standardOutputContent`.
AWS: a command run in a `RUNNING` sandbox reaches a terminal status that reflects the
process: `echo <marker>` -> `SUCCEEDED`, `exitCode` '0', the marker present in
`standardOutputContent`; `exit 7` -> `FAILED`, `exitCode` '7'.
Emulator: `StoredCommandExecution::settle` (same `src/state.rs`) ends every execution in
`COMMAND_EXECUTION_STATUS_FAULT` carrying `COMMAND_NOT_EXECUTED_MESSAGE`, with no exit
code and no output, so the five checks spin on `BatchGetCommandExecutions` until their
budget is gone and never reach their own negative assertions.
Why it is BLOCKED, and on what: the capability both this and cause 1 need is RUNNING a
caller-supplied shell command line in the build image and reporting its real exit code
and output. Served faithfully that needs the image's shell and filesystem — a runtime
this platform does not provide ("Containers and external processes are not available to
a generated emulator yet", `vendor/engine/src/lib.rs:13-15`). What was checked and ruled
out by the parent fix:
* `brush-core` 0.5.0 (`cargo info brush-core`: MIT, rust-version 1.88.0, dependencies
  include `command-fds`, `nix`, `uzers`, `check_elevation`) is the only maintained
  in-process POSIX/bash shell on the registry, and it is a FULL shell: it resolves and
  executes external commands as host processes against the host filesystem. That breaks
  containment (no process spawn and no host filesystem on request data) and is unfaithful
  anyway — the host is not the CodeBuild image — and it exposes no seam that disables
  external execution.
* writing an evaluator for a shell subset instead is the forbidden hand-written language
  subset: a subset sized to `echo` and `exit` answers every other command wrongly while
  reporting success.
So the honest exit for this cause (and cause 1 with it) is
`mark_wont_fix(platform_blocker)` naming `vendor/engine/src/lib.rs:13-15`, UNLESS a
contained in-process shell engine appears — a shell crate with a seam for external
command resolution, or a WASI runtime plus a shell image the tree may ship. A fixer that
finds one implements it once, in a shared place, and both causes follow from it: the
command execution reports the process's own status, exit code and output, and a batch
build's constituent builds then have an outcome for the batch status to aggregate.
Traces: codebuild.command-execution-lifecycle-in-sandbox,
codebuild.command-execution-lifecycle-reaches-terminal-state,
codebuild.e2e-sandbox-command-execution-after-stop-rejected,
codebuild.sandbox-lifecycle-active-to-terminated,
codebuild.sandbox-lifecycle-transitions.

Cause 3 of 3: CloudFormation does not know the `AWS::CodeBuild::Fleet` resource type and
refuses the template outright.
AWS: a `CreateStack` whose template declares an `AWS::CodeBuild::Fleet` resource is
accepted; the stack reaches `CREATE_COMPLETE` and the fleet then exists in CodeBuild, so
`ListFleets` / `BatchGetFleets` return its ARN.
Emulator: `CreateStack` is refused with HTTP 400 `ValidationError`, "Template format
error: Unrecognized resource types: [AWS::CodeBuild::Fleet]" — the stack is never created
at all. The type is absent from the recognised-type list
(`services/service-com-amazonaws-cloudformation-cloudformation/src/state/resource_types.rs:334-336`,
which lists `AWS::CodeBuild::Project`, `::ReportGroup` and `::SourceCredential`).
Why it is BLOCKED, and on what: a recognised type must be PROVISIONED by calling the
owning service's own create — registering it in `resource_types.rs` alone would let the
stack reach `CREATE_COMPLETE` for a fleet CodeBuild does not hold, which is the
"resource made to look provisioned" antipattern and the stale symptom this cause already
superseded once. Type ownership, the `Provisioner` enum and the create/delete/describe
dispatch all live in ONE file —
`services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs`,
28,667 lines (`provisioner_for` at :810; the `AWS::CodeBuild::ReportGroup` arms at :835,
:1311, :2260 and :2613; its provider at :23431) — and `state/provisioning.rs` only CALLS
`provisioner_for`, so there is no other seam. The merge gate refuses a fix that touches a
file longer than 5,000 lines, so the prerequisite is its own change: split
`provisioner.rs` into a module directory (one file per service family, behaviour
unchanged), and only then add the `AWS::CodeBuild::Fleet` entry plus a provider that
calls CodeBuild's own `CreateFleet`/`DeleteFleet`/`BatchGetFleets` the way
`create_codebuild_report_group` (:23431) calls `CreateReportGroup`.
Traces: codebuild.deploy-fleet-list-reflects-fleet.

## Evidence
Replay of the parent issue's 10 units after its fix: 3 passed (its causes 1, 2 and 6 —
`start-build-batch-without-batch-config-rejected`,
`start-build-batch-malformed-input-rejected`,
`put-resource-policy-write-action-rejected`), 7 failed, and the 7 keys above are still
open. `other_causes` was empty: the parent fix caused none of these.
* Causes 1 and 2: `Check timed out after 60.0s` — the batch poll waits on
  `{'FAILED','STOPPED','SUCCEEDED'}` and the execution poll on `{'SUCCEEDED','FAILED'}`,
  and the emulator settles both to `FAULT`.
* Cause 3: `botocore.exceptions.ClientError: An error occurred (ValidationError) when
  calling the CreateStack operation: Template format error: Unrecognized resource types:
  [AWS::CodeBuild::Fleet]`.

## Steps to Reproduce
1. Boot the emulator and replay the check ids in Issue-Trace-Ids.
2. Cause 1: `CreateProject` with a `buildBatchConfig` and a buildspec whose `batch:`
   section runs a command that exits 1, `StartBuildBatch`, then poll
   `BatchGetBuildBatches` — observe `buildBatchStatus` `FAULT` at currentPhase `FAILED`
   instead of `FAILED`, so `RetryBuildBatch` / `StopBuildBatch` / `DeleteBuildBatch` are
   never exercised on a terminal batch.
3. Cause 2: `StartSandbox` on a NO_SOURCE project, poll `BatchGetSandboxes` until
   `RUNNING`, then `StartCommandExecution(sandboxId=..., command='echo marker',
   type='SHELL')` and poll `BatchGetCommandExecutions` — observe `FAULT` with no
   `exitCode` and no `standardOutputContent` instead of `SUCCEEDED` / '0' / the marker.
   Repeat with `command='exit 7'` (expect `FAILED` / '7'). The `StopSandbox` refusal that
   follows is already served with AWS's own sentence.
4. Cause 3: `CreateStack` with a template whose only resource is `AWS::CodeBuild::Fleet`
   (baseCapacity 1, environmentType LINUX_CONTAINER, computeType BUILD_GENERAL1_SMALL) —
   observe 400 `ValidationError`; expect `CREATE_COMPLETE` and the fleet ARN in
   `ListFleets`.
