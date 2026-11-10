Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe|run#16|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Commit: a11b28e572acb01424c781dc62b11ca84bce3a9b

# pipes: CloudFormation UpdateStack switching an AWS::Pipes::Pipe to a role that cannot read its source ends UPDATE_COMPLETE instead of failing stabilization and rolling back

> _Note: this issue was identified against the code base at commit `a11b28e572acb01424c781dc62b11ca84bce3a9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipe

## Description
This bundle has 1 cause; it must be closed before submitting.

Cause 1 of 1: a CloudFormation stack update that switches a working AWS::Pipes::Pipe onto an execution role which has no read permission on the pipe's source is reported as a successful update. Note the related OPEN issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md (its Cause 4 covers the CREATE path: a stack whose pipe role cannot read its source reaching CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and a CREATE_COMPLETE stack whose pipe does not exist). That issue does NOT cover this cause: its replay key for this very check was a delivery timeout in _await_record, the pipe now does create and deliver, and the surviving divergence is on the UPDATE path — stabilization failure, the UPDATE_FAILED resource event and its reason text, and the rollback that restores the previous role. Those are the CloudFormation AWS::Pipes::Pipe provider's update/stabilize/rollback behaviours, none of which that issue's Cause 4 describes.

AWS:      Recorded behavior only for the CloudFormation resource provider (the Smithy model describes only the pipes control plane). The pipes model does state the terminal states the provider reads: Smithy (com.amazonaws.pipes) PipeState includes UPDATE_FAILED and UPDATE_ROLLBACK_FAILED, and DescribePipe/UpdatePipe return StateReason (PipeStateReason, @length 0..512, pattern ^.*$) alongside CurrentState, i.e. an UpdatePipe is accepted synchronously (CurrentState UPDATING) and the role/source failure surfaces later as a terminal CurrentState plus a human-readable reason. The CFN provider for AWS::Pipes::Pipe stabilizes on that state, so when the new role cannot call sqs:ReceiveMessage on the source the resource update fails: the stack emits exactly ONE UPDATE_FAILED StackEvent for that logical resource, then rolls the stack back to UPDATE_ROLLBACK_COMPLETE, after which the pipe is back on its previous RoleArn, CurrentState RUNNING, and still delivers records. The UPDATE_FAILED event's ResourceStatusReason is:
            Resource handler returned message: "Resource of type 'AWS::Pipes::Pipe' with identifier '<pipe-name>' did not stabilize. Status Reason is Input parameter is invalid from the request due to : The provided customer role does not have permissions to call ReceiveMessage on SQS" (RequestToken: <token>)
          Everything outside the angle brackets is the fixed text of this error class and is required verbatim, including the nested quoting and the "did not stabilize. Status Reason is " join. The variable parts are taken from the request and from state: <pipe-name> is the pipe's Name property from the template, and <token> is the per-request client request token (a per-run value, never part of the requirement); the inner "ReceiveMessage on SQS" names the source-read action the role is missing, i.e. the action for the configured source type.
Emulator: UpdateStack is accepted and the stack settles at UPDATE_COMPLETE — "switching the pipe to a role that cannot read its source ended 'UPDATE_COMPLETE', expected UPDATE_ROLLBACK_COMPLETE". No UPDATE_FAILED StackEvent is emitted for the pipe's logical resource, no ResourceStatusReason of the form above is produced, the pipe is left on the new (unusable) role rather than rolled back to the working one, and no terminal pipes state (UPDATE_FAILED / UPDATE_ROLLBACK_FAILED) or StateReason is ever written. The pipe's update is applied unconditionally: nothing resolves the new RoleArn against the configured Source or checks the role's source-read grant, so there is no stabilization step for the provider to fail on.
Why:      a client that uses stack status as its deployment gate is told a broken deployment succeeded. The pipe is left pointing at a role that cannot read its source, so from that moment the pipe silently stops moving data while both CloudFormation and DescribePipe report health; the rollback that AWS guarantees — previous role restored, pipe RUNNING, delivery resumed — never happens, so there is nothing for the client to fall back to and no reason string telling it which permission is missing. Automated pipelines that branch on UPDATE_ROLLBACK_COMPLETE, and the 'deploy, verify, roll back' pattern generally, become dead code.
Fix:      the CloudFormation AWS::Pipes::Pipe resource provider's update path (resource update + stabilization + rollback) together with the pipes crate's asynchronous update validation and state machine: validate the new RoleArn's trust and its source-read grant against the configured Source during UpdatePipe, drive the pipe to the modeled terminal state with a populated StateReason, have the provider surface that as a single UPDATE_FAILED resource event carrying the reason text above, and roll the stack (and the pipe's RoleArn) back to the pre-update state, leaving the pipe RUNNING and delivering.
Traces:   pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe (the create-side positive control in the same check passes: the stack reaches CREATE_COMPLETE and a record sent before the update is delivered to the target, so this is specifically the update/stabilize/rollback path).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): Total 138/170 passed (81.2%); Mismatched 29, Errored 3, Skipped 0, Unloadable 0. No bootstrap signal — the emulator was serving: pipes 86/96 passed, and in this very check CloudFormation CreateStack/DescribeStacks, IAM CreateRole, SQS SendMessage/ReceiveMessage and pipes CreatePipe all answered 200 and the pre-update record was delivered end-to-end.

Failing check: [mismatch] pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe
  File "check_pipes_deploy_cfn_pipe_update_to_a_role_without_source_read_fails_and_rolls_back_to_the_working_pipe.py", line 45, in run
    assert status == "UPDATE_ROLLBACK_COMPLETE", (
  AssertionError: switching the pipe to a role that cannot read its source ended 'UPDATE_COMPLETE', expected UPDATE_ROLLBACK_COMPLETE

Expected (AWS):
  - DescribeStacks StackStatus after the update: UPDATE_ROLLBACK_COMPLETE
  - DescribeStackEvents: exactly one event with LogicalResourceId = the pipe's logical id and ResourceStatus = UPDATE_FAILED
  - that event's ResourceStatusReason starts with the fixed text:
      Resource handler returned message: "Resource of type 'AWS::Pipes::Pipe' with identifier '<pipe-name>' did not stabilize. Status Reason is Input parameter is invalid from the request due to : The provided customer role does not have permissions to call ReceiveMessage on SQS"
    followed by " (RequestToken: <token>)". <pipe-name> is the Name property of the template's pipe; <token> is a per-run request token and is not part of the requirement.
  - DescribePipe afterwards: RoleArn = the original (working) role, CurrentState = RUNNING, and a record sent to the source after the rollback still reaches the target.

Actual (emulator):
  - StackStatus after the update: UPDATE_COMPLETE (no rollback, no UPDATE_FAILED event, no ResourceStatusReason of that class).
  - The assertion on the stack status fires first, so the role-restored / RUNNING / still-delivers assertions are not reached; the pipe is left on the role that cannot read its source.

Generated stack, queue, role and pipe names, ARNs, account id, region, request tokens and timestamps in this check differ per run and are examples only. The requirements are: a stack update whose AWS::Pipes::Pipe cannot stabilize on its new role ends UPDATE_ROLLBACK_COMPLETE; it emits exactly one UPDATE_FAILED event for that resource with the reason text above; and the rollback restores the pipe's previous RoleArn with CurrentState RUNNING and working delivery.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. CreateStack with Capabilities=[CAPABILITY_IAM] and a template that declares: two AWS::SQS::Queue resources (SourceQueue, TargetQueue); two AWS::IAM::Role resources trusting pipes.amazonaws.com, where GoodRole grants sqs:SendMessage on TargetQueue plus sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on SourceQueue and BadRole grants only sqs:SendMessage on TargetQueue; and an AWS::Pipes::Pipe (logical id ThePipe) with Source = SourceQueue ARN, SourceParameters.SqsQueueParameters.BatchSize = 1, Target = TargetQueue ARN, DesiredState = RUNNING and RoleArn selected by a String parameter PipeRole (AllowedValues good|bad) via Fn::If. Pass PipeRole=good.
3. Wait for CREATE_COMPLETE, then SendMessage to the source queue URL and poll ReceiveMessage on the target queue until the pipe-delivered record arrives (positive control; this passes today).
4. UpdateStack with the same TemplateBody, Capabilities=[CAPABILITY_IAM] and Parameters=[{PipeRole: bad}], then poll DescribeStacks until StackStatus is not *_IN_PROGRESS.
5. Observe StackStatus = UPDATE_COMPLETE instead of UPDATE_ROLLBACK_COMPLETE.
6. Paginate DescribeStackEvents for the stack and filter LogicalResourceId == "ThePipe" and ResourceStatus == "UPDATE_FAILED": observe zero such events (AWS produces exactly one, whose ResourceStatusReason begins with the 'did not stabilize. Status Reason is Input parameter is invalid from the request due to : The provided customer role does not have permissions to call ReceiveMessage on SQS' text).
7. Call pipes DescribePipe on the template's pipe name and observe RoleArn is the BadRole ARN rather than the restored GoodRole ARN.
8. After fixing, re-replay pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe.
