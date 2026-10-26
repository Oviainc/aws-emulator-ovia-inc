Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.e2e-instance-lifecycle rds.e2e-shard-group-provision-and-teardown-flow
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.e2e-instance-lifecycle|run#31|Timeout|time.sleep(30)
Issue-Key: rds.e2e-shard-group-provision-and-teardown-flow|_wait_shard_group_status#24|Timeout|time.sleep(poll_s)
Issue-Commit: df6081fa6700f84ddafc058f223e27db3aea4084

# RDS end-to-end lifecycle flows never complete: instance create/stop/start/delete and the Limitless cluster + shard-group flow both run out the replay budget without finishing

> _Note: this issue was identified against the code base at commit `df6081fa6700f84ddafc058f223e27db3aea4084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
One cause (Cause 1 of 1), grounded in two failing checks.

Cause 1 of 1: both RDS end-to-end lifecycle checks in the suite never reach their final assertions — each is cut off by the replay's per-check budget (60s) while polling a DescribeDB* report for a settled status. Failing checks: rds.e2e-instance-lifecycle, rds.e2e-shard-group-provision-and-teardown-flow. Neither failure names a refused call, so the divergence is that the flows never converge, not a single wrong response.

AWS: recorded behavior only for the pacing; the status vocabulary is modeled. CreateDBInstance answers a DBInstance in 'creating' that reaches 'available' on its own; StopDBInstance (Smithy: errors DBInstanceNotFoundFault 404, InvalidDBInstanceStateFault 400) moves it 'stopping' -> 'stopped'; DeleteDBInstance leaves it 'deleting' until DescribeDBInstances answers 404 DBInstanceNotFound with message "DBInstance <id> not found.". CreateDBCluster answers 'creating' -> 'available'; CreateDBShardGroup (Smithy: output DBShardGroup with Status; errors DBClusterNotFoundFault 404, DBShardGroupAlreadyExistsFault 400, InvalidDBClusterStateFault 400, UnsupportedDBEngineVersionFault 400) answers a shard group that reaches 'available', and DeleteDBShardGroup takes it back out so DescribeDBShardGroups answers DBShardGroupNotFound / "DBShardGroup <id> not found.". Both checks follow exactly that shape, polling each stage with 15s-30s sleeps between reads.

Emulator: both checks end as `Error: Check timed out after 60.0s` with no failing call recorded. Every operation they touch is implemented in this checkout (create_db_instance.rs, stop_db_instance.rs, create_db_cluster.rs, create_db_shard_group.rs / describe_db_shard_groups.rs are all marked IMPLEMENTED), so no refusal or 501 was surfaced to the client; the flows simply never got to a settled state inside the budget. Hypothesis (neither the model nor the replay states this, so it is offered as a lead, not as the requirement): crate::state sets every accepted lifecycle move as a pending transition due STATUS_TRANSITION_DELAY later (services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:138, 300ms) and materialises it only when the NEXT request calls settle_due_transitions, so the first read after each accepted call still reports the transient status ('creating' / 'stopping' / 'deleting'). Each such read costs the check a full poll sleep (15s for the instance flow, 30s for the cluster/shard-group flow), and two of them exceed the budget: instance flow = wait-for-available + wait-for-stopped; shard-group flow = wait-for-cluster-available + wait-for-shard-group-available.

Why: a client that drives an RDS resource through its documented lifecycle and waits on the service's own status reports (which is what every SDK waiter does) never observes the settled state in a reasonable number of polls, so provisioning scripts, waiters and CI flows hang rather than progress — the control-plane answers are individually plausible but the sequence never converges. For the shard-group flow the whole consequence under test (DescribeDBShardGroups going empty -> one available shard group -> empty again) is never observed at all.

Fix: the DB-instance / DB-cluster / DB-shard-group lifecycle in crate::state (the pending_status + settle_due_transitions machinery and the handlers that set it: create_db_instance.rs, modify_db_instance.rs, stop_db_instance.rs, start_db_instance.rs, delete_db_instance.rs, create_db_cluster.rs, create_db_shard_group.rs, delete_db_shard_group.rs) must let a poller converge: the settled status has to be observable on the reads that follow an accepted call rather than requiring a wall-clock wait per stage, and the refusals the flows retry on (InvalidDBInstanceState for Stop while a storage change is in flight, InvalidDBClusterStateFault for DeleteDBShardGroup) must clear in the same way.

Traces: rds.e2e-instance-lifecycle (create -> describe -> wait available -> ModifyDBInstance AllocatedStorage=25 ApplyImmediately -> Stop -> wait stopped -> Start -> Delete -> wait gone), rds.e2e-shard-group-provision-and-teardown-flow (aurora-postgresql *-limitless cluster -> wait available -> CreateDBShardGroup MaxACU=16 -> wait available -> DeleteDBShardGroup -> wait gone).

## Evidence
Expected (AWS): each lifecycle stage settles and the poll loop advances — DescribeDBInstances reports 'available' then 'stopped' then 404 DBInstanceNotFound ("DBInstance <id> not found."); DescribeDBClusters reports 'available'; DescribeDBShardGroups reports an 'available' shard group whose DBClusterIdentifier is the created cluster, then DBShardGroupNotFound ("DBShardGroup <id> not found.") after the delete.

Actual (emulator), both checks, verbatim replay detail: `Error: Check timed out after 60.0s` — no call-level error, no status mismatch recorded, i.e. the flows never reached their assertions. Observed structure of the two checks: rds.e2e-instance-lifecycle polls DescribeDBInstances every 15s for 'available' and for 'stopped'; rds.e2e-shard-group-provision-and-teardown-flow polls DescribeDBClusters and DescribeDBShardGroups every 30s for 'available'.

Supporting source (lead, not a requirement): services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:138 `pub const STATUS_TRANSITION_DELAY: Duration = Duration::from_millis(300);`, used by PendingStatus::in_a_moment (state.rs:1664) and materialised only by settle_due_transitions at the start of a later handler (e.g. describe_db_shard_groups.rs:168, create_db_cluster.rs:814); create_db_instance.rs:564-565 stores status 'creating' with that pending move.

## Steps to Reproduce
1. Boot the emulator and point an RDS client at it.
2. Instance flow: CreateDBInstance(DBInstanceIdentifier=<id>, Engine='mysql', EngineVersion='8.0', DBInstanceClass='db.t3.micro', AllocatedStorage=20, MasterUsername='sim2real', BackupRetentionPeriod=0, DBSubnetGroupName=<two-AZ group>), then poll DescribeDBInstances until 'available', ModifyDBInstance(AllocatedStorage=25, ApplyImmediately=True), StopDBInstance (retrying InvalidDBInstanceState), poll until 'stopped', StartDBInstance, poll until 'available', DeleteDBInstance(SkipFinalSnapshot=True) and poll until DBInstanceNotFound — observe that the sequence does not converge within the replay budget.
3. Shard-group flow: CreateDBCluster(Engine='aurora-postgresql', EngineVersion=<a *-limitless version from DescribeDBEngineVersions>, ClusterScalabilityType='limitless', StorageType='aurora-iopt1', ...), poll DescribeDBClusters for 'available', CreateDBShardGroup(DBShardGroupIdentifier=<id>, DBClusterIdentifier=<cluster>, MaxACU=16, ComputeRedundancy=0), poll DescribeDBShardGroups for 'available', then DeleteDBShardGroup and poll until DBShardGroupNotFound — observe the same non-convergence.
4. Replay the behavioral checks: rds.e2e-instance-lifecycle and rds.e2e-shard-group-provision-and-teardown-flow.
