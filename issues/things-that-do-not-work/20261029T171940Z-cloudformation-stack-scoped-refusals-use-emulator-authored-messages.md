Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-template-generator-and-hook-result-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-template-generator-and-hook-result-errors cloudformation-stack-resource-inspection cloudformation-stack-rollback-and-signals cloudformation-stack-utils-lifecycle cloudformation-stack-drift-safe cfnrep-drift-modified-property-differences cloudformation-stack-drift-detection-workflow cloudformation-stack-policy-and-termination-protection cloudformation-stack-drift-detection cloudformation-stack-failure-rollback-and-cancel-recovery cloudformation-stack-recovery-operation-errors
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeChangeSetHooks|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|DescribeChangeSetHooks|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeChangeSetHooks|4xx->4xx|status
Issue-Key: cloudformation|DescribeStackResource|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|SignalResource|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|SetStackPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DetectStackDrift|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DetectStackResourceDrift|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeStackDriftDetectionStatus|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ContinueUpdateRollback|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|RollbackStack|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation stack-scoped refusals use emulator-authored messages, codes and statuses

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChangeSetHooks

## Description
Nine stack-scoped operations refuse with the emulator's own wording instead of AWS's, and one of them also returns the wrong error code and HTTP status. AWS does not use one 'stack does not exist' sentence — each operation has its own fixed template (bare, quoted, bracketed, or colon-prefixed) and the only variable part is the stack name from the request — so this cannot be fixed by one shared message. 9 net-new root causes, one service.

The replay reported owned_by=[] and no stub for all nine. The open issue "CloudFormation refusal taxonomy diverges: wrong not-found/validation message templates, duplicate ClientRequestToken not TokenAlreadyExistsException, ..." is the nearest neighbour and carries none of these fixture#step bindings, so none of these refusals is currently exercised by any issue's replay.

Cause 1 of 9: DescribeChangeSetHooks answers the wrong status, code and message when the named stack does not exist.
AWS: Smithy: DescribeChangeSetHooks input StackName (StackNameOrId) and ChangeSetName; its only modelled error is ChangeSetNotFoundException (404, awsQuery code ChangeSetNotFound). When the StackName itself does not exist, the recorded refusal is 400 with Code ValidationError and Message 'Stack [<StackName from the request>] does not exist' — the stack is resolved before the change set.
Emulator: 404 with Code ChangeSetNotFound and 'ChangeSet [<ChangeSetName>] does not exist'.
Why: a client cannot tell a bad stack name (fix the stack) from a missing change set (create one), and retry logic keyed on 400-vs-404 branches the wrong way.
Fix: the DescribeChangeSetHooks handler's resolution order and its error construction.
Traces: cloudformation-template-generator-and-hook-result-errors#task#6.

Cause 2 of 9: DescribeStackResource's stack-not-found message.
AWS: 400 ValidationError with "Stack '<StackName from the request>' does not exist" — single-quoted stack name, fixed text otherwise.
Emulator: 'Stack with id <name> does not exist'.
Why: the message is a requirement of this error class; clients matching AWS's text do not match.
Fix: the DescribeStackResource handler's not-found message.
Traces: cloudformation-stack-resource-inspection#task#7.

Cause 3 of 9: SignalResource's stack-not-found message.
AWS: 400 ValidationError with 'Stack <StackName> does not exist' — bare name, no quoting or bracketing.
Emulator: 'Stack with id <name> does not exist'.
Why/Fix: as cause 2, in the SignalResource handler.
Traces: cloudformation-stack-rollback-and-signals#task#6.

Cause 4 of 9: SetStackPolicy's stack-not-found message.
AWS: 400 ValidationError with 'Stack: <StackName> does not exist' — colon after 'Stack'.
Emulator: 'Stack with id <name> does not exist'.
Why/Fix: as cause 2, in the SetStackPolicy handler.
Traces: cloudformation-stack-utils-lifecycle#task#12.

Cause 5 of 9: DetectStackDrift's stack-not-found message.
AWS: 400 ValidationError with 'Stack [<StackName>] does not exist' — square-bracketed name.
Emulator: 'Stack with id <name> does not exist'.
Why/Fix: as cause 2, in the DetectStackDrift handler.
Traces: cloudformation-stack-drift-safe#task#4.

Cause 6 of 9: DetectStackResourceDrift's resource-not-in-stack message names the stack by name, not by id.
AWS: 400 ValidationError with 'Resource <LogicalResourceId from the request> does not exist in stack <the stack's full stack ARN>' — the fixed text is 'does not exist in stack' and the variable parts are the requested logical id and the resolved stack id.
Emulator: 'Resource <logical id> does not exist for stack <stack name>' — different preposition and the stack name instead of its ARN.
Why/Fix: as cause 2, in the DetectStackResourceDrift handler (which must resolve the stack to its id for the message).
Traces: cfnrep-drift-modified-property-differences#task#6, cloudformation-stack-drift-detection-workflow#task#8, cloudformation-stack-drift-safe#task#6, cloudformation-stack-policy-and-termination-protection#task#14.

Cause 7 of 9: DescribeStackDriftDetectionStatus' unknown-detection-id message.
AWS: 400 ValidationError with 'Stack drift detection result [<StackDriftDetectionId from the request>] does not exist'.
Emulator: 'Drift detection with id <id> does not exist'.
Why/Fix: as cause 2, in the DescribeStackDriftDetectionStatus handler.
Traces: cloudformation-stack-drift-detection#task#6, cloudformation-stack-drift-safe#task#5.

Cause 8 of 9: ContinueUpdateRollback's wrong-status refusal leaks state into the message.
AWS: Smithy: ContinueUpdateRollback's only modelled error is TokenAlreadyExistsException; the wrong-status refusal is 400 ValidationError with the fixed sentence 'RollbackUpdatedStack cannot be called from current stack status' — no stack id or status is interpolated.
Emulator: 400 with 'Stack:<stack ARN> is in <status> state and can not be continued.'
Why: the refusal text is a requirement of this error class; the emulator's version embeds per-run values (the stack ARN) that AWS's message never contains.
Fix: the ContinueUpdateRollback status gate's error message.
Traces: cloudformation-stack-failure-rollback-and-cancel-recovery#task#5, cloudformation-stack-recovery-operation-errors#task#9, cloudformation-stack-rollback-and-signals#task#4.

Cause 9 of 9: RollbackStack's wrong-status refusal does the same.
AWS: 400 ValidationError with the fixed sentence 'RollbackStack can only be called on any of the [UPDATE_FAILED, CREATE_FAILED] statuses.' — the permitted-status list is fixed text, nothing is interpolated.
Emulator: 400 with 'Stack:<stack ARN> is in <status> state and can not be rolled back.'
Why/Fix: as cause 8, in the RollbackStack status gate.
Traces: cloudformation-stack-recovery-operation-errors#task#8, cloudformation-stack-rollback-and-signals#task#5.

## Evidence
Replay of simparity-wire-aws-v1, 9 mismatch clusters:
- cloudformation-template-generator-and-hook-result-errors#task#6 DescribeChangeSetHooks: status expected 400, got 404; Code expected 'ValidationError', got 'ChangeSetNotFound'; Message expected 'Stack [sim2real-probe-none-stack] does not exist', got 'ChangeSet [sim2real-probe-none-changeset] does not exist'.
- cloudformation-stack-resource-inspection#task#7 DescribeStackResource 400: expected "Stack '<name>' does not exist", got 'Stack with id <name> does not exist'.
- cloudformation-stack-rollback-and-signals#task#6 SignalResource 400: expected 'Stack <name> does not exist', got 'Stack with id <name> does not exist'.
- cloudformation-stack-utils-lifecycle#task#12 SetStackPolicy 400: expected 'Stack: <name> does not exist', got 'Stack with id <name> does not exist'.
- cloudformation-stack-drift-safe#task#4 DetectStackDrift 400: expected 'Stack [<name>] does not exist', got 'Stack with id <name> does not exist'.
- cfnrep-drift-modified-property-differences#task#6 DetectStackResourceDrift 400: expected 'Resource NoSuchResource does not exist in stack arn:aws:cloudformation:us-west-2:000000000000:stack/<name>/<id>', got 'Resource NoSuchResource does not exist for stack <name>' (4 affected steps).
- cloudformation-stack-drift-detection#task#6 DescribeStackDriftDetectionStatus 400: expected 'Stack drift detection result [<id>] does not exist', got 'Drift detection with id <id> does not exist' (2 affected steps).
- cloudformation-stack-failure-rollback-and-cancel-recovery#task#5 ContinueUpdateRollback 400: expected 'RollbackUpdatedStack cannot be called from current stack status', got 'Stack:<stack ARN> is in CREATE_IN_PROGRESS state and can not be continued.' (3 affected steps).
- cloudformation-stack-recovery-operation-errors#task#8 RollbackStack 400: expected 'RollbackStack can only be called on any of the [UPDATE_FAILED, CREATE_FAILED] statuses.', got 'Stack:<stack ARN> is in CREATE_COMPLETE state and can not be rolled back.' (2 affected steps).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Call DescribeChangeSetHooks(StackName=<nonexistent stack>, ChangeSetName=<any>) — the emulator answers 404 ChangeSetNotFound instead of 400 ValidationError 'Stack [<stack>] does not exist'.
3. Call DescribeStackResource, SignalResource, SetStackPolicy and DetectStackDrift against a nonexistent stack name and compare each 400 message with the per-operation AWS template (quoted / bare / colon-prefixed / bracketed).
4. Call DetectStackResourceDrift with a LogicalResourceId that is not in an existing stack, and DescribeStackDriftDetectionStatus with an unknown detection id.
5. Call ContinueUpdateRollback on a CREATE_IN_PROGRESS stack and RollbackStack on a CREATE_COMPLETE stack — both messages interpolate the stack ARN and status, which AWS's fixed sentences do not.
