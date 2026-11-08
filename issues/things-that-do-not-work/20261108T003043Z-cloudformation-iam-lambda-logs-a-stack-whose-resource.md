Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected|run#34|AssertionError|assert stack_failed[0].get("DetailedStatus") == "VALIDATION_FAILED", (
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# cloudformation__iam__lambda__logs: a stack whose resource create fails template validation records no StackEvent DetailedStatus

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: `StackEvent.DetailedStatus` is never populated, so a validation-caused CREATE_FAILED is indistinguishable from any other create failure.
AWS: declaring `AWS::Logs::LogGroup` at a log-group name Lambda has already created implicitly fails the stack, and the stack's own `CREATE_FAILED` event carries `DetailedStatus = VALIDATION_FAILED` in addition to its `ResourceStatusReason`. Smithy: cloudformation `StackEvent.DetailedStatus` targets the `DetailedStatus` enum with values `CONFIGURATION_COMPLETE` and `VALIDATION_FAILED`, documented as "VALIDATION_FAILED - template validation failed because of invalid properties in the template. The ResourceStatusReason field shows what properties are defined incorrectly." The logical ids and the physical log-group name in the reason are values from the request; the enum value is the requirement.
Emulator: every event's `DetailedStatus` is absent (`None`), on the stack's `CREATE_FAILED`, on `ROLLBACK_COMPLETE` and on the resource's own `CREATE_FAILED`. The resource-level reason is produced (`Resource handler returned message: "The specified log group already exists" ... HandlerErrorCode: AlreadyExists`), so only the modeled detailed-status classification is missing.
Why: a client that classifies a failed deploy by `DetailedStatus` (validation error vs. runtime/provisioning error) cannot: every failure looks the same, and tooling that branches on `VALIDATION_FAILED` to re-synthesize a template never triggers.
Fix: the CloudFormation stack-event recorder/serializer must set `DetailedStatus` on the events it emits — `VALIDATION_FAILED` when a resource create fails template/property validation.
Traces: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected.

## Evidence
Expected: the stack's CREATE_FAILED StackEvent carries `DetailedStatus == 'VALIDATION_FAILED'`.
Actual: `AssertionError: stack CREATE_FAILED DetailedStatus is None, expected 'VALIDATION_FAILED'`; the recorded event tuples are `[('<stack>', 'ROLLBACK_COMPLETE', None, 'The following resource(s) failed to create: [LogGroup]. '), ('<stack>', 'CREATE_FAILED', None, 'The following resource(s) failed to create: [LogGroup]. '), ('LogGroup', 'CREATE_FAILED', None, 'Resource handler returned message: "The specified log group already exists" (RequestToken: ..., HandlerErrorCode: AlreadyExists)'), ...]` — every DetailedStatus is None.

## Steps to Reproduce
1. Deploy a stack with a Lambda function and role; invoke the function once so Lambda creates `/aws/lambda/<fn>` implicitly.
2. Create a second stack that declares `AWS::Logs::LogGroup` with `LogGroupName` equal to that implicit name.
3. Wait for the stack to reach a terminal status and call `DescribeStackEvents` with FailedEvents detail.
4. The stack's own `CREATE_FAILED` event must carry `DetailedStatus: VALIDATION_FAILED`; the emulator leaves it unset on every event.
