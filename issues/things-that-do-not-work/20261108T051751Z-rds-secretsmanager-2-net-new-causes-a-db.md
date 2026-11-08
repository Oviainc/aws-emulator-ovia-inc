Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.refarch-aur-omitted-cluster-settings-return-service-defaults rds__secretsmanager.refarch-aur-reader-endpoint-exists-with-zero-reader-instances
Issue-Key: rds__secretsmanager.refarch-aur-omitted-cluster-settings-return-service-defaults|run#11|AssertionError|assert cluster["Port"] == 5432, (
Issue-Key: rds__secretsmanager.refarch-aur-reader-endpoint-exists-with-zero-reader-instances|run#15|AssertionError|assert writer_host.startswith(f"{name}.cluster-"), (
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# rds__secretsmanager: 2 net-new causes — a DB cluster's omitted Port reads back as an ephemeral listener port, and the cluster writer/reader endpoints are a raw host address instead of cluster-derived DNS names

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
This issue bundles 2 net-new root causes in the rds__secretsmanager Aurora reference-architecture suite, grounded in failing behavioural checks the suite report marked neither ALREADY FILED nor OWNED. Cause 1 is the cluster-level counterpart of the instance-level port defect in the open issue 20261106T031005Z-rds-7-net-new-causes-endpoint-port-publishes.md (whose Issue-Trace-Ids cover DBInstance Endpoint.Port, not DBCluster$Port, and do not include this check). A fixer must close BOTH causes below before submitting.

=== Cause 1 of 2: DBCluster$Port reports an ephemeral internal listener port when Port was omitted at create time ===
AWS: Smithy: DBCluster$Port — "The port that the database engine is listening on"; CreateDBClusterMessage$Port is optional and defaults to the engine's port (Aurora PostgreSQL 5432, Aurora MySQL 3306). So a cluster created with Port omitted must read back Port = the engine default, exactly as it must read back the other service defaults the same check asserts (BackupRetentionPeriod 1, DeletionProtection false, a minted preferred backup/maintenance window).
Emulator: DescribeDBClusters reports Port = a value from the host's dynamic port range (observed 35693) for an aurora-postgresql cluster created with Port omitted — a different value per cluster. The requirement is the engine default (or the requested Port); which socket the emulator's data plane happens to bind is an implementation detail that must not leak into the response.
Why: a client reading DBCluster.Port to build a connection string, a security-group rule, a DB-proxy target or a CloudFormation Endpoint.Port output gets a number AWS would never return, and "Aurora PostgreSQL listens on 5432" is false here.
Fix: the cluster-port default and its projection in the rds crate's CreateDBCluster/DescribeDBClusters path — record the engine-family default when Port is omitted (else the requested value) and serve that, decoupled from the local listener.
Traces: rds__secretsmanager.refarch-aur-omitted-cluster-settings-return-service-defaults.

=== Cause 2 of 2: a DB cluster's Endpoint and ReaderEndpoint are a raw host address, not cluster-derived DNS names ===
AWS: Smithy: DBCluster$Endpoint is the cluster's writer endpoint and DBCluster$ReaderEndpoint is "the reader endpoint for the DB cluster", which "load-balances connections across the Aurora Replicas" — both are minted by the CLUSTER, so they exist even with zero reader members. RDS derives them from the cluster's own DBClusterIdentifier: the writer is <DBClusterIdentifier>.cluster-<account-token>.<region>.rds.amazonaws.com and the reader is the same name with the cluster-ro- infix (<DBClusterIdentifier>.cluster-ro-<account-token>.<region>.rds.amazonaws.com). The two names differ, share a DNS suffix, hosted zone and port, and the writer's host always begins "<DBClusterIdentifier>.cluster-".
Emulator: DescribeDBClusters reports the writer endpoint as a bare host address ('127.0.0.1') — it is not derived from the cluster identifier, carries no 'cluster-' infix, and cannot be distinguished from the reader endpoint or from any other cluster's. The requirement here is the NAME's derivation and shape, not any particular host value, which is environment-specific.
Why: the writer/reader split is the entire point of an Aurora cluster endpoint: a client that must send writes to the writer name and reads to the cluster-ro- name has no way to address them, cannot CNAME either, and cannot tell one cluster's endpoint from another's. Any configuration, DNS record or connection string keyed on the documented cluster endpoint name is unbuildable.
Fix: the cluster-endpoint minting in the rds crate's state and the DescribeDBClusters/CreateDBCluster serializers — derive per-cluster writer and reader DNS names from the cluster identifier (with the cluster- and cluster-ro- infixes and a shared suffix/hosted zone/port) and serve sessions behind them, instead of publishing the data plane's raw listener address.
Traces: rds__secretsmanager.refarch-aur-reader-endpoint-exists-with-zero-reader-instances.

## Evidence
Cause 1: AssertionError: an omitted Port must default to the engine's 5432, got 35693 — DescribeDBClusters on an aurora-postgresql cluster created without Port.
Cause 2: AssertionError: the writer endpoint is derived from the cluster identifier with a 'cluster-' infix; got '127.0.0.1' — DBCluster.Endpoint for a cluster named <id> must begin '<id>.cluster-', and DBCluster.ReaderEndpoint must be the matching '<id>.cluster-ro-' name sharing its suffix, hosted zone and port.

## Steps to Reproduce
Cause 1: CreateDBCluster(Engine=aurora-postgresql, MasterUsername/ManageMasterUserPassword set, a DB subnet group) with Port, BackupRetentionPeriod, DeletionProtection and the maintenance/backup windows all omitted; wait available; DescribeDBClusters and read Port — it must be 5432.
Cause 2: on that same cluster with zero reader members, DescribeDBClusters and read Endpoint and ReaderEndpoint — the writer host must start '<DBClusterIdentifier>.cluster-', the reader host must be the matching '<DBClusterIdentifier>.cluster-ro-' name, the two must differ, and they must share their DNS suffix, HostedZoneId and Port.
