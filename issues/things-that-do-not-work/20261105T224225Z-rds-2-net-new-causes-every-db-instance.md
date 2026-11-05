Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.e2e-snapshot-lifecycle rds.snaprestore-final-snapshot-on-delete-restores-rows rds.snapshot-restore-data-query-round-trip
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.e2e-snapshot-lifecycle|run#32|AssertionError|assert dst_addr != src_addr, f"restored instance reused the source endpoint {dst_addr}"
Issue-Key: rds.snaprestore-final-snapshot-on-delete-restores-rows|_poll#27|Timeout|time.sleep(interval_s)
Issue-Key: rds.snapshot-restore-data-query-round-trip|_poll#29|Timeout|time.sleep(interval_s)
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# rds: 2 net-new causes — every DB instance publishes the same Endpoint.Address (a snapshot-restored instance reuses the source's endpoint), and the snapshot-restore-then-query flows never complete

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreDBInstanceFromDBSnapshot

## Description
This issue bundles 2 net-new root causes in the rds service, grounded in failing behavioural checks the suite marked neither ALREADY FILED nor OWNED. They are distinct from the open issues 20261105T090000Z-rds-a-db-instance-publishes-the-listeners-port-not-the-requested-port.md (Endpoint.Port, not Address) and 20261105T131136Z-rds-10-net-new-causes-the-engine-data.md (whose restore cause is the point-in-time path and LatestRestorableTime, not snapshot restore). A fixer must close BOTH causes below before submitting.

=== Cause 1 of 2: distinct DB instances publish an identical Endpoint.Address, so a snapshot-restored instance is indistinguishable from its source ===
AWS: Smithy: DBInstance$Endpoint is an Endpoint structure whose Address member is documented "Specifies the DNS address of the DB instance" — RDS mints one DNS name per instance, derived from that instance's own DBInstanceIdentifier (<identifier>.<account-token>.<region>.rds.amazonaws.com), so two instances that exist at the same time NEVER report the same Address; a restore from a snapshot into a new identifier is reachable only at its own new address, and the source instance keeps serving its own.
Emulator: DescribeDBInstances reports the SAME Endpoint.Address for the restored instance and for its source instance (the check compares the two addresses and finds them equal), so the published endpoint does not identify an instance at all. The requirement here is that the address be per-instance and derived from the instance identifier — not any particular host value, which is environment-specific.
Why: a client that connects to the restored instance's published endpoint reaches whatever single listener the emulator exposes, i.e. potentially the SOURCE database; it cannot route to the restored data, cannot verify a restore, and any configuration that keys a connection string, a DNS CNAME or a security rule on the per-instance endpoint collapses onto one shared address.
Fix: the DB-instance endpoint publisher in the rds crate's state/data-plane layer — derive and store a per-instance endpoint address from the instance's own identifier and serve each instance's sessions behind it, instead of publishing one shared listener address for every instance.
Traces: rds.e2e-snapshot-lifecycle.

=== Cause 2 of 2: the snapshot-restore-then-query flows never complete — the checks are cut off at the harness cap with nothing to assert ===
AWS: recorded behavior only for the timing; the contract is Smithy's: CreateDBSnapshot drives the snapshot to Status='available', RestoreDBInstanceFromDBSnapshot (and the DeleteDBInstance FinalDBSnapshotIdentifier path that produces a final snapshot) creates a new DB instance that progresses creating -> available and publishes an Endpoint that serves the restored rows, and DeleteDBInstance(FinalDBSnapshotIdentifier=<name>) leaves that snapshot describable afterwards. Every state a client polls for is reached in bounded time.
Emulator: both snapshot-restore round-trip checks end with "Check timed out after 60.0s" — the flow never reaches the state the poll waits on, so not a single assertion in either check is evaluated. Hypothesis: the restored instance (or the final snapshot the delete must produce) never settles into the state DescribeDBInstances/DescribeDBSnapshots must report, so the poll loop spends the whole budget. The operation's scaffold stub was previously closed by the fixed issue 20261105T131136Z-rds-restoredbinstancefromdbsnapshot-stub.md; what is filed here is that the implemented path still never converges.
Why: a client that follows the documented backup/restore runbook (snapshot, restore, query the restored rows; or delete-with-final-snapshot, then restore from it) hangs forever instead of either succeeding or being refused — the whole restore feature is unusable and its failure is invisible, because nothing ever errors.
Fix: the DB-snapshot/restore lifecycle in the rds crate's state — the snapshot and restored-instance status transitions (including the final snapshot DeleteDBInstance must create) and the DescribeDBInstances/DescribeDBSnapshots reads that must report them.
Traces: rds.snaprestore-final-snapshot-on-delete-restores-rows, rds.snapshot-restore-data-query-round-trip.

## Evidence
Cause 1: AssertionError: restored instance reused the source endpoint 127.0.0.1 — DescribeDBInstances returned the same Endpoint.Address for the snapshot-restored instance as for its source; AWS reports a distinct per-instance DNS address for each (DBInstance$Endpoint$Address: "Specifies the DNS address of the DB instance").
Cause 2: both checks ended with "Check timed out after 60.0s"; no restore ever reached a state the poll accepts, so no response body was compared at all.

## Steps to Reproduce
Cause 1: CreateDBInstance A; wait available; CreateDBSnapshot of A; wait available; RestoreDBInstanceFromDBSnapshot into identifier B; wait available; compare DescribeDBInstances B .Endpoint.Address with A's — they are equal.
Cause 2: (a) CreateDBInstance A, write rows over the engine endpoint, DeleteDBInstance(A, FinalDBSnapshotIdentifier=S, SkipFinalSnapshot=false), poll DescribeDBSnapshots for S, then RestoreDBInstanceFromDBSnapshot(S -> B) and poll B to available and query the rows. (b) CreateDBInstance A, CreateDBSnapshot S, RestoreDBInstanceFromDBSnapshot(S -> B), poll B to available and query the restored rows. Both polls never see the awaited state.
