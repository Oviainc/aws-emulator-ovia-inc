Issue-Source: trace-sweep-divergence
Issue-Trace: stack-policy-input-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: stack-policy-input-validation stack-resource-types-input-validation template-constraint-description-messages
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateStack|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|CreateStack|4xx->2xx|status
Issue-Key: cloudformation|SetStackPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|SetStackPolicy|4xx->2xx|status
Issue-Key: cloudformation|GetStackPolicy|2xx->2xx|$.GetStackPolicyResponse.GetStackPolicyResult
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.LastOperations.member.OperationType
Issue-Key: cloudformation|DescribeStacks|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|DescribeStacks|4xx->2xx|status
Issue-Related: 20261029T171940Z-cloudformation-change-set-previews-operation-events-and-the.md
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation never validates a stack policy, so invalid policies are stored and applied

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetStackPolicy

## Description
Stack policy bodies are stored without any validation, so a structurally invalid policy is accepted on both the create and the set path, is then readable, and the rejected set is even recorded as a stack update. This bundle carries 5 distinct causes.

Cause 1 of 5 - CreateStack with an invalid StackPolicyBody.
AWS: 400 Sender/ValidationError, Message 'Error validating stack policy to be applied after stack creation: Invalid stack policy' - entirely fixed text for this error class. Smithy: CreateStack takes StackPolicyBody with length{min:1,max:16384}; the policy's CONTENT is validated by the service, which is why the refusal is a ValidationError rather than a constraint message.
Emulator: 200, the stack is created with the invalid policy.
Why: a caller believes its update protection is in force when nothing was accepted by AWS at all.
Fix: CreateStack's stack-policy validation, ahead of stack creation.
Traces: stack-policy-input-validation#task#3 (cluster also covers stack-resource-types-input-validation#task#1 and #task#3, template-constraint-description-messages#task#2 through #task#5).

Cause 2 of 5 - SetStackPolicy with an invalid StackPolicyBody.
AWS: 400 Sender/ValidationError, Message 'Error validating stack policy: Invalid stack policy' - entirely fixed text. Smithy: SetStackPolicy has an empty output and declares no operation-specific error, so the refusal is the service-common ValidationError.
Emulator: 200.
Why: same as cause 1, on the mutate-later path.
Fix: SetStackPolicy's stack-policy validation.
Traces: stack-policy-input-validation#task#8.

Cause 3 of 5 - GetStackPolicy after the refused SetStackPolicy.
AWS: GetStackPolicyResult is empty - no StackPolicyBody, because the refused policy was never stored.
Emulator: returns the invalid policy body it stored.
Why: the read-back confirms a policy that AWS does not hold, so a caller cannot tell a rejected set from an applied one.
Fix: follows from cause 2 - the refused body must not be written to stack state; GetStackPolicy's serializer must omit an absent body.
Traces: stack-policy-input-validation#task#9.

Cause 4 of 5 - DescribeStacks after the refused SetStackPolicy.
AWS: the stack is untouched - StackStatus 'CREATE_COMPLETE' and LastOperations.OperationType 'CREATE_STACK'; SetStackPolicy is not a stack update and records no operation.
Emulator: StackStatus 'UPDATE_IN_PROGRESS' and LastOperations.OperationType 'UPDATE_STACK'.
Why: a caller polling the stack sees a phantom update in progress and waits on an operation AWS never started.
Fix: SetStackPolicy's handler must not transition stack state or append a stack operation.
Traces: stack-policy-input-validation#task#10.

Cause 5 of 5 - DescribeStacks for a stack whose CreateStack was refused.
AWS: 400 Sender/ValidationError, Message 'Stack with id <stackName> does not exist' - fixed text 'Stack with id ' / ' does not exist' with the requested name; a refused create leaves no stack.
Emulator: 200, describing a stack that should not exist.
Why: the refused create leaves a ghost stack that later calls can mutate, and the name stays taken.
Fix: follows from cause 1 - the refusal must happen before any stack record is written.
Traces: stack-policy-input-validation#task#5 (cluster also covers stack-resource-types-input-validation#task#2 and #task#4, template-constraint-description-messages#task#6 through #task#9).

## Evidence
Replay of simparity-wire-aws-v1, 5 divergence clusters, all stub_path='' and owned_by=[].
Cause 1: expected 400 ValidationError 'Error validating stack policy to be applied after stack creation: Invalid stack policy', got 200 (7 affected steps).
Cause 2: expected 400 ValidationError 'Error validating stack policy: Invalid stack policy', got 200.
Cause 3: expected GetStackPolicyResult '', got {'StackPolicyBody': '{"Statement": [{"Effect": "Maybe", ...}]}'} - i.e. the invalid body with an Effect value that is not Allow/Deny was stored and returned.
Cause 4: expected StackStatus 'CREATE_COMPLETE' and LastOperations.member.OperationType 'CREATE_STACK', got 'UPDATE_IN_PROGRESS' and 'UPDATE_STACK'.
Cause 5: expected 400 ValidationError 'Stack with id <stackName> does not exist', got 200 (7 affected steps).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture stack-policy-input-validation, step task#3 - CreateStack with a StackPolicyBody whose statement Effect is not Allow/Deny.
3. Cause 5: same fixture, step task#5 - DescribeStacks on that stack name.
4. Causes 2-4: same fixture, steps task#8, task#9 and task#10 - SetStackPolicy with the invalid body, then GetStackPolicy and DescribeStacks.
5. Compare each response's status, Error.Message, GetStackPolicyResult, StackStatus and LastOperations against the recorded AWS responses.
