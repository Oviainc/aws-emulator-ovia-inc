Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__logs.codebuild-buildspec-stdout-reaches-the-configured-cloudwatch-log-group codebuild__logs.codebuild-log-delivery-denied-without-putlogevents-permission codebuild__logs.codebuild-parameter-store-env-var-resolves-and-its-exact-value-is-masked-in-the-log
Issue-Key: codebuild__logs.codebuild-buildspec-stdout-reaches-the-configured-cloudwatch-log-group|run#19|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__logs.codebuild-log-delivery-denied-without-putlogevents-permission|run#25|AssertionError|assert ok_1["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__logs.codebuild-parameter-store-env-var-resolves-and-its-exact-value-is-masked-in-the-log|run#25|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeBuild/CloudWatch Logs: builds FAULT at PROVISIONING so no buildspec stdout is ever delivered to the configured log group

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
One root cause behind all 3 failing checks of the `codebuild__logs` cross-service suite (none marked ALREADY FILED or OWNED).

Cause 1 of 1: builds never execute, so nothing is ever written to CloudWatch Logs.
AWS: a build walks the Smithy BuildPhaseType sequence and ends SUCCEEDED on exit 0 (Smithy StatusType); with logsConfig.cloudWatchLogs ENABLED, the buildspec's stdout is delivered to the configured log group/stream, a build whose role lacks logs:PutLogEvents fails delivery, and a value resolved from a Parameter Store env var is masked in the log.
Emulator: every build ends buildStatus FAULT with phases [SUBMITTED SUCCEEDED, QUEUED SUCCEEDED, PROVISIONING FAULT, COMPLETED] and the context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. No buildspec runs, so no log events exist to deliver, mask or deny.
Why: log-driven CI consumers (dashboards, failure triage, secret-masking assertions) see no events at all, and the build reports an infrastructure FAULT rather than SUCCEEDED/FAILED.
Fix: the CodeBuild build state machine behind StartBuild and its log-delivery step (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs plus the phase state batch_get_builds.rs reads back, and the CloudWatch Logs writes that the build's logsConfig implies). Same underlying defect as the codebuild-service issue filed in this sweep.
Traces: codebuild__logs.codebuild-buildspec-stdout-reaches-the-configured-cloudwatch-log-group, codebuild__logs.codebuild-log-delivery-denied-without-putlogevents-permission, codebuild__logs.codebuild-parameter-store-env-var-resolves-and-its-exact-value-is-masked-in-the-log.

## Evidence
AssertionError: build was 'FAULT'; phases=[('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)]; and "the build must succeed for its log to be evidence; got 'FAULT' ... contexts=[None, None, [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}], None]".

## Steps to Reproduce
Boot the emulator from this checkout and replay the codebuild__logs suite, or: create a project with logsConfig.cloudWatchLogs ENABLED and a buildspec that echoes a marker, StartBuild, poll BatchGetBuilds to terminal (observe FAULT), then FilterLogEvents on the configured log group and observe no events.
