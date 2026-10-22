Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__events.eventbridge-project-name-filter-delivers-only-the-named-projects-build-events
Issue-Key: codebuild__events.eventbridge-project-name-filter-delivers-only-the-named-projects-build-events|run#26|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeBuild/EventBridge: builds FAULT at PROVISIONING so no terminal CodeBuild Build State Change event is ever published for a project-name-filtered rule

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
One root cause behind the single failing check of the `codebuild__events` cross-service suite (not marked ALREADY FILED or OWNED).

Cause 1 of 1: builds never reach a real terminal state, so there is no state change for EventBridge to deliver.
AWS: a build walks the Smithy BuildPhaseType sequence and ends SUCCEEDED on exit 0 (Smithy StatusType); reaching a terminal build status publishes a 'CodeBuild Build State Change' event on the default event bus, which a rule filtered on detail.project-name delivers only for the named project.
Emulator: the named project's build ends buildStatus FAULT with phases [SUBMITTED SUCCEEDED, QUEUED SUCCEEDED, PROVISIONING FAULT, COMPLETED] and the context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed', so the check cannot establish the terminal transition the event assertion depends on.
Why: event-driven consumers (pipelines, notifications, metrics) never observe build completion, and the filtering behaviour of a project-name rule cannot be exercised.
Fix: the CodeBuild build state machine behind StartBuild and the state-change event it must publish on terminal transitions (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs plus the phase state batch_get_builds.rs reads back, and the EventBridge publication hook). Same underlying defect as the codebuild-service issue filed in this sweep.
Traces: codebuild__events.eventbridge-project-name-filter-delivers-only-the-named-projects-build-events.

## Evidence
AssertionError: the named project's build must SUCCEED so that it definitely had a terminal state change to publish; got 'FAULT'; phases=[('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)]; contexts=[None, None, [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}], None].

## Steps to Reproduce
Boot the emulator from this checkout and replay the codebuild__events suite, or: create two projects, PutRule with an event pattern on source aws.codebuild / detail-type 'CodeBuild Build State Change' filtered by detail.project-name, StartBuild on the named project, poll BatchGetBuilds to terminal (observe FAULT) and observe that no state-change event is delivered.
