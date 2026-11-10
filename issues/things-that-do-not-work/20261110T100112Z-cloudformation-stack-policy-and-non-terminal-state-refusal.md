Issue-Source: trace-sweep-divergence
Issue-Trace: stack-policy-input-validation
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: stack-policy-input-validation cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateStack|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|UpdateStack|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudformation stack-policy and non-terminal-state refusal messages are reworded, so clients matching AWS text miss them

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes; both are refusal messages the emulator authors with its own wording instead of AWS's. Each is numbered with its own fixture and step; both must be closed before this issue is done. The status (400) and the error code are correct in both cases — only the Message text diverges, and the message is part of the contract.

Cause 1 of 2: CreateStack called with both StackPolicyBody and StackPolicyURL names the two parameters in the wrong order.
AWS: the refusal is HTTP 400 with the message 'You cannot specify both StackPolicyURL and StackPolicyBody'. The whole string is fixed text — it names the mutually exclusive members in that order and has no variable part and no trailing period. Smithy: CreateStack input carries both StackPolicyBody (@length 1..16384) and StackPolicyURL (@length 1..5120); the mutual exclusion is not modelled, so the message is recorded behavior.
Emulator: HTTP 400 with 'You cannot specify both StackPolicyBody and StackPolicyURL' — the two member names are swapped.
Why: a client or test that matches on the AWS message (or logs it for an operator) does not recognise the refusal, and the message does not match what AWS's own documentation and SDK tests show.
Fix: the CreateStack input-validation path that rejects StackPolicyBody+StackPolicyURL (same validator that handles the StackPolicyDuringUpdate pair).
Traces: stack-policy-input-validation#task#4.

Cause 2 of 2: the non-terminal-state UpdateStack refusal appends a full stop AWS does not emit.
AWS: HTTP 400 with 'This stack is currently in a non-terminal [<StackStatus>] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API' — fixed text with one variable part, the stack's current status taken from emulator state and rendered in square brackets (here UPDATE_FAILED), and NO trailing period after 'rollback-stack API'.
Emulator: emits the same text with a '.' appended after 'rollback-stack API'.
Why: callers that string-match the refusal to decide between `update-stack --disable-rollback` and `rollback-stack` fail to match, and the emulator's text diverges from AWS for every stack status that reaches this branch.
Fix: the UpdateStack handler's non-terminal-state guard message template.
Traces: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#8 (2 steps in the cluster, the other being stack-policy-input-validation#task#7).

## Evidence
Cause 1 (CreateStack, stack-policy-input-validation#task#4, status 400 both sides): $.ErrorResponse.Error.Message: value_mismatch (expected 'You cannot specify both StackPolicyURL and StackPolicyBody', got 'You cannot specify both StackPolicyBody and StackPolicyURL').
Cause 2 (UpdateStack, cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#8, status 400 both sides): $.ErrorResponse.Error.Message: value_mismatch (expected '...To rollback to the last known good state, use the rollback-stack API', got the same text with a trailing '.').

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture stack-policy-input-validation step task#4 — CreateStack with both StackPolicyBody and StackPolicyURL set and compare ErrorResponse.Error.Message.
3. Cause 2: replay fixture cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward step task#8 — drive a stack into UPDATE_FAILED, call UpdateStack without --disable-rollback and compare ErrorResponse.Error.Message character for character.
