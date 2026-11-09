Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__events.eventbridge-project-name-filter-delivers-only-the-named-projects-build-events
Issue-Key: codebuild__events.eventbridge-project-name-filter-delivers-only-the-named-projects-build-events|_drain#28|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CodeBuild: build state-change events never reach an EventBridge rule filtered by project-name

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Cause 1 of 1: no CodeBuild build state-change event is delivered.
AWS: `StartBuild` drives a build through its phases and CodeBuild publishes `CodeBuild Build State Change` events (source `aws.codebuild`) onto the default bus, with `detail.project-name` and `detail.build-status`. A rule whose pattern constrains `detail.project-name` to a named project receives only that project's build events, and a sibling project's builds are withheld from it.
Emulator: nothing is delivered — the check exhausts its budget waiting for the named project's build event (`Check timed out after 180.0s`), so neither the positive delivery nor the withholding of the other project's events can be observed.
Why: a client that gates a pipeline or a notification on build state gets silence: the rule never fires, nothing is dead-lettered, and no error explains it.
Fix: the CodeBuild build lifecycle's event emission — each build phase/state transition must publish a `CodeBuild Build State Change` event onto the default bus so EventBridge pattern filtering on `detail.project-name` can run.
Traces: codebuild__events.eventbridge-project-name-filter-delivers-only-the-named-projects-build-events.

## Evidence
`Error: Check timed out after 180.0s` — the check waits for a `CodeBuild Build State Change` event for the named project to reach the rule's target and none ever arrives.

## Steps to Reproduce
1. Create two CodeBuild projects.
2. PutRule on the default bus with `{"source":["aws.codebuild"],"detail-type":["CodeBuild Build State Change"],"detail":{"project-name":["<project-a>"]}}` and PutTargets an SQS queue.
3. `codebuild.start_build(projectName='<project-a>')`, then `start_build` on project B.
4. Poll the queue — no event for either project arrives.
5. Replay codebuild__events.eventbridge-project-name-filter-delivers-only-the-named-projects-build-events.
