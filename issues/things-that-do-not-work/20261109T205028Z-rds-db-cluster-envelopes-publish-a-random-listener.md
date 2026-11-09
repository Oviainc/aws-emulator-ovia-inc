Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-cluster-lifecycle-with-snapshot
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-cluster-lifecycle-with-snapshot rds-db-cluster-state-roles-and-capacity-operations rds-multi-cluster-isolation rds-aurora-serverless-data-api-managed-secret-sql-round-trip rds-aurora-serverless-v2-config-read-back rds-db-cluster-lifecycle rds-delete-requires-final-snapshot-param rds-duplicate-create-error rds-global-cluster-delete-with-member
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.ReaderEndpoint
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBClusterMembers.DBClusterMember.PromotionTier
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.MasterUserSecret.SecretArn
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.ReaderEndpoint
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.ReaderEndpoint
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.ReaderEndpoint
Issue-Related: 20261108T233943Z-rds-db-cluster-envelopes-report-the-emulator-s-random.md
Issue-Related: 20261109T083259Z-rds-serves-emulator-authored-values-on-success-paths.md
Issue-Commit: fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7

# rds: DB cluster envelopes publish a random listener port instead of the engine port (6 causes)

> _Note: this issue was identified against the code base at commit `fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Six divergence clusters share one root cause: every operation that returns a `DBCluster` envelope serializes `DBCluster.Port` as the emulator's own ephemeral listener port rather than the port the database engine is addressed on, and the same envelopes drop or synthesize neighbouring members. All six are on the `rds` service and all six are net-new (the replay reported no owning issue and no blocking stub for any of them).

Cause 1 of 6: CreateDBCluster — DBCluster.Port is the emulator's listener port.
AWS: Smithy `DBCluster.Port` (target IntegerOptional) is documented "The port that the database engine is listening on"; `CreateDBCluster` accepts an optional input `Port` (IntegerOptional). AWS echoes the requested port, and when the request omits `Port` it echoes the engine's default port for the cluster's `Engine` (3306 for the MySQL-family engines, 5432 for the PostgreSQL-family engines). The value is therefore a deterministic function of the request and the engine, not of the host the API runs on.
Emulator: the `CreateDBCluster` result reports a port drawn from the host's ephemeral range (the replay observed a five-digit ephemeral value where the recording carries the engine port 3306), and `ReaderEndpoint` is reported on a host the emulator does not answer on (verdict `host_not_own`).
Why: a client builds its connection string from `DBCluster.Port`, and tooling asserts the port it asked for (or the engine default) came back. An ephemeral per-boot number means the connection string is wrong on every run, and any config read-back or CloudFormation output that carries the port diverges.
Fix: the CreateDBCluster handler and the DBCluster serializer in the rds service crate — resolve `Port` from the request, falling back to the engine-family default, and persist it on cluster state so every later envelope reads it back; do not substitute the process's bound listener port.
Traces: rds-db-cluster-lifecycle-with-snapshot#task#1 (cluster also covers rds-db-cluster-state-roles-and-capacity-operations#setup#1, rds-multi-cluster-isolation#setup#1, rds-multi-cluster-isolation#setup#2).

Cause 2 of 6: ModifyDBCluster — the same wrong port is read back after a modify.
AWS: `ModifyDBCluster` returns the `DBCluster` envelope, so `Port` must still be the engine/requested port for that cluster (3306 in the recording).
Emulator: ModifyDBCluster echoes the same ephemeral listener port the create emitted, and `ReaderEndpoint` is again flagged `host_not_own`.
Why: a caller that modifies a cluster and re-reads its endpoint from the mutation response gets an unusable port.
Fix: the ModifyDBCluster handler must project the persisted cluster port, not re-derive it from the listener.
Traces: rds-db-cluster-lifecycle-with-snapshot#task#3.

Cause 3 of 6: DeleteDBCluster — the delete-time envelope carries the listener port.
AWS: `DeleteDBCluster` returns the cluster as it stood when the delete was accepted, so `Port` is still the engine port (5432 in the recording).
Emulator: DeleteDBCluster reports an ephemeral listener port; `ReaderEndpoint` is flagged `host_not_own`.
Why: teardown assertions and deletion bookkeeping that capture the final cluster record see a port that never belonged to the cluster.
Fix: the DeleteDBCluster handler's DBCluster projection.
Traces: rds-db-cluster-lifecycle#teardown#6 (cluster also covers rds-db-cluster-lifecycle-with-snapshot#teardown#10, rds-db-cluster-state-roles-and-capacity-operations#teardown#13, rds-multi-cluster-isolation#teardown#5, rds-multi-cluster-isolation#teardown#6).

Cause 4 of 6: DescribeDBClusters — the read path reports the listener port for an Aurora Serverless v2 cluster.
AWS: `DescribeDBClusters` projects the stored cluster, so `Port` is the engine port (5432 for the aurora-postgresql cluster in the recording).
Emulator: DescribeDBClusters returns an ephemeral listener port and a `ReaderEndpoint` host the emulator does not serve.
Why: config read-back — the canonical way a client learns where to connect — is wrong, so Serverless v2 connection details cannot be trusted.
Fix: the DescribeDBClusters DBCluster serializer / cluster state.
Traces: rds-aurora-serverless-v2-config-read-back#task#2 (cluster also covers rds-db-cluster-lifecycle#task#4, rds-db-cluster-lifecycle-with-snapshot#task#2, rds-delete-requires-final-snapshot-param#task#3, rds-duplicate-create-error#task#3, rds-multi-cluster-isolation#task#3, rds-multi-cluster-isolation#task#4).

Cause 5 of 6: DescribeDBClusters on a managed-secret cluster also omits DBClusterMembers.PromotionTier and mints a non-AWS MasterUserSecret ARN.
AWS: Smithy `DBClusterMember.PromotionTier` (IntegerOptional) is "the order in which an Aurora Replica is promoted to the primary instance"; AWS populates it for every cluster member (the recording carries 1). `DBCluster.MasterUserSecret` is "The secret managed by RDS in Amazon Web Services Secrets Manager for the master user password" and its `SecretArn` is a Secrets Manager ARN for an RDS-owned secret; `DBCluster.Port` is again the engine port (5432).
Emulator: `PromotionTier` is absent from every `DBClusterMember`, `MasterUserSecret.SecretArn` is an emulator-minted ARN whose secret-name segment is built from the internal cluster resource id rather than AWS's managed-secret naming, `Port` is an ephemeral listener port, and `ReaderEndpoint` is flagged `host_not_own`.
Why: failover-ordering logic reads `PromotionTier` and finds nothing, and a client that passes `MasterUserSecret.SecretArn` to Secrets Manager carries an identifier shaped unlike the one AWS mints.
Fix: the DBCluster/DBClusterMember serializer (emit PromotionTier from member state) and the managed-master-user-secret minting in cluster state. Related open issue 20261107T030257Z-rds-secretsmanager-2-net-new-causes-the-rds.md discusses the managed-secret name; it does not cover PromotionTier or this step's replay key.
Traces: rds-aurora-serverless-data-api-managed-secret-sql-round-trip#task#9.

Cause 6 of 6: CreateDBCluster for a global-cluster member reports the listener port and a placeholder GlobalClusterResourceId.
AWS: `DBCluster.GlobalClusterResourceId` is "the Amazon Web Services Region-unique, immutable identifier of the global database cluster that this DB cluster is a member of" — an opaque `cluster-`-prefixed identifier that is unique per global cluster. `DBCluster.Port` is the engine port (3306 here).
Emulator: `Port` is an ephemeral listener port, `GlobalClusterResourceId` is a fixed zero-padded counter value of the form `cluster-000…001` reused for every global cluster, and `ReaderEndpoint` is flagged `host_not_own`.
Why: a caller that correlates a member cluster to its global cluster by `GlobalClusterResourceId` cannot distinguish two global clusters, and the port is unusable.
Fix: the CreateDBCluster handler's global-cluster linkage plus the resource-id minting in rds state. Open issue 20261108T233943Z-rds-db-cluster-envelopes-report-the-emulator-s-random.md covers a related envelope fault but carries no replay key for this step.
Traces: rds-global-cluster-delete-with-member#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (98 fixtures, 993 steps, score 49.8%).

Cause 1 — rds-db-cluster-lifecycle-with-snapshot#task#1, CreateDBCluster, status 200 -> 200:
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.Port: value_mismatch (expected '3306', got '40185')
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.ReaderEndpoint: host_not_own

Cause 2 — rds-db-cluster-lifecycle-with-snapshot#task#3, ModifyDBCluster, 200 -> 200:
  $.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.Port: value_mismatch (expected '3306', got '40185')
  $.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.ReaderEndpoint: host_not_own

Cause 3 — rds-db-cluster-lifecycle#teardown#6, DeleteDBCluster, 200 -> 200:
  $.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.Port: value_mismatch (expected '5432', got '43513')
  $.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.ReaderEndpoint: host_not_own

Cause 4 — rds-aurora-serverless-v2-config-read-back#task#2, DescribeDBClusters, 200 -> 200:
  $...DBCluster.Port: value_mismatch (expected '5432', got '36417')
  $...DBCluster.ReaderEndpoint: host_not_own

Cause 5 — rds-aurora-serverless-data-api-managed-secret-sql-round-trip#task#9, DescribeDBClusters, 200 -> 200:
  $...DBCluster.MasterUserSecret.SecretArn: value_mismatch (expected a shaped RDS-managed secret ARN, got an emulator-minted 'rds!cluster-cluster-00000000000000000000000001-…' ARN)
  $...DBCluster.DBClusterMembers.DBClusterMember.PromotionTier: missing (expected '1', got None)
  $...DBCluster.Port: value_mismatch (expected '5432', got '37473')
  $...DBCluster.ReaderEndpoint: host_not_own

Cause 6 — rds-global-cluster-delete-with-member#task#7, CreateDBCluster, 200 -> 200:
  $...DBCluster.GlobalClusterResourceId: value_mismatch (expected a shaped unique id, got 'cluster-00000000000000000000000001')
  $...DBCluster.Port: value_mismatch (expected '3306', got '42759')
  $...DBCluster.ReaderEndpoint: host_not_own

The ports the emulator returned (40185, 43513, 36417, 37473, 42759) are host ephemeral-range values and differ run to run; the recorded values are the engine defaults 3306/5432.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Cause 1: replay fixture rds-db-cluster-lifecycle-with-snapshot step task#1 — CreateDBCluster for a MySQL-family Aurora cluster with no explicit Port; read DBCluster.Port in the response.
3. Cause 2: same fixture, step task#3 — ModifyDBCluster the same cluster and read DBCluster.Port.
4. Cause 3: fixture rds-db-cluster-lifecycle step teardown#6 — DeleteDBCluster and read DBCluster.Port.
5. Cause 4: fixture rds-aurora-serverless-v2-config-read-back step task#2 — DescribeDBClusters on the aurora-postgresql Serverless v2 cluster and read DBCluster.Port.
6. Cause 5: fixture rds-aurora-serverless-data-api-managed-secret-sql-round-trip step task#9 — DescribeDBClusters on the managed-secret cluster and read DBClusterMembers[].PromotionTier, MasterUserSecret.SecretArn and Port.
7. Cause 6: fixture rds-global-cluster-delete-with-member step task#7 — CreateDBCluster with GlobalClusterIdentifier set and read DBCluster.GlobalClusterResourceId and Port.
Each step returns HTTP 200 with the diverging members, so the divergence is in the serialized values, not the status.
