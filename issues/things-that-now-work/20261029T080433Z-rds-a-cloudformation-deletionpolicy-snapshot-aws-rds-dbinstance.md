Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored|_wait_snapshot_available#56|TimeoutError|raise TimeoutError(
Issue-Commit: 9aa9958003004f44d89dbc31abda0f8624986a4f

# rds: a CloudFormation DeletionPolicy=Snapshot AWS::RDS::DBInstance leaves NO final DB snapshot behind after the stack reaches DELETE_COMPLETE (1 cause)

> _Note: this issue was identified against the code base at commit `9aa9958003004f44d89dbc31abda0f8624986a4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDBInstance

## Description
One net-new root cause, grounded in the one surviving failure of this replay for service `rds` (every other `rds` failure in this run is marked ALREADY FILED or OWNED).

Cause 1 of 1: a CloudFormation stack deleting an `AWS::RDS::DBInstance` carrying `DeletionPolicy: Snapshot` produces no final DB snapshot. Failing check: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.

AWS: a DeletionPolicy of `Snapshot` makes CloudFormation delete the DB instance by calling DeleteDBInstance WITHOUT SkipFinalSnapshot and WITH a FinalDBSnapshotIdentifier that CloudFormation derives from the stack/logical id, so a manual snapshot of the instance survives the stack. Smithy (com-amazonaws-rds-amazonrdsv19): DeleteDBInstance input has `SkipFinalSnapshot` (boolean) and `FinalDBSnapshotIdentifier` (String), and errors DBSnapshotAlreadyExistsFault / SnapshotQuotaExceededFault — i.e. the snapshot is minted by the delete itself. DescribeDBSnapshots takes `DBInstanceIdentifier` and `SnapshotType` filters and returns DBSnapshot records whose `SnapshotType` ("Provides the type of the DB snapshot.") is `manual` for a final snapshot and whose `Status` reaches `available`; after the instance is gone the snapshot is still listable by its DBInstanceIdentifier. Recorded behavior only for the snapshot's id shape: AWS names the final snapshot with a prefix derived from the stack name, and the check grades only `startswith(<stack name>)`, `SnapshotType == "manual"`, `Engine == "mysql"` — never a literal recorded id.

Emulator: the whole earlier flow now converges — the stack reaches CREATE_COMPLETE, the DeletionProtection=true delete settles DELETE_FAILED naming [Db], UpdateStack on the DELETE_FAILED stack is refused ValidationError, the second delete reaches DELETE_COMPLETE, and DescribeDBInstances answers DBInstanceNotFound `DBInstance <id> not found.` — and then DescribeDBSnapshots(DBInstanceIdentifier=<deleted instance>, SnapshotType="manual") keeps returning an EMPTY DBSnapshots list for the rest of the run. The check's poll (`_wait_snapshot_available`, 2 s interval, only exits on a snapshot whose Status is `available`) therefore never exits and the run budget expires: `TimeoutError: final snapshot of 'sim2real-rds-dp-<id>-db-<id>' available: the run budget ended first; not graded`. The assertion it would have reached is `no 'available' manual snapshot for deleted instance '<id>' within <N>s: []`. Hypothesis: the CloudFormation RDS provisioner deletes the instance unconditionally with SkipFinalSnapshot semantics and never reads the resource's DeletionPolicy, so no DBSnapshot is inserted into the RDS store (the sibling DeletionPolicy on this same stack, `Retain` on the DBParameterGroup, is never graded either because the check stops at the snapshot wait).

Why: a client that uses DeletionPolicy=Snapshot does so precisely to keep the data after the stack is torn down. Against the emulator the stack reports a clean DELETE_COMPLETE and the data is silently gone: no snapshot to restore from, RestoreDBInstanceFromDBSnapshot has nothing to name, and any teardown/rebuild automation that asserts the retained snapshot (or reaps it afterwards) hangs or fails.

Fix: the CloudFormation RDS DBInstance provisioning/deprovisioning path and the RDS snapshot state it drives — the stack delete must honour the resource's DeletionPolicy/UpdateReplacePolicy of `Snapshot` by taking a final snapshot (an id derived from the stack name, SnapshotType `manual`, Engine carried over from the instance, Status settling to `available`) that DescribeDBSnapshots lists under the deleted instance's DBInstanceIdentifier, and must likewise honour `Retain` on the DBParameterGroup.

Related open issue that does NOT cover this: 20261027T064130Z-rds-the-protected-stack-deletion-policy-deploy-flow.md records that this same check never converged at all (the earlier stack/instance waits never settled, `Check timed out after 60.0s`, nothing graded). Those waits now all settle; the only remaining divergence is the missing final snapshot, so fixing the convergence of the stack lifecycle alone leaves this check failing.

Traces: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.

## Evidence
Expected (real AWS, recorded flow): after the stack reaches DELETE_COMPLETE and DescribeDBInstances answers DBInstanceNotFound `DBInstance <instance-id> not found.`, DescribeDBSnapshots(DBInstanceIdentifier=<instance-id>, SnapshotType='manual') returns exactly ONE DBSnapshot: DBSnapshotIdentifier starting with the stack name, SnapshotType 'manual', Engine 'mysql', Status 'available'; and DescribeDBParameterGroups still lists the DeletionPolicy=Retain group with DBParameterGroupFamily=<family> and user parameter max_connections=77.

Actual (emulator): DescribeDBSnapshots for the deleted instance returns an empty list indefinitely. Replay output for the check, verbatim:

  #### [error] rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
  Error: File "check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py", line 158, in run
  snap = _wait_snapshot_available(rds, instance_id, timeout_s=_budget_s(ctx, 600), ctx=ctx)
  File "check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py", line 366, in _wait_snapshot_available
  raise TimeoutError(
  TimeoutError: final snapshot of 'sim2real-rds-dp-64d972ebbd61-db-8fb4b8f3cac6' available: the run budget ended first; not graded

Every assertion BEFORE that line passed in this run (NoEcho masking, DeletionProtection=true, DELETE_FAILED with StackStatusReason 'The following resource(s) failed to delete: [Db]. ', the Db DELETE_FAILED event citing deletion protection, the ValidationError 'Stack:<stack-id> is in DELETE_FAILED state and can not be updated.', DELETE_COMPLETE, and DBInstanceNotFound 'DBInstance <id> not found.'), which isolates the divergence to the DeletionPolicy=Snapshot artifact. The snapshot id, instance id and stack name above are per-run values that only fill the template; the requirement is the snapshot's existence, its stack-name-derived prefix, SnapshotType 'manual', inherited Engine and Status 'available'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. rds: DescribeDBEngineVersions(Engine='mysql', DefaultOnly=true) and DescribeOrderableDBInstanceOptions to pick two AZs.
3. cloudformation: CreateStack with VPC + 2 Subnets + SecurityGroup + AWS::RDS::DBSubnetGroup + AWS::RDS::DBParameterGroup (DeletionPolicy/UpdateReplacePolicy Retain, Parameters {max_connections: '77'}) + AWS::RDS::DBInstance (Engine mysql, db.t3.micro, AllocatedStorage 20, BackupRetentionPeriod 0, DeletionProtection=true, DeletionPolicy/UpdateReplacePolicy Snapshot). Wait for CREATE_COMPLETE.
4. cloudformation: DeleteStack; wait for DELETE_FAILED. rds: ModifyDBInstance(DeletionProtection=false, ApplyImmediately=true). cloudformation: DeleteStack again; wait for DELETE_COMPLETE.
5. rds: DescribeDBInstances(<instance-id>) -> DBInstanceNotFound (as expected).
6. rds: DescribeDBSnapshots(DBInstanceIdentifier=<instance-id>, SnapshotType='manual') repeatedly. Observe: DBSnapshots is always empty — no final snapshot was ever created, where real AWS leaves one 'available' manual snapshot named from the stack.
