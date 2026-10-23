Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias codedeploy.alarm-rollback-returns-live-traffic-to-prior-version codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created codedeploy.batch-get-deployment-targets-lambda-target-shape codedeploy.concurrent-create-deployment-one-wins-rest-rejected codedeploy.create-deployment-rejects-malformed-appspec-content codedeploy.deployment-rollback-restores-alias-and-records-both-deployments codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment codedeploy.lambda-canary-diverts-live-invokes codedeploy.lambda-deployment-all-at-once-shifts-alias codedeploy.lambda-deployment-canary-holds-partial-alias-weight codedeploy.lambda-deployment-stop-rolls-back-alias-and-refuses-restop codedeploy.lifecycle-event-hook-status-gates-deployment-progress codedeploy.lifecycle-hook-failed-status-denied-traffic-shift codedeploy.linear-traffic-shift-walks-the-weight-ladder-over-time codedeploy.s3-revision-etag-mismatch-rejected-and-alias-unmoved codedeploy.s3-revision-object-contents-decide-alias-target codedeploy.skip-wait-time-triggers-immediate-termination codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only
Issue-Key: lambda|UpdateFunctionCode|NotImplemented
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# Implement lambda UpdateFunctionCode: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
`UpdateFunctionCode` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_function_code.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `UpdateFunctionCode` call, so each one unblocks only when `UpdateFunctionCode` works: `codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias`, `codedeploy.alarm-rollback-returns-live-traffic-to-prior-version`, `codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created`, `codedeploy.batch-get-deployment-targets-lambda-target-shape`, `codedeploy.concurrent-create-deployment-one-wins-rest-rejected`, `codedeploy.create-deployment-rejects-malformed-appspec-content`, `codedeploy.deployment-rollback-restores-alias-and-records-both-deployments`, `codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved`, `codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment`, `codedeploy.lambda-canary-diverts-live-invokes`, `codedeploy.lambda-deployment-all-at-once-shifts-alias`, `codedeploy.lambda-deployment-canary-holds-partial-alias-weight`, `codedeploy.lambda-deployment-stop-rolls-back-alias-and-refuses-restop`, `codedeploy.lifecycle-event-hook-status-gates-deployment-progress`, `codedeploy.lifecycle-hook-failed-status-denied-traffic-shift`, `codedeploy.linear-traffic-shift-walks-the-weight-ladder-over-time`, `codedeploy.s3-revision-etag-mismatch-rejected-and-alias-unmoved`, `codedeploy.s3-revision-object-contents-decide-alias-target`, `codedeploy.skip-wait-time-triggers-immediate-termination`, `codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_function_code.rs`. Replaying the corpus, the emulator refused the `UpdateFunctionCode` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateFunctionCode operation: operation UpdateFunctionCode is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda emulator
Call UpdateFunctionCode and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
