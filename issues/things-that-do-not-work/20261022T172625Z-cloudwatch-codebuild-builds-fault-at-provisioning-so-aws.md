Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace
Issue-Key: cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace|_run_build#9|AssertionError|assert build["buildStatus"] == want, (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CloudWatch/CodeBuild: builds FAULT at PROVISIONING, so AWS/CodeBuild per-outcome metrics cannot be attributed to a succeeded or failed build

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
One root cause behind the single failing check of the `cloudwatch__codebuild` cross-service suite (not marked ALREADY FILED or OWNED).

Cause 1 of 1: a build that should succeed ends FAULT at PROVISIONING, so the per-outcome metric split in the AWS/CodeBuild namespace is unreachable.
AWS: a build walks the Smithy BuildPhaseType sequence (SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED) and ends SUCCEEDED on exit 0 or FAILED on a non-zero command (Smithy StatusType); CodeBuild then accounts for the outcome in the AWS/CodeBuild namespace, where Builds / SucceededBuilds / FailedBuilds (Unit Count, statistic Sum) are dimensioned by ProjectName and Duration tracks the build's own window (recorded behavior only for the metric cadence — the check records its own clock rather than pinning a literal).
Emulator: both the succeeding and the failing build end buildStatus FAULT with phases [SUBMITTED SUCCEEDED, QUEUED SUCCEEDED, PROVISIONING FAULT, COMPLETED], so the check fails before any GetMetricStatistics assertion. Metric emission itself was therefore never exercised and is not claimed here.
Why: an outcome that AWS counts as a success or a failure is reported as an infrastructure fault, so no consumer — alarm, dashboard or the per-outcome counters themselves — can attribute a build outcome.
Fix: the CodeBuild build state machine behind StartBuild (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs plus the phase state batch_get_builds.rs reads back); once builds reach SUCCEEDED/FAILED, the AWS/CodeBuild metric publication for those transitions must follow.
Traces: cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace.

## Evidence
AssertionError: build sim2real-cwcb-<id>:<uuid> on sim2real-cwcb-<id> ended with buildStatus 'FAULT', expected 'SUCCEEDED'; the per-outcome metric totals asserted later are only attributable to builds whose outcome CodeBuild itself confirms. Phases were [('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)].

## Steps to Reproduce
Boot the emulator from this checkout and replay the cloudwatch__codebuild suite, or: create a NO_SOURCE project, StartBuild once with a buildspec exiting 0 and once with buildspecOverride exiting 1, poll BatchGetBuilds to terminal (both report FAULT), then call CloudWatch GetMetricStatistics for AWS/CodeBuild SucceededBuilds and FailedBuilds with dimension ProjectName.
