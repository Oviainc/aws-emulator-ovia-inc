Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip|_wait_terminal#21|Timeout|time.sleep(5)
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation: UPDATE_ROLLBACK_FAILED recovery never converges — the ContinueUpdateRollback / ResourcesToSkip enforcement check exhausts the harness window with no terminal stack state

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ContinueUpdateRollback

## Description
Cause 1 of 1 — the UPDATE_ROLLBACK_FAILED recovery state machine does not reach a terminal state, so the ContinueUpdateRollback contract is unobservable.
AWS: recorded behavior only — a stack whose update fails and whose rollback cannot restore a resource lands in UPDATE_ROLLBACK_FAILED. ContinueUpdateRollback WITHOUT ResourcesToSkip is accepted and the stack settles back in UPDATE_ROLLBACK_FAILED (the enforcement holds and the resource keeps its updated value); WITH ResourcesToSkip=[<logical-id>] the stack reaches UPDATE_ROLLBACK_COMPLETE and the skipped resource is left exactly as the failed update made it. Every one of these transitions is terminal within the replay window.
Emulator: the check never gets a verdict — it is killed at the harness ceiling (60 s) with no terminal status, i.e. the stack/rollback state machine does not settle (the preceding issue 20261025T015306Z-cloudformation-long-running-operations-never-converge-23-behavioral.md is marked FIXED, and a resolved issue does not cover this failure; convergence still does not happen on this recovery path).
Why: a client driving the documented recovery of a stuck rollback can neither observe the refusal-by-re-entering-UPDATE_ROLLBACK_FAILED nor the successful skip, so stacks stranded in UPDATE_ROLLBACK_FAILED cannot be recovered or even diagnosed.
Hypothesis: the non-convergence is in the update-rollback workflow rather than the request handler, since the calls themselves are accepted; which stage stalls (the WaitCondition timeout, the denied in-place resource restore, or the ContinueUpdateRollback resume) is not stated by the replay output.
Fix: the UpdateStack/rollback workflow and ContinueUpdateRollback handling in the cloudformation crate (the stage that must drive UPDATE_ROLLBACK_IN_PROGRESS to UPDATE_ROLLBACK_FAILED / UPDATE_ROLLBACK_COMPLETE rather than leaving the stack in flight).
Traces: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip

## Evidence
cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip:
  Error: Check timed out after 60.0s

The check: a stack under a service role updates a queue's VisibilityTimeout and then fails on a WaitCondition timeout while the role is denied sqs:SetQueueAttributes, so the rollback cannot restore the queue and the stack must land UPDATE_ROLLBACK_FAILED; it then asserts ContinueUpdateRollback without ResourcesToSkip returns the stack to UPDATE_ROLLBACK_FAILED and with ResourcesToSkip=[Q] reaches UPDATE_ROLLBACK_COMPLETE, verifying the skipped queue's attributes at SQS itself. No stack state transition completed inside the window.

## Steps to Reproduce
1. Create a stack with a service role, an AWS::SQS::Queue and an AWS::CloudFormation::WaitCondition.
2. UpdateStack changing the queue's VisibilityTimeout with the role denied sqs:SetQueueAttributes and the WaitCondition left unsignalled, so the update fails and the rollback cannot restore the queue.
3. Poll DescribeStacks: the stack must reach UPDATE_ROLLBACK_FAILED. Then ContinueUpdateRollback with no ResourcesToSkip (must settle back in UPDATE_ROLLBACK_FAILED) and with ResourcesToSkip=[<queue logical id>] (must reach UPDATE_ROLLBACK_COMPLETE).
4. Observe that no stack status transition becomes terminal; the replay is killed at the 60 s ceiling.
