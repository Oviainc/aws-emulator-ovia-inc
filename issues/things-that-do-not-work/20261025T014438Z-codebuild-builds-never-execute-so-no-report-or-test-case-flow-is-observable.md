Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.codebuild-junit-report-ingestion-reports-each-test-case-with-its-exact-status codebuild.describe-test-cases-absent-report-not-found codebuild.e2e-webhook-triggered-build-retry-flow codebuild.list-reports-for-report-group-sort-order-consistent codebuild.report-group-trend-reflects-created-reports
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.codebuild-junit-report-ingestion-reports-each-test-case-with-its-exact-status|run#12|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.describe-test-cases-absent-report-not-found|_create_real_report#2|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.e2e-webhook-triggered-build-retry-flow|run#21|AssertionError|assert build1_final["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.list-reports-for-report-group-sort-order-consistent|run#18|AssertionError|assert status_1 == "SUCCEEDED", f"build 1 did not succeed: {status_1}"
Issue-Key: codebuild.report-group-trend-reflects-created-reports|run#15|AssertionError|assert final_build["buildStatus"] == "SUCCEEDED", (
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CodeBuild: builds never execute, so no report, test-case or report-group-trend flow can be observed

> _Note: this report was split out of
> `20261025T014438Z-codebuild-8-further-divergences-getresourcepolicy-updatewebhook-deletebuildbatch-stubs.md`
> (its cause 1 of 8) when the other seven causes of that bundle were closed. It carries that
> cause's five replay keys so they stay replayable; nothing else of that bundle is open._

## Operation
StartBuild

## Description
AWS: Smithy `StartBuild` returns a `Build`; `StatusType` enumerates SUCCEEDED, FAILED, FAULT,
TIMED_OUT, IN_PROGRESS, STOPPED and `BuildPhaseType` enumerates SUBMITTED, QUEUED, PROVISIONING,
DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED. A
build whose buildspec commands all exit 0 reaches `buildStatus` SUCCEEDED; a build whose buildspec
declares a JUnit `reports` section mints a real report whose ARN is readable through
`BatchGetReports` / `DescribeTestCases` / `ListReportsForReportGroup` / `GetReportGroupTrend`; a
webhook-triggered build and its `RetryBuild` reach a terminal outcome the same way.

Emulator: every build walks SUBMITTED(SUCCEEDED) -> QUEUED(SUCCEEDED) -> PROVISIONING(FAULT) ->
COMPLETED, `buildStatus` FAULT, with the PROVISIONING phase carrying the context message 'this
emulator does not run CodeBuild build containers, so the build could not be provisioned and no
buildspec was executed'. No report, test case, trend data point or artifact is ever produced.

Why: a client cannot tell a passing build from a failing one, and every downstream reporting
surface (reports, test cases, report-group trends) is permanently empty while the control plane
claims the build ran and completed.

## What this needs, and why it is not a code fix today
A CodeBuild buildspec is a shell program run inside a build container. This platform provides no
container runtime and no external process, and the project's guidance for running caller-supplied
code lists engines only for JavaScript and for a native library shipped inside the caller's own
package — a runtime it has no engine for keeps the operation's modelled failure for a request it
cannot run, and a result must not be synthesised. Reporting `buildStatus` SUCCEEDED, phase
outcomes, durations and JUnit test-case statuses for a buildspec that nothing executed — or minting
reports whose contents were never produced — would be an outcome no engine produced, so the
modelled FAULT state with the reason above is the honest answer until a build-execution engine
exists here.

So this report stays open as the record of the divergence and of its five replay keys. It is
resolvable in two ways: an in-process engine that really runs a buildspec's commands (and then the
phase walk, the exit codes, the artifacts and the JUnit ingestion all follow from what ran), or a
decision that the emulator will not run builds, which closes it as a platform blocker.

The same defect, from the other corpus, is
`20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md` (its cause 1); that
report carries none of the five keys above, which is why they live here.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from the commit above:
expected `buildStatus` SUCCEEDED/FAILED, got 'FAULT' with phases
[('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)]:
'every buildspec command exited zero, so the build must be SUCCEEDED even though a test case
failed; got FAULT' (junit ingestion); 'prerequisite build must succeed to mint a real report, got
FAULT' (describe-test-cases); 'expected the original build to succeed, got FAULT' (webhook retry);
'build 1 did not succeed: FAULT' (list-reports ordering); 'build did not succeed: FAULT'
(report-group trend). The PROVISIONING phase carries contexts [{'message': 'this emulator does not
run CodeBuild build containers, so the build could not be provisioned and no buildspec was
executed'}].

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. `CreateProject` with a `NO_SOURCE` inline buildspec that echoes and exits 0 — optionally with a
   `reports` section pointing at a JUnit XML it writes.
3. `StartBuild`, then `BatchGetBuilds` until `buildComplete` — observe `buildStatus` FAULT at
   PROVISIONING.
4. `BatchGetReports` / `ListReportsForReportGroup` / `DescribeTestCases` / `GetReportGroupTrend` —
   observe that no report was ever minted.
