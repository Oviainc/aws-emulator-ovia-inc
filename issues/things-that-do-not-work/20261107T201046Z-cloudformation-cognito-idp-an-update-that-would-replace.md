Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback
Issue-Key: cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback|run#16|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudformation__cognito-idp: an update that would replace a custom-named AWS::Cognito::UserPoolGroup reaches UPDATE_COMPLETE instead of failing the resource and rolling back

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: CloudFormation does not refuse a replacement of a resource whose name is declared explicitly in the template, so an update that AWS fails and rolls back completes instead.
AWS: AWS::Cognito::UserPoolGroup's GroupName is a create-only property, so declaring it (even with the exact value CloudFormation itself generated) forces a REPLACEMENT. CloudFormation refuses to replace a custom-named resource during an update: the Group resource gets exactly one UPDATE_FAILED stack event whose ResourceStatusReason is "CloudFormation cannot update a stack when a custom-named resource requires replacing. Rename <resource name> and update the stack again." — the two sentences are fixed text and the only variable part is the resource's name taken from the template/state — and the stack ends UPDATE_ROLLBACK_COMPLETE with the live group untouched (same CreationDate, still the only group in the pool). A subsequent ordinary update with no GroupName declared then succeeds in place (UPDATE_COMPLETE, description changed, CreationDate unchanged). (Error class and message: recorded behavior plus AWS's documented custom-named-resource replacement rule; the Smithy model does not describe template-level update semantics.)
Emulator: the update is applied and the stack reports UPDATE_COMPLETE, so no UPDATE_FAILED event, no reason sentence, and no rollback.
Why: the refusal exists to stop an update from silently destroying and re-creating a named resource (here a Cognito user-pool group and its membership). A client that relies on it gets the opposite signal — success — and cannot tell whether the group it depends on was preserved or replaced; worse, deployment tooling that treats UPDATE_COMPLETE as "the declared state is live" will never surface the rename AWS demands.
Fix: the CloudFormation update planner/provisioner — detect that a required replacement targets a resource with a template-declared (custom) name, fail that resource with AWS's reason and drive the stack into UPDATE_ROLLBACK_COMPLETE without touching the live resource.
Traces: cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback.

## Evidence
From the behavioral replay — cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback [mismatch]:
  assert status == "UPDATE_ROLLBACK_COMPLETE", (...)
  AssertionError: stack status after declaring the generated name explicitly is 'UPDATE_COMPLETE', expected UPDATE_ROLLBACK_COMPLETE

Sequence: CreateStack with AWS::Cognito::UserPoolGroup and NO GroupName (reaches CREATE_COMPLETE; the generated name is read from the stack's GroupRef output), then UpdateStack with the same template plus GroupName set to that exact generated value.
Expected (AWS): stack UPDATE_ROLLBACK_COMPLETE; exactly one UPDATE_FAILED event for logical id Group with ResourceStatusReason "CloudFormation cannot update a stack when a custom-named resource requires replacing. Rename <generated group name> and update the stack again."; cognito-idp GetGroup shows the same CreationDate and ListGroups still returns exactly that one group.
Actual (emulator): stack UPDATE_COMPLETE; the refusal, the event and its reason never occur.

## Steps to Reproduce
Replay the behavioral suite and re-run cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback. Manually: cognito-idp:CreateUserPool; CreateStack with a single AWS::Cognito::UserPoolGroup (UserPoolId, Description "d1", Precedence 1, no GroupName) and an output {"Ref":"Group"}; read the generated name from DescribeStacks Outputs; UpdateStack with the same template but GroupName = that generated name; poll DescribeStacks for a terminal status (expect UPDATE_ROLLBACK_COMPLETE) and read DescribeStackEvents for the Group UPDATE_FAILED reason.
