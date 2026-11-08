Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected|run#33|AssertionError|assert len(stack_failed) == 1, f"expected one stack-level CREATE_FAILED: {summary!r}"
Issue-Commit: f1d70ff20b4a607718bee5570d5907dd84812972

# cloudformation__iam__lambda__logs: a failed CreateStack emits no stack-level CREATE_FAILED StackEvent at all — the event stream jumps from CREATE_IN_PROGRESS straight to ROLLBACK_IN_PROGRESS

> _Note: this issue was identified against the code base at commit `f1d70ff20b4a607718bee5570d5907dd84812972`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: CloudFormation's stack-event recorder no longer emits the stack-level CREATE_FAILED event for a create that fails and rolls back. DescribeStackEvents for the rolled-back stack contains events for the failing resource and the stack's CREATE_IN_PROGRESS / ROLLBACK_IN_PROGRESS / ROLLBACK_COMPLETE transitions, but NO event whose LogicalResourceId is the stack itself and whose ResourceStatus is CREATE_FAILED.

AWS: when a CreateStack fails, the stack's own failure is recorded as a StackEvent whose LogicalResourceId (and StackName) is the stack and whose ResourceStatus is CREATE_FAILED, emitted exactly once, before the ROLLBACK_IN_PROGRESS transition. Its ResourceStatusReason is the aggregate wording whose fixed text is "The following resource(s) failed to create: [" … "]. " — the variable part is the comma-separated list of the logical ids that failed, taken from the template and the operation (here the single logical id of the AWS::Logs::LogGroup). For this scenario — declaring an AWS::Logs::LogGroup at a log-group name that already exists, which AWS catches in pre-deployment validation — that same stack-level CREATE_FAILED event additionally carries DetailedStatus = VALIDATION_FAILED. Smithy (service-com-amazonaws-cloudformation-cloudformation): StackEvent carries required StackId/EventId/StackName/Timestamp plus LogicalResourceId, ResourceStatus, ResourceStatusReason and DetailedStatus, where DetailedStatus targets the DetailedStatus enum with values CONFIGURATION_COMPLETE and VALIDATION_FAILED, documented as "VALIDATION_FAILED - template validation failed because of invalid properties in the template. The ResourceStatusReason field shows what properties are defined incorrectly." DescribeStackEvents is paginated over StackEvents. The presence and the wording of the stack-level CREATE_FAILED event itself is recorded behavior only; the DetailedStatus enum value is what the model states.

Emulator: the paginated DescribeStackEvents stream for the failed stack is, newest first, (LogicalResourceId, ResourceStatus, DetailedStatus, ResourceStatusReason): (<stack>, ROLLBACK_COMPLETE, None, "The following resource(s) failed to create: [LogGroup]. "), (LogGroup, DELETE_COMPLETE, None, None), (LogGroup, DELETE_IN_PROGRESS, None, None), (<stack>, ROLLBACK_IN_PROGRESS, None, "The following resource(s) failed to create: [LogGroup]. Rollback requested by user."), (LogGroup, CREATE_FAILED, None, 'Resource handler returned message: "The specified log group already exists" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)'), (LogGroup, CREATE_IN_PROGRESS, None, None), (<stack>, CREATE_IN_PROGRESS, None, "User Initiated"). There is no (<stack>, CREATE_FAILED, …) entry anywhere in the stream, so a selection of stack-scoped CREATE_FAILED events yields zero rows where AWS yields exactly one.

Why: a client that reads DescribeStackEvents to decide whether the stack-level operation failed, and to classify why, has nothing to read: the stack's own failure record is absent, so the aggregate "The following resource(s) failed to create: [...]" reason is only reachable off the ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE events, and the DetailedStatus classification (VALIDATION_FAILED vs. a provisioning failure) has no event to hang on at all. Deployment tooling that waits for, or branches on, a stack-level CREATE_FAILED event — rather than polling StackStatus — never observes the failure and misattributes it to the resource alone.

Fix: the CloudFormation stack-event recorder on the create-stack path (the component that appends StackEvents as the stack transitions). It must append the stack-scoped CREATE_FAILED event, once, with the aggregate failed-resource reason, before the ROLLBACK_IN_PROGRESS transition. Note that this is the inverse of what two earlier issues asked for: 20261029T175114Z-cloudformation-stack-outputs-carry-no-outputvalue-an-asg.md (Cause 3, "a create that fails on one resource emits an extra stack-level CREATE_FAILED event") and 20261106T123006Z-cognito-identity-5-net-new-root-causes-cfn.md ("a stack-level CREATE_FAILED event is invented") called for that event to be suppressed; suppressing it unconditionally is what this check now observes, so the recorder needs to distinguish the event AWS does emit from whatever those checks object to, not drop it. Also related but NOT covering this: 20261108T003043Z-cloudformation-iam-lambda-logs-a-stack-whose-resource.md (that issue presumes the stack-level CREATE_FAILED event EXISTS and only its DetailedStatus is unset — it carries a different replay key, the DetailedStatus assertion) and 20261107T133755Z-cloudformation-iam-lambda-logs-createstack-declaring-an-aws.md (no NAME_CONFLICT_VALIDATION pre-deployment validation, keyed on the terminal-status assertion, which now passes).

Traces: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected (mismatch at the stack-level CREATE_FAILED count, after the stack had already been confirmed to settle in ROLLBACK_COMPLETE).

## Evidence
Check: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected, verdict mismatch.

Failing assertion (check source line 213):
  stack_failed = [e for e in events if e["LogicalResourceId"] == collide_stack and e["ResourceStatus"] == "CREATE_FAILED"]
  assert len(stack_failed) == 1, f"expected one stack-level CREATE_FAILED: {summary!r}"

Replay detail:
  AssertionError: expected one stack-level CREATE_FAILED: [('sim2real-xsvc-stk-e18423b00bba', 'ROLLBACK_COMPLETE', None, 'The following resource(s) failed to create: [LogGroup]. '), ('LogGroup', 'DELETE_COMPLETE', None, None), ('LogGroup', 'DELETE_IN_PROGRESS', None, None), ('sim2real-xsvc-stk-e18423b00bba', 'ROLLBACK_IN_PROGRESS', None, 'The following resource(s) failed to create: [LogGroup]. Rollback requested by user.'), ('LogGroup', 'CREATE_FAILED', None, 'Resource handler returned message: "The specified log group already exists" (RequestToken: 6c9d20a0-5e1c-0534-4714-2e0292fed38b, HandlerErrorCode: AlreadyExists)'), ('LogGroup', 'CREATE_IN_PROGRESS', None, None), ('sim2real-xsvc-stk-e18423b00bba', 'CREATE_IN_PROGRESS', None, 'User Initiated')]
  (tuples are (LogicalResourceId, ResourceStatus, DetailedStatus, ResourceStatusReason); the stack name and the RequestToken are per-run values and are not part of the requirement)

Expected (AWS), for this failed CreateStack:
  - exactly one StackEvent with LogicalResourceId == the stack name and ResourceStatus == "CREATE_FAILED";
  - its ResourceStatusReason is "The following resource(s) failed to create: [<failed logical ids>]. ";
  - for this name-conflict validation failure, that event's DetailedStatus is "VALIDATION_FAILED";
  - it precedes the stack's ROLLBACK_IN_PROGRESS event.

Actual (emulator): zero StackEvents with LogicalResourceId == the stack name and ResourceStatus == "CREATE_FAILED"; the stack's event sequence is CREATE_IN_PROGRESS -> ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE with no CREATE_FAILED in between.

Supporting, from the same run: the stack DID settle in ROLLBACK_COMPLETE and the failing resource's own CREATE_FAILED event WAS recorded with the AlreadyExists handler reason, so the failure itself was detected and only the stack-scoped event is missing. Every event in the stream has DetailedStatus None.

## Steps to Reproduce
1. Boot the emulator; point cloudformation, iam, lambda and logs clients at it.
2. CreateStack A from a template containing only an AWS::IAM::Role (managed policy AWSLambdaBasicExecutionRole) and an AWS::Lambda::Function named <fn> with an inline ZipFile handler; wait for CREATE_COMPLETE.
3. Invoke <fn> once so /aws/lambda/<fn> is created implicitly; confirm with logs DescribeLogGroups that the group exists with no retentionInDays.
4. DeleteStack A and wait for it to go away — the implicit /aws/lambda/<fn> group stays behind.
5. CreateStack B from a template declaring AWS::Logs::LogGroup{LogGroupName: /aws/lambda/<fn>, RetentionInDays: 3} plus the same role and a function named <fn> that DependsOn the log group; wait for a terminal stack status.
6. Page cloudformation DescribeStackEvents for stack B and select the events whose LogicalResourceId equals the stack name and whose ResourceStatus is CREATE_FAILED.
   Expected: exactly one such event, with ResourceStatusReason "The following resource(s) failed to create: [LogGroup]. " and DetailedStatus "VALIDATION_FAILED", ordered before the stack's ROLLBACK_IN_PROGRESS event.
   Actual: zero such events; the stack's own transitions are only CREATE_IN_PROGRESS, ROLLBACK_IN_PROGRESS and ROLLBACK_COMPLETE.
7. Re-replay cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected to confirm a fix.
