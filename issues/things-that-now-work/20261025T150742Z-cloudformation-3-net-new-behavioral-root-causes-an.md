Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip cloudformation.deep-cloudformation-2-deployment-mode-express cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip|run#22|AssertionError|assert st["StackStatus"] == "UPDATE_ROLLBACK_FAILED", (
Issue-Key: cloudformation.deep-cloudformation-2-deployment-mode-express|_failed_create#3|AssertionError|assert ("B", "CREATE_FAILED") in events, (
Issue-Key: cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact|run#25|AssertionError|assert companion_got == companion_expected, (
Issue-Commit: d2e421bd781628705310382509b1712cb5427402

# cloudformation: 3 net-new behavioral root causes — an unsignalled WaitCondition never times out (failed update ends UPDATE_COMPLETE), a duplicate SSM parameter name never fails its resource, and ParameterNotFound carries an emulator-authored message

> _Note: this issue was identified against the code base at commit `d2e421bd781628705310382509b1712cb5427402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Three distinct net-new root causes, all grounded in cloudformation.* behavioral checks that are NOT covered by any already-filed issue (the replay report marked neither ALREADY FILED nor OWNED for them). Related open issues exist but none carries these checks' replay keys: 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md, 20261025T015306Z-cloudformation-long-running-operations-never-converge-23-behavioral.md and 20261025T053459Z-cloudformation-emits-no-per-resource-stack-events-and.md describe neighbouring symptoms; the three causes below are separate and must each be closed.

=== Cause 1 of 3: AWS::CloudFormation::WaitCondition never times out, so an update that must fail reaches UPDATE_COMPLETE and the UPDATE_ROLLBACK_FAILED / ContinueUpdateRollback recovery state machine is unreachable ===
AWS: Smithy: UpdateStack / ContinueUpdateRollback(StackName, RoleARN, ResourcesToSkip: list<ResourceToSkip>, ForceRollback); StackStatus enum includes UPDATE_ROLLBACK_FAILED and UPDATE_ROLLBACK_COMPLETE. An AWS::CloudFormation::WaitCondition with Properties.Timeout="90" and Count=1 that is never signalled must fail its resource after the timeout elapses, which fails the stack update. When the stack's service role is then denied the permission needed to restore an already-mutated resource (sqs:SetQueueAttributes on the queue), the rollback cannot complete and the stack settles in UPDATE_ROLLBACK_FAILED with the queue's resource status UPDATE_FAILED and its VisibilityTimeout left at the updated value. ContinueUpdateRollback WITHOUT ResourcesToSkip is accepted (200, empty output) but re-enters UPDATE_ROLLBACK_FAILED while the deny stands; ContinueUpdateRollback WITH ResourcesToSkip=["Q"] converges to UPDATE_ROLLBACK_COMPLETE and leaves the skipped resource exactly as the failed update made it (it is NOT reverted).
Emulator: the update, whose template adds an unsignalled WaitCondition/WaitConditionHandle depending on the queue, settles in UPDATE_COMPLETE (StackStatusReason None). The timeout is never honoured, so nothing in the recovery path is ever exercised: no UPDATE_ROLLBACK_FAILED, no UPDATE_FAILED resource status, and ContinueUpdateRollback's ResourcesToSkip semantics are untested.
Why: a client that models a provisioning barrier with a WaitCondition is told its deployment succeeded when the barrier never fired, and any recovery tooling built on UPDATE_ROLLBACK_FAILED + ContinueUpdateRollback(ResourcesToSkip=[...]) can never be rehearsed against the emulator — the state it keys on does not exist.
Fix: the stack-operation state machine / provisioning engine for AWS::CloudFormation::WaitCondition (timeout accounting against signals received) and the rollback path that must mark the mutated resource UPDATE_FAILED and park the stack in UPDATE_ROLLBACK_FAILED, plus ContinueUpdateRollback's ResourcesToSkip handling.
Traces: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip (assertion: "after the denied rollback the stack is 'UPDATE_COMPLETE' (None), expected UPDATE_ROLLBACK_FAILED").

=== Cause 2 of 3: two AWS::SSM::Parameter resources with the SAME Name both reach CREATE_COMPLETE — a runtime provisioning collision is never detected, so DeploymentConfig.Mode's failure semantics are unobservable ===
AWS: Smithy: CreateStack.DeploymentConfig -> DeploymentConfig{Mode: DeploymentConfigMode, DisableRollback: boolean}; Mode STANDARD "ensures resources are ready to serve traffic before completing the operation", Mode EXPRESS "complete the stack operation when resource configuration is applied". A template declaring resource A (AWS::SSM::Parameter, Name=<param>) and resource B (AWS::SSM::Parameter, Name={"Ref": "A"}) makes B request the SAME parameter name as A, so B's create fails at the SSM call only after A is CREATE_COMPLETE. Real AWS records A -> CREATE_COMPLETE and B -> CREATE_FAILED in the stack's event history; under EXPRESS the stack stops at CREATE_FAILED and keeps what was created, under STANDARD (and under EXPRESS with DeploymentConfig.DisableRollback=false) it rolls back to ROLLBACK_COMPLETE and the already-created parameter is destroyed.
Emulator: the event history is A CREATE_IN_PROGRESS / A CREATE_COMPLETE, B CREATE_IN_PROGRESS / B CREATE_COMPLETE, stack CREATE_IN_PROGRESS / CREATE_COMPLETE. B never fails: the name collision between the two parameters is not detected, so no CREATE_FAILED event is ever emitted and all three DeploymentConfig arms behave identically.
Why: a client cannot distinguish EXPRESS from STANDARD deployment behaviour, and more fundamentally a template that cannot possibly provision is reported as a successful deployment — downstream state (whether the surviving parameter still exists) is the opposite of AWS's for two of the three arms.
Fix: the resource provisioner for AWS::SSM::Parameter must issue the create against SSM with the same uniqueness rule SSM enforces (a second parameter claiming an existing Name must be refused) and the stack engine must turn that refusal into a CREATE_FAILED resource event and the mode-dependent terminal stack status.
Traces: cloudformation.deep-cloudformation-2-deployment-mode-express (assertion: "[express] B's designed runtime failure did not happen; events=[('A','CREATE_COMPLETE'),('A','CREATE_IN_PROGRESS'),('B','CREATE_COMPLETE'),('B','CREATE_IN_PROGRESS'),(<stack>,'CREATE_COMPLETE'),(<stack>,'CREATE_IN_PROGRESS')]").

=== Cause 3 of 3: ssm GetParameter's ParameterNotFound carries an emulator-authored message where AWS sends none ===
AWS: Smithy: ssm ParameterNotFound is a client error with an OPTIONAL `message` member (httpError 400, awsQueryError code ParameterNotFound); the model documents the error as "The parameter couldn't be found. Verify the name and try again." but does not prescribe a wire message. Recorded behavior only: for GetParameter on a name that does not exist, AWS answers error code `ParameterNotFound` with an EMPTY message — there is no fixed text and no variable part. The check asserts exactly ('ParameterNotFound', '').
Emulator: the same call answers code `ParameterNotFound` with message `Parameter <Name> not found`, i.e. the fixed text "Parameter " + the request's Name + " not found". The error code is right; the message is emulator-authored prose AWS never sends.
Why: clients and test suites that assert on the full error payload (or that log/compare error messages, as this check's positive control does) see a message where AWS sends none, so an exact-match assertion on a legitimate AWS response fails against the emulator.
Fix: the GetParameter handler's not-found path (the ssm service crate's ParameterNotFound construction) must emit ParameterNotFound with no message for GetParameter rather than synthesizing one. This is NOT the same defect as 20261024T190718Z-cloudformation-secretsmanager-ssm-ssm-getparameter-never-resolves-aws-reference-secretsmanager-so.md, which is about /aws/reference/secretsmanager resolution.
Traces: cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact (assertion: "ssm:GetParameter returned ('ParameterNotFound', 'Parameter /sim2real-cfn-retain-companion-... not found'), expected ('ParameterNotFound', '')").

## Evidence
Cause 1 — cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip
  Expected: DescribeStacks StackStatus == UPDATE_ROLLBACK_FAILED after the service role is denied sqs:SetQueueAttributes and the unsignalled WaitCondition (Timeout 90, Count 1) fails the update; queue VisibilityTimeout stays 60; DescribeStackResources status for 'Q' == UPDATE_FAILED; ContinueUpdateRollback() re-enters UPDATE_ROLLBACK_FAILED; ContinueUpdateRollback(ResourcesToSkip=['Q']) -> UPDATE_ROLLBACK_COMPLETE with the queue still at 60.
  Actual: AssertionError: after the denied rollback the stack is 'UPDATE_COMPLETE' (None), expected UPDATE_ROLLBACK_FAILED.

Cause 2 — cloudformation.deep-cloudformation-2-deployment-mode-express
  Expected: DescribeStackEvents for the stack contains ('A','CREATE_COMPLETE') AND ('B','CREATE_FAILED'); then under DeploymentConfig={'Mode':'EXPRESS'} the stack settles CREATE_FAILED with A's parameter still present, under STANDARD it settles ROLLBACK_COMPLETE with the parameter gone.
  Actual: AssertionError: [express] B's designed runtime failure did not happen; events=[('A','CREATE_COMPLETE'), ('A','CREATE_IN_PROGRESS'), ('B','CREATE_COMPLETE'), ('B','CREATE_IN_PROGRESS'), ('sim2real-behav-dmode-express-<id>','CREATE_COMPLETE'), ('sim2real-behav-dmode-express-<id>','CREATE_IN_PROGRESS')] — both same-named parameters created successfully.

Cause 3 — cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact
  Expected: ssm:GetParameter(Name=<deleted companion parameter>) -> ClientError (Code='ParameterNotFound', Message='')
  Actual: ClientError (Code='ParameterNotFound', Message='Parameter /sim2real-cfn-retain-companion-<id> not found')
  (the <id> suffix is a per-run generated name; the divergence is the presence of the message template "Parameter <Name> not found" at all, not its value)

## Steps to Reproduce
Cause 1:
1. iam:CreateRole with a cloudformation.amazonaws.com trust policy and an inline policy allowing sqs:* on arn:aws:sqs:<region>:<account>:<stack-name> plus sqs:ListQueues on *.
2. cfn:CreateStack(StackName=S, RoleARN=role, TemplateBody={Resources:{Q:{Type:AWS::SQS::Queue, Properties:{QueueName:S, VisibilityTimeout:30, SqsManagedSseEnabled:true}}}}) and wait for CREATE_COMPLETE.
3. cfn:UpdateStack(StackName=S, RoleARN=role, TemplateBody=same but VisibilityTimeout:60 and additionally H:{Type:AWS::CloudFormation::WaitConditionHandle} and W:{Type:AWS::CloudFormation::WaitCondition, DependsOn:Q, Properties:{Handle:{Ref:H}, Timeout:"90", Count:1}}). Never signal the handle.
4. Once the queue has been updated, iam:PutRolePolicy on the same role with {Effect:Deny, Action:sqs:SetQueueAttributes, Resource:*}.
5. Poll cfn:DescribeStacks until terminal. Observe UPDATE_COMPLETE; AWS gives UPDATE_ROLLBACK_FAILED.

Cause 2:
1. cfn:CreateStack(StackName=S, DeploymentConfig={"Mode":"EXPRESS"}, TemplateBody={Resources:{A:{Type:AWS::SSM::Parameter,Properties:{Name:"/p",Type:"String",Value:"resource-a"}}, B:{Type:AWS::SSM::Parameter,Properties:{Name:{"Ref":"A"},Type:"String",Value:"resource-b"}}}}).
2. Poll cfn:DescribeStacks until terminal, then page cfn:DescribeStackEvents.
3. Observe B CREATE_COMPLETE and stack CREATE_COMPLETE; AWS gives B CREATE_FAILED and the stack CREATE_FAILED (EXPRESS) / ROLLBACK_COMPLETE (STANDARD).

Cause 3:
1. ssm:GetParameter(Name="/does-not-exist").
2. Inspect the error payload: Code is ParameterNotFound (correct) but Message is "Parameter /does-not-exist not found"; AWS returns an empty message.
