Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored|_wait_snapshot_available#56|Timeout|time.sleep(15)
Issue-Commit: de2fdfddb99280a454e9752c3aabdc5314d728a2

# rds: the protected-stack deletion-policy deploy flow never converges — the check exhausts the 60 s harness cap before any DeletionPolicy consequence is observable

> _Note: this issue was identified against the code base at commit `de2fdfddb99280a454e9752c3aabdc5314d728a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
One net-new root cause, grounded in the single surviving failure of this replay (every other failing check in this run is marked ALREADY FILED or OWNED).

Cause 1 of 1: the RDS deletion-policy deploy flow (CreateStack -> CREATE_COMPLETE -> DeleteStack on a DeletionProtection=true DBInstance -> DELETE_FAILED -> lift protection -> DeleteStack -> DELETE_COMPLETE -> DeletionPolicy Snapshot/Retain artifacts) never reaches the terminal stack state the flow needs inside the replay window. The check `rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored` ends with `Check timed out after 60.0s` and produces NO response assertion at all, so none of the deletion-policy consequences are observable.

AWS: recorded behavior only for the timing; the state machine the flow depends on is modelled. Smithy (com-amazonaws-cloudformation-cloudformation): `StackStatus` enumerates CREATE_IN_PROGRESS / CREATE_COMPLETE / DELETE_IN_PROGRESS / DELETE_FAILED / DELETE_COMPLETE among its terminal and in-progress values, and DeleteStack documents "Once the call completes successfully, stack deletion starts" — i.e. every `*_IN_PROGRESS` status is transient and the stack settles into a terminal status (CREATE_COMPLETE, DELETE_FAILED, DELETE_COMPLETE) that DescribeStacks reports, which is exactly what the check polls for. On real AWS the same recorded flow settles through CREATE_COMPLETE, then DELETE_FAILED (the protected DBInstance refuses deletion, StackStatusReason `The following resource(s) failed to delete: [Db]. `), then DELETE_COMPLETE, and leaves the DeletionPolicy Snapshot final snapshot plus the DeletionPolicy Retain DBParameterGroup behind.

Emulator: the replay reports only `Error: Check timed out after 60.0s` for this check — the check never returns from one of its DescribeStacks / DescribeDBInstances wait loops (`_wait_stack(..., 'CREATE_COMPLETE')`, `_wait_stack(..., 'DELETE_FAILED')`, `_wait_instance(..., protect=False)`, `_wait_stack(..., 'DELETE_COMPLETE')`, `_wait_snapshot_available`), all of which poll a non-terminal state and only exit on the wanted terminal status, a terminal-but-different status, or their own long timeout. The replay output does not identify WHICH of those waits stalls, so the stage is not attributable from the output alone: the observable divergence is that the flow does not converge, where the recorded flow does. Note this is a DIFFERENT failure mode from the one already filed for the same check: open issue 20261027T044602Z-rds-cloudformation-updatestack-accepts-a-stack-in-delete-failed.md records that UpdateStack on a DELETE_FAILED stack is accepted instead of refused with 400 ValidationError `Stack:<stack-id> is in DELETE_FAILED state and can not be updated.` (fixed text `Stack:`, ` is in `, ` state and can not be updated.`; variable parts the resolved stack id and the current StackStatus). That issue was authored from a run in which every assertion up to the UpdateStack call PASSED; in this run the check no longer reaches that call at all, so that issue does not cover this convergence failure, and fixing only the UpdateStack precondition will leave this check timing out.

Why: a client that deploys RDS through CloudFormation drives exactly this loop — submit, then poll DescribeStacks until a terminal StackStatus. If a stack lifecycle step never settles, every such client hangs until its own timeout and can never tell a refused delete (DELETE_FAILED, DB intact, protection still on) from a slow one, cannot discover that protection must be lifted on the instance itself, and never gets to see that DeletionPolicy Snapshot produced a manual snapshot of the deleted instance or that DeletionPolicy Retain kept the DBParameterGroup with its template parameter value. Any automation keyed on stack waiters (CloudFormation waiters, CDK, `aws cloudformation deploy`) is unusable against this flow.

Fix: the stack-lifecycle settle path shared by the CloudFormation create/delete handlers and the RDS resource provisioner — the CloudFormation stack store's settle/transition logic (services/service-com-amazonaws-cloudformation-cloudformation/src/state/, driven from src/ops/create_stack.rs and src/ops/delete_stack.rs, whose resource calls go through src/state/provisioning.rs) together with the RDS DBInstance state machine it drives (services/service-com-amazonaws-rds-amazonrdsv19/src/) must make every `*_IN_PROGRESS` stack status and every non-`available` DBInstance status advance to a terminal status promptly, so DescribeStacks and DescribeDBInstances report CREATE_COMPLETE, DELETE_FAILED (with the protected instance's refusal as the reason), DELETE_COMPLETE and `available` without an unbounded wait.

Traces: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored (the only failing check in this replay not marked ALREADY FILED or OWNED).

## Evidence
Expected (recorded flow, real AWS us-west-2): CreateStack(VPC + 2 Subnets + SecurityGroup + AWS::RDS::DBSubnetGroup + AWS::RDS::DBParameterGroup[DeletionPolicy Retain] + AWS::RDS::DBInstance[mysql, DeletionPolicy Snapshot, DeletionProtection=true]) settles to StackStatus=CREATE_COMPLETE; DeleteStack then settles to StackStatus=DELETE_FAILED with StackStatusReason "The following resource(s) failed to delete: [Db]. " and a Db DELETE_FAILED stack event whose ResourceStatusReason cites deletion protection, while DescribeDBInstances keeps reporting DBInstanceStatus=available / DeletionProtection=true; after ModifyDBInstance(DeletionProtection=false) the stack settles to DELETE_COMPLETE, DescribeDBInstances answers DBInstanceNotFound "DBInstance <id> not found.", one manual snapshot of the instance is available, and the retained DBParameterGroup still reports max_connections=77.

Actual (emulator): the check produces no assertion result at all. Replay output for the check, verbatim:

  #### [mismatch] rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
  Error: Check timed out after 60.0s

Every one of the check's polling helpers (`_wait_stack` for CREATE_COMPLETE / DELETE_FAILED / DELETE_COMPLETE, `_wait_instance`, `_wait_snapshot_available`) sleeps 10-15 s between DescribeStacks / DescribeDBInstances / DescribeDBSnapshots calls and returns only on the wanted terminal state, so a stack or instance left in a non-terminal state consumes the whole 60 s cap. Smithy (com-amazonaws-cloudformation-cloudformation) StackStatus: CREATE_IN_PROGRESS, CREATE_COMPLETE, DELETE_IN_PROGRESS, DELETE_FAILED, DELETE_COMPLETE (among others) — the in-progress values are transient by definition.

Related open issue that does NOT cover this: 20261027T044602Z-rds-cloudformation-updatestack-accepts-a-stack-in-delete-failed.md (same check, but its cause is the missing UpdateStack DELETE_FAILED precondition, reached only after the waits this run never completes).

## Steps to Reproduce
1. rds.describe_db_engine_versions(Engine='mysql', DefaultOnly=True) and rds.describe_orderable_db_instance_options(Engine='mysql', EngineVersion=<default>, DBInstanceClass='db.t3.micro', Vpc=True) to pick two AZs.
2. cfn.create_stack with a template declaring AWS::EC2::VPC, two AWS::EC2::Subnet, AWS::EC2::SecurityGroup, AWS::RDS::DBSubnetGroup, AWS::RDS::DBParameterGroup (DeletionPolicy/UpdateReplacePolicy Retain, Parameters {max_connections: '77'}) and AWS::RDS::DBInstance (Engine mysql, DBInstanceClass db.t3.micro, AllocatedStorage 20, DeletionPolicy/UpdateReplacePolicy Snapshot, DeletionProtection=true, BackupRetentionPeriod 0), plus a NoEcho DbPassword parameter.
3. Poll cfn.describe_stacks(StackName=<stack-id>) every 15 s for StackStatus=CREATE_COMPLETE.
4. cfn.delete_stack(StackName=<stack-id>) and poll describe_stacks for StackStatus=DELETE_FAILED, polling rds.describe_db_instances(<instance>) on each iteration.
5. rds.modify_db_instance(DeletionProtection=False, ApplyImmediately=True), wait for DBInstanceStatus=available with DeletionProtection=false, cfn.delete_stack again and poll for DELETE_COMPLETE; then look for the manual final snapshot and the retained parameter group.
6. Observe: against the emulator the sequence never reaches the next terminal state in time — the replay harness kills the check at 60 s with `Check timed out after 60.0s` and no step of 3-6 ever reports a terminal StackStatus, whereas the recorded AWS flow settles through CREATE_COMPLETE, DELETE_FAILED, DELETE_COMPLETE and leaves the Snapshot/Retain artifacts behind.
