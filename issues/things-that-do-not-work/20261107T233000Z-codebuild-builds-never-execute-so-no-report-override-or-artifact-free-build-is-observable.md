Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.report-group-trend-reflects-report-data codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only codebuild.start-build-artifacts-override-rejected-without-a-location-and-no-artifacts-uploads-nothing
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.report-group-trend-reflects-report-data|run#12|AssertionError|assert build["buildStatus"] == "SUCCEEDED", f"build did not succeed: {build}"
Issue-Key: codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only|run#12|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.start-build-artifacts-override-rejected-without-a-location-and-no-artifacts-uploads-nothing|run#19|AssertionError|assert done_silent["buildStatus"] == "SUCCEEDED", (
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodeBuild builds never execute, so no report, no per-build override and no artifact-free build is observable (1 cause)

> _Split out of `issues/things-that-now-work/20261029T204615Z-codebuild-accepts-four-refusals-aws-rejects-and-builds.md` when that report's other seven causes were fixed: this is its cause 8, which is NOT fixed, so the three replay units that still fail on it are filed here rather than recorded as working. The code base can have drifted since the commit above._

## Operation
StartBuild

## Description
ONE root cause in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006): a build never executes its buildspec, so it never reaches a successful terminal status and nothing a running build would produce is observable.

AWS: `StartBuild` -> {build}; `Build$phases` carries SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE ... COMPLETED, and `Build$buildStatus` reaches `SUCCEEDED` for a buildspec whose commands exit 0 (and `FAILED` when a command or a declared artifact fails). Recorded behaviour: the PROVISIONING phase SUCCEEDS and the buildspec executes, so the reports a build's buildspec declares are listed for their report group and `GetReportGroupTrend` derives a value from them, and `StartBuild`'s `imageOverride` / `computeTypeOverride` take effect in the build that ran — and only in that build.

Emulator: every build ends `buildStatus` `FAULT` with phases [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT'), ('COMPLETED', None)], the PROVISIONING phase carrying the context message that states this emulator runs no build container, so nothing was provisioned and no buildspec was executed.

Why it matters: no build-produced state is ever observable. `GetReportGroupTrend` has no reports to derive a trend from, a per-build image/compute override can never be shown to apply to that build only, and a caller that starts a build on a project with no artifacts cannot observe the successful, upload-free build AWS gives it.

## What this needs
A build EXECUTION component: a CodeBuild build runs caller-supplied shell commands inside a build container, and this emulator has no container runtime and no external-process runtime to run them in, so there is nothing to execute a buildspec with. The honest terminal state is the one the emulator reports today — reporting `SUCCEEDED` for a buildspec nothing ran would be a status for work no engine performed — so closing this needs the execution capability itself, in-process and contained (a shell/command engine whose filesystem, process and network reach is confined, bounded by a wall-clock timeout, a step budget and an output cap), after which the build state machine can advance PROVISIONING and the phases after it from what actually ran.

The same execution gap is described by `issues/things-that-do-not-work/20261025T014438Z-codebuild-builds-never-execute-so-no-report-or-test-case-flow-is-observable.md` and `issues/things-that-do-not-work/20261102T124528Z-codebuild-3-net-new-divergences-builds-fault-at.md`; this report exists so the three replay units below are not recorded as working while it is open.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built with the other seven causes of the parent report fixed: 6 of its 9 units pass; these three fail, each on the build's terminal status.

- `codebuild.report-group-trend-reflects-report-data`: "build did not succeed", `buildStatus` `FAULT`, phases SUBMITTED/SUCCEEDED, QUEUED/SUCCEEDED, PROVISIONING/FAULT, COMPLETED.
- `codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only`: "the overridden build ended 'FAULT'; phases [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT'), ('COMPLETED', None)]".
- `codebuild.start-build-artifacts-override-rejected-without-a-location-and-no-artifacts-uploads-nothing`: the two refusal legs this check begins with now pass (an S3 `artifactsOverride` with no location is refused `InvalidInputException` 'Invalid artifacts: location is required', and one naming a bucket that does not exist 'Bucket <bucket> does not exist', with no build left behind); the check then starts a build with no artifacts and asserts `done_silent["buildStatus"] == "SUCCEEDED"`, which needs this cause.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. CreateProject on a NO_SOURCE project whose inline buildspec runs a command that exits 0.
3. StartBuild on it and poll BatchGetBuilds until `buildComplete` — expect `buildStatus` `SUCCEEDED`; observe `FAULT` with PROVISIONING/FAULT.
4. For the report leg: declare a report in the buildspec and poll ListReportsForReportGroup / GetReportGroupTrend — expect the run the build produced; observe none.
