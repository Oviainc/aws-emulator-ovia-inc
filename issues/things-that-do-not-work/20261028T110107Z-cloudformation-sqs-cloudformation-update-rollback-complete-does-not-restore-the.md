Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back
Issue-Key: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back|run#23|AssertionError|assert params["VisibilityTimeout"] == "30", (
Issue-Commit: 48883a331e1fae405ebe84b115d9b023bc18cbad

# CloudFormation UPDATE_ROLLBACK_COMPLETE does not restore the stack's Parameters: DescribeStacks keeps the failed update's rejected parameter value

> _Note: this issue was identified against the code base at commit `48883a331e1fae405ebe84b115d9b023bc18cbad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: after an UpdateStack that fails and rolls back, the stored stack keeps the FAILED update's parameter values, so DescribeStacks reports a stack configuration that was never deployed.
Failing check id: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back

AWS: UpdateStack's contract is that a failed update is rolled back to the stack's PREVIOUS configuration - template, parameters and resources - and the stack lands in UPDATE_ROLLBACK_COMPLETE. After that terminal status DescribeStacks reports Stacks[0].Parameters as the pre-update values, i.e. the values of the last successfully deployed revision; the values the rejected UpdateStack submitted are discarded with the rest of that revision. Smithy (com-amazonaws-cloudformation-cloudformation): Stack.Parameters is documented only as "A list of Parameter structures" and UpdateStack.Parameters as "A list of Parameter structures that specify input parameters for the stack" - the model states no rollback rule, so the requirement is AWS's documented UpdateStack rollback semantics ("If the stack update fails, CloudFormation rolls back the stack to the previous stack configuration"), which this emulator's own handler module doc restates verbatim (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack.rs, header: "the update is then ROLLED BACK to the previous stack configuration ... which lands the stack in UPDATE_ROLLBACK_COMPLETE"). Recorded behavior confirms the rest of the contract is met: the update was rejected by the owning service, the stack reached UPDATE_ROLLBACK_COMPLETE, and the resource side was correctly put back.

Emulator: the rollback is only half done. For a stack with a Number parameter wired into AWS::SQS::Queue VisibilityTimeout, an UpdateStack that pushes the parameter past the SQS limit (a value above 43200) behaves correctly up to and including the resource restore - the stack status is UPDATE_ROLLBACK_COMPLETE, an UPDATE_FAILED event is recorded for the queue's logical id, the queue's physical resource id is unchanged, the queue still resolves to its original URL, and GetQueueAttributes reports the pre-update VisibilityTimeout. But DescribeStacks still reports the stack parameter at the REJECTED value: Stacks[0].Parameters contains ParameterKey=VisibilityTimeout with the value the failed update submitted instead of the pre-update value. The replay assertion reads: "stack parameter VisibilityTimeout is '50000' after rollback, expected the pre-update '30'" (50000 is the request's submitted value and 30 the pre-update value; both are values from the request/state, not fixed text).

Why: the stack's Parameters list is how a client learns what configuration is actually in force. A client relying on AWS's behavior reads back a configuration that was never deployed and that contradicts the real resource state: a follow-up UpdateStack using UsePreviousValue=true for that parameter resolves to the rejected value and re-submits it, a CreateChangeSet/drift comparison is planned against the never-deployed revision, and any reconciliation loop that compares desired-vs-reported parameters sees a stack that claims the bad value while its resources hold the good one. The divergence is silent - the stack status says the rollback succeeded.

Fix: CloudFormation's UpdateStack rollback settle path. The handler snapshots the whole pre-update configuration (PreviousValues in services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack.rs captures template_body, processed_template, included_template_body, parameters, tags, capabilities, role_arn, ...), but the rollback it hands to state only carries the resource lists (crate::state::OwedRollback { previous, applied }), so when the rollback settles (crate::state::settle_due_rollbacks) nothing puts the stored stack's parameters - nor its template/processed stage - back. The rollback must restore the captured PreviousValues configuration onto the stored stack, not only the resources, and the DescribeStacks serializer should then read the restored values. Related but distinct open issue: 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md covers failed stacks that never roll back at all; here the rollback DOES run and the resources ARE restored, and what is left behind is the stack's own parameter record.

Traces: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back (behavioral check; the suite reports it as a mismatch, failing at the DescribeStacks Parameters assertion after UPDATE_ROLLBACK_COMPLETE).

## Evidence
Check: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back [mismatch]

Scenario: a stack with Parameters {QueueName, VisibilityTimeout:Number} and one AWS::SQS::Queue whose VisibilityTimeout is {Ref: VisibilityTimeout}. Created with VisibilityTimeout=30 (CREATE_COMPLETE), then UpdateStack with VisibilityTimeout=50000, which SQS refuses (it caps VisibilityTimeout at 43200).

Expected (AWS): after the rollback the stack is back on its previous configuration -
  - StackStatus == UPDATE_ROLLBACK_COMPLETE
  - an UPDATE_FAILED stack event for the queue's logical id
  - the queue's PhysicalResourceId and URL unchanged
  - sqs:GetQueueAttributes VisibilityTimeout == the pre-update value
  - cfn:DescribeStacks Stacks[0].Parameters[VisibilityTimeout].ParameterValue == the PRE-UPDATE value
  - the message sent before the rejected update is still receivable

Actual (emulator): every expectation above holds EXCEPT the stack's own parameter view. The replay assertion that fired:
    assert params["VisibilityTimeout"] == "30"
    AssertionError: stack parameter VisibilityTimeout is '50000' after rollback, expected the pre-update '30'
where params is built from cfn.describe_stacks(StackName=...)["Stacks"][0]["Parameters"]. The emulator therefore reports the rejected update's parameter value as the stack's current parameter value, while the resource it feeds holds the pre-update value - the stack record and the resource state disagree.

Code pointers for the cause: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack.rs captures the pre-update snapshot in PreviousValues::of (including `parameters: stack.parameters.clone()`), but the rollback handed to state is `crate::state::OwedRollback { previous: previous_resources.clone(), applied: materialised.resources.clone() }` (~line 987) - resources only; no parameter/template restore is scheduled, and crate::state::settle_due_rollbacks has nothing to put back.

## Steps to Reproduce
1. Clone the repo and boot the emulator (git_clone, then run_behavioral_suite).
2. Replay the single check: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back

Or by hand against the emulator endpoint:
1. cloudformation:CreateStack with a template declaring Parameters {QueueName: String, VisibilityTimeout: Number} and Resources {Queue: AWS::SQS::Queue with QueueName={Ref:QueueName}, VisibilityTimeout={Ref:VisibilityTimeout}} and Outputs {QueueUrl: {Ref: Queue}}. Pass VisibilityTimeout=30. Wait for CREATE_COMPLETE.
2. Record the pre-update state: DescribeStackResource(LogicalResourceId="Queue").PhysicalResourceId, and sqs:GetQueueAttributes(AttributeNames=["VisibilityTimeout"]) == "30".
3. cloudformation:UpdateStack with the SAME TemplateBody and VisibilityTimeout=50000 (above the SQS maximum of 43200, and the parameter declares no MinValue/MaxValue so UpdateStack itself is accepted and the refusal comes from the SQS resource).
4. Poll DescribeStacks until the stack is terminal; observe StackStatus == UPDATE_ROLLBACK_COMPLETE and an UPDATE_FAILED event for logical id "Queue" in DescribeStackEvents.
5. Observe the resource side is correctly restored: sqs:GetQueueAttributes VisibilityTimeout == "30" and the physical resource id is unchanged.
6. BUG: read DescribeStacks(StackName=...)["Stacks"][0]["Parameters"] - the VisibilityTimeout entry still carries the rejected update's value ("50000") instead of the pre-update "30".
7. Consequence to confirm: issue a further UpdateStack passing that parameter with UsePreviousValue=true; it resolves to the rejected value rather than the deployed one.
