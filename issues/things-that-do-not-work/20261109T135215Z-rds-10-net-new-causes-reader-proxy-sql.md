Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.aurora-mysql-reader-endpoint-write-refused-read-only rds.aurora-writer-reader-endpoint-query-routing rds.db-proxy-mysql-data-plane-round-trip-pinning-and-iam-auth rds.sqlserver-express-admin-login-identity-and-privilege-boundary rds.sqlserver-express-database-table-round-trip-visible-to-second-session rds.sqlserver-express-managed-secret-login-and-dynamic-parameter-applies rds.sqlserver-express-transaction-rollback-and-unique-constraint-violation rds.deploy-db-instance-cfn-behaves rds.mysql-custom-port-and-session-identity-match-creation-parameters rds.snapshot-restore-different-class-and-port rds.db-cluster-modify-while-creating-rejected rds.db-proxy-modify-while-creating-rejected rds.stop-db-instance-double-stop-and-modify-while-stopped-rejected rds.delete-cluster-conflicting-snapshot-name-rejected rds.delete-cluster-final-snapshot-name-conflict-error rds.automated-backup-arn-param-malformed-rejected rds.cluster-read-replica-promotion rds.modify-apply-immediately-true-clears-a-prior-deferred-class-change rds.cross-region-snapshot-copy-kms-restore-round-trip rds.restore-instance-mismatched-engine-version-rejected rds.delete-automated-backup-by-arn rds.time-zone-parameter-changes-now-offset-in-new-session
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.aurora-mysql-reader-endpoint-write-refused-read-only|run#22|PublishedUrlRejected|reader_host, reader_port = ctx.endpoint_host(reader_ep, int(port))
Issue-Key: rds.aurora-writer-reader-endpoint-query-routing|run#31|PublishedUrlRejected|reader_ep_dial, reader_port_dial = ctx.endpoint_host(reader_ep, int(port))
Issue-Key: rds.db-proxy-mysql-data-plane-round-trip-pinning-and-iam-auth|run#21|PublishedUrlRejected|proxy_host, proxy_port = ctx.endpoint_host(proxy["Endpoint"], port)
Issue-Key: rds.sqlserver-express-admin-login-identity-and-privilege-boundary|run#8|PublishedUrlRejected|host, port = ctx.endpoint_host(inst["Endpoint"]["Address"], int(inst["Endpoint"]["Port"]))
Issue-Key: rds.sqlserver-express-database-table-round-trip-visible-to-second-session|run#6|PublishedUrlRejected|host, port = ctx.endpoint_host(inst["Endpoint"]["Address"], int(inst["Endpoint"]["Port"]))
Issue-Key: rds.sqlserver-express-managed-secret-login-and-dynamic-parameter-applies|run#6|PublishedUrlRejected|host, port = ctx.endpoint_host(inst["Endpoint"]["Address"], int(inst["Endpoint"]["Port"]))
Issue-Key: rds.sqlserver-express-transaction-rollback-and-unique-constraint-violation|run#6|PublishedUrlRejected|host, port = ctx.endpoint_host(inst["Endpoint"]["Address"], int(inst["Endpoint"]["Port"]))
Issue-Key: rds.deploy-db-instance-cfn-behaves|run#12|AssertionError|assert endpoint["Address"].lower().startswith(instance_id.lower()), (
Issue-Key: rds.mysql-custom-port-and-session-identity-match-creation-parameters|run#7|AssertionError|assert inst["Endpoint"]["Port"] == _PORT, (
Issue-Key: rds.snapshot-restore-different-class-and-port|run#29|AssertionError|assert int(dst["Endpoint"]["Port"]) == _DST_PORT, (
Issue-Key: rds.db-cluster-modify-while-creating-rejected|run#5|AssertionError|assert after != "creating", (
Issue-Key: rds.db-proxy-modify-while-creating-rejected|run#7|AssertionError|assert after != "creating", (
Issue-Key: rds.stop-db-instance-double-stop-and-modify-while-stopped-rejected|run#17|AssertionError|assert modify_err == expected_modify, (
Issue-Key: rds.delete-cluster-conflicting-snapshot-name-rejected|run#17|AssertionError|assert message == (
Issue-Key: rds.delete-cluster-final-snapshot-name-conflict-error|run#15|AssertionError|assert message == (
Issue-Key: rds.automated-backup-arn-param-malformed-rejected|run#5|AssertionError|assert delete_code == "InvalidParameterValue", f"unexpected error code: {delete_code}"
Issue-Key: rds.cluster-read-replica-promotion|run#10|AssertionError|assert code == "InvalidDBClusterStateFault", f"expected InvalidDBClusterStateFault, got {code!r}"
Issue-Key: rds.modify-apply-immediately-true-clears-a-prior-deferred-class-change|run#14|AssertionError|assert pending.get("DBInstanceClass") == _TO_CLASS, (
Issue-Key: rds|CopyDBSnapshot|DBSnapshotNotFound
Issue-Key: rds.restore-instance-mismatched-engine-version-rejected|_error#15|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: rds.delete-automated-backup-by-arn|_poll_until#11|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: rds.time-zone-parameter-changes-now-offset-in-new-session|run#4|AssertionError|assert params.get("time_zone", {}).get("ApplyType") == "dynamic", (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# rds: 10 net-new causes — reader/proxy/SQL Server endpoints are unreachable AWS hostnames, requested ports are ignored, and several refusals, restores and backups diverge

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
10 net-new root causes in the RDS control plane and its published connection details. Related open issues cover neighbouring causes but carry none of these replay keys: 20261108T051751Z-rds-7-net-new-causes-endpoint-port-is.md, 20261104T053605Z-rds-10-net-new-causes-refusal-messages-are.md, 20261104T053605Z-rds-6-net-new-causes-applyimmediately-false-is.md, 20261109T083259Z-rds-serves-emulator-authored-values-on-success-paths.md.

Cause 1 of 10: cluster reader endpoints, SQL Server instance endpoints and DB proxy endpoints are published as real-AWS hostnames the emulator does not serve.
AWS: the Endpoint/ReaderEndpoint a client reads is the address it connects to; against AWS it resolves and completes the engine handshake. An emulator must announce an address it answers on (its own endpoint host, a documented own host, or a loopback address) because callers dial exactly the string it returns. The provisioned postgres/mysql instance endpoints already do; these three classes do not.
Emulator: DescribeDBClusters publishes '<cluster>.cluster-ro-<hex>.<region>.rds.amazonaws.com', DescribeDBInstances publishes '<instance>.<hex>.<region>.rds.amazonaws.com:1433' for SQL Server, and the proxy publishes '<proxy>.proxy-<hex>.<region>.rds.amazonaws.com' - none of which is an address the emulator serves, so every connection attempt is refused before any traffic (PublishedUrlRejected).
Why: reader-endpoint routing, read-only enforcement on readers, the entire SQL Server surface and the whole proxy data plane are unreachable; a client is handed a connection string that cannot work.
Fix: the endpoint publisher for cluster reader endpoints, SQL Server instance endpoints and DB proxy endpoints in the RDS service's cluster/instance/proxy projections.
Traces: rds.aurora-mysql-reader-endpoint-write-refused-read-only, rds.aurora-writer-reader-endpoint-query-routing, rds.db-proxy-mysql-data-plane-round-trip-pinning-and-iam-auth, rds.sqlserver-express-admin-login-identity-and-privilege-boundary, rds.sqlserver-express-database-table-round-trip-visible-to-second-session, rds.sqlserver-express-managed-secret-login-and-dynamic-parameter-applies, rds.sqlserver-express-transaction-rollback-and-unique-constraint-violation.

Cause 2 of 10: an instance's published Endpoint.Address is a bare IP, not a name derived from the instance identifier.
AWS: Endpoint.Address begins with the DB instance identifier ('<db-instance-identifier>.<hash>.<region>.rds.amazonaws.com'), so a client can tie an address back to its instance.
Emulator: Address is '127.0.0.1'.
Why: a client that validates or routes on the identifier prefix (and any tooling that logs which instance a connection points at) cannot. Note this interacts with cause 1: the address must be both derived from the identifier AND served by the emulator.
Fix: the instance endpoint address builder in the RDS instance projection.
Traces: rds.deploy-db-instance-cfn-behaves.

Cause 3 of 10: the requested Port is discarded on create and on restore.
AWS: CreateDBInstance(Port=3315) and RestoreDBInstanceFromDBSnapshot(Port=5433) publish exactly that port in Endpoint.Port and listen on it; Smithy models Port on both inputs.
Emulator: Endpoint.Port is a randomly assigned high port (40157 where 3315 was asked for, 39789 where 5433 was) - the request's value is ignored.
Why: a client that pins a non-default port in its connection config, security groups or IaC connects to the wrong port, and the value it read back is not the one it set.
Fix: the Port parameter handling in the CreateDBInstance / RestoreDBInstanceFromDBSnapshot handlers and the listener allocation behind the published endpoint.
Traces: rds.mysql-custom-port-and-session-identity-match-creation-parameters, rds.snapshot-restore-different-class-and-port.

Cause 4 of 10: five refusal messages are emulator-authored prose instead of AWS's fixed text.
AWS, per class (fixed text; the identifier and status are values from the request and from the emulator's state):
 - ModifyDBCluster on a cluster that is not available: InvalidDBClusterStateFault, "DB cluster isn't available for modification with status <status>."
 - ModifyDBProxy on a proxy that is not ACTIVE: InvalidDBProxyStateFault, 'DB Proxy <name> is in an unsupported state - <STATE>, needs to be in [ACTIVE]'.
 - ModifyDBInstance on a stopped instance: InvalidDBInstanceState, "You can't modify a stopped DB instance. Start the DB instance, and then modify it." (Smithy: ModifyDBInstance models InvalidDBInstanceStateFault, awsQueryError code InvalidDBInstanceState, 400.)
 - DeleteDBCluster with a FinalDBSnapshotIdentifier that already exists, and the equivalent conflicting-name delete: the check asserts AWS's exact text for the cluster-snapshot-already-exists class; the emulator's differs.
Emulator: 'This cluster could not be modified. Please try again'; 'DB proxy <name> is in state creating and cannot be modified'; 'Database instance is not in available state.'; 'DB cluster snapshot already exists: <name>' (twice).
Why: a client that matches on the message to distinguish 'wrong state' from 'wrong request', or to extract the blocking status/name, matches nothing; operators reading the message get no instruction (AWS's stopped-instance text tells them to start it first).
Fix: the refusal message templates in the ModifyDBCluster, ModifyDBProxy, ModifyDBInstance and DeleteDBCluster handlers.
Traces: rds.db-cluster-modify-while-creating-rejected, rds.db-proxy-modify-while-creating-rejected, rds.stop-db-instance-double-stop-and-modify-while-stopped-rejected, rds.delete-cluster-conflicting-snapshot-name-rejected, rds.delete-cluster-final-snapshot-name-conflict-error.

Cause 5 of 10: operations answer NotImplemented where AWS answers a modeled refusal.
AWS: DeleteDBInstanceAutomatedBackup validates the shape of DBInstanceAutomatedBackupsArn before any lookup and refuses a malformed ARN with InvalidParameterValue and the message 'The provided ARN (<arn>) is invalid for this parameter (DbInstanceAutomatedBackupsArn).'; with a well-formed DbiResourceId it then answers DBInstanceAutomatedBackupNotFound 'Automated backups could not be found for: <resource-id>' (Smithy: DeleteDBInstanceAutomatedBackup errors DBInstanceAutomatedBackupNotFoundFault, InvalidDBInstanceAutomatedBackupStateFault). PromoteReadReplicaDBCluster on a cluster that is not a read replica refuses InvalidDBClusterStateFault 'You cannot promote a DB cluster that is not a read replica.' (Smithy: errors DBClusterNotFoundFault, InvalidDBClusterStateFault).
Emulator: both answer the unmodeled code NotImplemented. The PromoteReadReplicaDBCluster scaffold stub already has an auto-filed issue (it was reported against check rds.read-replica-cluster-promote); this cause is grounded in the two checks below that are not covered by it, and DeleteDBInstanceAutomatedBackup's ARN validation is net-new.
Why: an SDK cannot classify NotImplemented as a client fault, so retry logic and error branches behave arbitrarily, and a client never learns that its ARN was malformed or its cluster was not a replica.
Fix: the DeleteDBInstanceAutomatedBackup handler (ARN shape validation and its two refusals) and the PromoteReadReplicaDBCluster handler.
Traces: rds.automated-backup-arn-param-malformed-rejected, rds.cluster-read-replica-promotion.

Cause 6 of 10: ModifyDBInstance(ApplyImmediately=False) records nothing in PendingModifiedValues.
AWS: a deferred modification is reflected immediately in the instance's PendingModifiedValues (here PendingModifiedValues.DBInstanceClass is the requested class) and stays there until the maintenance window applies it or a later ApplyImmediately=True modification supersedes it.
Emulator: PendingModifiedValues is {} right after the deferred modify, so the pending change is invisible.
Why: a client cannot see that a change is queued, cannot tell a deferred modify from a no-op, and cannot verify that an immediate modify cleared a prior deferred one.
Fix: the ModifyDBInstance handler's deferred-change bookkeeping and the DBInstance projection's PendingModifiedValues.
Traces: rds.modify-apply-immediately-true-clears-a-prior-deferred-class-change.

Cause 7 of 10: CopyDBSnapshot refuses a snapshot ARN that exists in the calling region.
AWS: CopyDBSnapshot with SourceDBSnapshotIdentifier set to a full snapshot ARN in the current region copies that snapshot; DBSnapshotNotFound is for a snapshot that does not exist.
Emulator: the call is refused 404 DBSnapshotNotFound with 'Requested snapshot <arn> not found in source region us-west-2' - the ARN's own region is the calling region, so the lookup rejects a snapshot it holds.
Why: ARN-addressed snapshot copies (the only form that works cross-region, and a legal form in-region) are impossible, so no snapshot copy/KMS re-encrypt/restore flow can run.
Fix: the CopyDBSnapshot handler's source resolution (it must accept a snapshot ARN, parse its region and match it against local state).
Traces: rds.cross-region-snapshot-copy-kms-restore-round-trip.

Cause 8 of 10: RestoreDBInstanceFromDBSnapshot accepts an engine version that does not match the snapshot.
AWS: restoring a snapshot into an instance whose requested engine/EngineVersion is incompatible with the snapshot is refused (InvalidRestoreFault / InvalidParameterCombination per Smithy's RestoreDBInstanceFromDBSnapshot error set) before any instance is created.
Emulator: the call succeeds 200 and returns a creating DBInstance.
Why: a client's restore of the wrong version silently produces an instance whose engine does not match its data, instead of a refusal at submit time.
Fix: the RestoreDBInstanceFromDBSnapshot handler's engine/version compatibility validation.
Traces: rds.restore-instance-mismatched-engine-version-rejected.

Cause 9 of 10: no automated backup is ever retained for a deleted instance.
AWS: deleting an instance that had a backup retention period leaves a retained automated backup, discoverable by its DbiResourceId/ARN, until it is explicitly deleted.
Emulator: polling for the retained automated backup of the deleted instance returns None for 120 s - nothing is ever created. (The DescribeDBInstanceAutomatedBackups scaffold stub has its own auto-filed issue; this check gets no exception, it gets an empty result, so the missing retention state is the defect.)
Why: a client that relies on retained automated backups for point-in-time recovery after a delete finds nothing to restore from and no error explaining why.
Fix: the automated-backup state the DeleteDBInstance path must create and the DescribeDBInstanceAutomatedBackups/DeleteDBInstanceAutomatedBackup reads over it.
Traces: rds.delete-automated-backup-by-arn.

Cause 10 of 10: the mysql8.4 engine-default parameter set omits time_zone.
AWS: DescribeEngineDefaultParameters/DescribeDBParameters for family mysql8.4 include a time_zone parameter whose ApplyType is 'dynamic', which is what makes a time-zone change take effect in new sessions without a reboot.
Emulator: time_zone is absent from the family's parameters (ApplyType read back as None).
Why: a client cannot set or even discover the instance's time zone through a parameter group, and cannot tell a dynamic parameter from a static one for it.
Fix: the engine-default parameter catalog for the mysql families in the RDS service.
Traces: rds.time-zone-parameter-changes-now-offset-in-new-session.

## Evidence
Cause 1: PublishedUrlRejected for sim2real-behav-clus-<hex>.cluster-ro-<hex>.us-west-2.rds.amazonaws.com:<port>, sim2real-behav-mssql-<hex>.<hex>.us-west-2.rds.amazonaws.com:1433 and sim2real-behav-rdspx-<hex>.proxy-<hex>.us-west-2.rds.amazonaws.com:<port> - 'its host is not one of the emulator's own hosts'.
Cause 2: expected endpoint address to start with '<db-instance-identifier>', got '127.0.0.1'.
Cause 3: Endpoint.Port must be the requested 3315; got 40157. restore must publish the requested port 5433, got 39789.
Cause 4: got ('InvalidDBClusterStateFault','This cluster could not be modified. Please try again') vs AWS "DB cluster isn't available for modification with status creating."; got ('InvalidDBProxyStateFault','DB proxy <name> is in state creating and cannot be modified') vs AWS 'DB Proxy <name> is in an unsupported state - CREATING, needs to be in [ACTIVE]'; got ('InvalidDBInstanceState','Database instance is not in available state.') vs AWS "You can't modify a stopped DB instance. Start the DB instance, and then modify it."; got 'DB cluster snapshot already exists: <name>' where the check requires AWS's wording (twice).
Cause 5: AssertionError: unexpected error code: NotImplemented (expected InvalidParameterValue with 'The provided ARN (<arn>) is invalid for this parameter (DbInstanceAutomatedBackupsArn).'); expected InvalidDBClusterStateFault, got 'NotImplemented'.
Cause 6: ModifyDBInstance(ApplyImmediately=False) must record the new class in PendingModifiedValues.DBInstanceClass; got {}.
Cause 7: DBSnapshotNotFoundFault: Requested snapshot arn:aws:rds:us-west-2:<acct>:snapshot:<name> not found in source region us-west-2.
Cause 8: real AWS rejects this call, but it SUCCEEDED and returned a creating DBInstance.
Cause 9: retained automated backup for db-<resource-id> not met within 120s. Last result: None.
Cause 10: time_zone must be ApplyType 'dynamic' in family mysql8.4; got None.

## Steps to Reproduce
Cause 1: create an Aurora cluster, a SQL Server Express instance and a DB proxy; read ReaderEndpoint / Endpoint.Address and connect to each.
Cause 2: create a DB instance and read Endpoint.Address.
Cause 3: CreateDBInstance with Port=3315; RestoreDBInstanceFromDBSnapshot with Port=5433; read Endpoint.Port.
Cause 4: ModifyDBCluster on a creating cluster; ModifyDBProxy on a creating proxy; ModifyDBInstance on a stopped instance; DeleteDBCluster with an already-used FinalDBSnapshotIdentifier.
Cause 5: DeleteDBInstanceAutomatedBackup with a malformed DBInstanceAutomatedBackupsArn; PromoteReadReplicaDBCluster on a non-replica cluster.
Cause 6: ModifyDBInstance(DBInstanceClass=<new>, ApplyImmediately=False) then DescribeDBInstances and read PendingModifiedValues.
Cause 7: CreateDBSnapshot then CopyDBSnapshot with SourceDBSnapshotIdentifier set to the snapshot's full ARN.
Cause 8: RestoreDBInstanceFromDBSnapshot with an EngineVersion incompatible with the snapshot.
Cause 9: create an instance with BackupRetentionPeriod>0, delete it, then poll DescribeDBInstanceAutomatedBackups for the retained backup.
Cause 10: DescribeEngineDefaultParameters(DBParameterGroupFamily='mysql8.4') and look for time_zone.
