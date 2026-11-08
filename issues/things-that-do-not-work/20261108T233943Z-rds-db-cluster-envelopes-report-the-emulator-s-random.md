Issue-Source: trace-sweep-divergence
Issue-Trace: rds-aurora-serverless-v2-config-read-back
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-aurora-serverless-v2-config-read-back rds-db-cluster-lifecycle rds-db-cluster-lifecycle-with-snapshot rds-delete-requires-final-snapshot-param rds-duplicate-create-error rds-multi-cluster-isolation rds-db-cluster-state-roles-and-capacity-operations rds-global-cluster-delete-with-member
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.Port
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.Port
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.Port
Issue-Key: rds|ModifyDBCluster|2xx->2xx|$.ModifyDBClusterResponse.ModifyDBClusterResult.DBCluster.Port
Issue-Key: rds|DeleteDBCluster|2xx->2xx|$.DeleteDBClusterResponse.DeleteDBClusterResult.DBCluster.Status
Issue-Related: 20261106T224204Z-rds-createdbcluster-envelope-and-the-engine-default-parameter.md
Issue-Commit: 0d0b5d4835067c793fae3be07582601671bbaf82

# DB cluster envelopes report the emulator's random listener port instead of the engine port, and drop delete-time Status and GlobalClusterResourceId

> _Note: this issue was identified against the code base at commit `0d0b5d4835067c793fae3be07582601671bbaf82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
This bundle carries 6 net-new causes. They share ONE fault: the DBCluster envelope the rds service projects is filled from the emulator's own runtime values instead of from the cluster's requested/engine-derived configuration, so DBCluster.Port carries a random high port on every operation that returns a cluster, and two further delete/global-cluster fields are not projected at all.

Shared statement of the fault:

AWS: every operation that returns a DBCluster (CreateDBCluster, ModifyDBCluster, DescribeDBClusters, DeleteDBCluster) reports DBCluster.Port as the port the cluster's endpoint listens on — the `Port` given in the create/modify request, otherwise the engine's default: 3306 for aurora/aurora-mysql, 5432 for aurora-postgresql. Smithy: ModifyDBCluster/CreateDBCluster input member `Port` (IntegerOptional) and output DBCluster.Port; the default-per-engine rule is AWS-documented behavior and is what the recordings show (5432 for the aurora-postgresql fixtures, 3306 for the aurora-mysql ones). The requirement is the engine/requested port, not any one recorded number.

Emulator: the same field carries a per-run random high port — 40553, 42819, 43285, 39077 in this replay — i.e. the emulator's own ephemeral listener, which also changes between clusters in one run. Status is 200 in all six causes; only the body diverges.

Why: DBCluster.Port is half of the connection coordinate. A client that reads Endpoint+Port back from CreateDBCluster/DescribeDBClusters and hands it to a driver, a connection string, a security-group rule or an IaC output builds a target that is wrong for the engine it asked for, and a test that asserts 'aurora-postgresql listens on 5432' fails against the emulator while passing against AWS. The two missing fields break delete-flow and global-database clients the same way: the delete response's Status is how a client learns the cluster went to backing-up rather than deleting, and GlobalClusterResourceId is how it ties a regional cluster to its global cluster.

Fix: the rds service crate's DBCluster projection/serializer plus the cluster state record it reads from. Store the effective port on the cluster at create/modify time (request `Port` else engine default) and project that; project Status from the delete transition and GlobalClusterResourceId from the global-cluster membership. Never project a listener port the process happens to be bound to.

Cause 1 of 6: DescribeDBClusters.DBCluster.Port — expected the aurora-postgresql default 5432, got 40553. Trace: rds-aurora-serverless-v2-config-read-back#task#2 (cluster also read back by rds-db-cluster-lifecycle#task#5, rds-db-cluster-lifecycle-with-snapshot#task#2, rds-delete-requires-final-snapshot-param#task#3, rds-duplicate-create-error#task#3, rds-multi-cluster-isolation#task#3 and #task#4 — 7 steps in the cluster).

Cause 2 of 6: DeleteDBCluster.DBCluster.Port — expected 5432, got 43285. Trace: rds-db-cluster-lifecycle#teardown#6 (also rds-db-cluster-state-roles-and-capacity-operations#teardown#13, rds-multi-cluster-isolation#teardown#5 and #teardown#6).

Cause 3 of 6: CreateDBCluster.DBCluster.Port — expected the aurora-mysql default 3306, got 42819. Trace: rds-db-cluster-lifecycle-with-snapshot#task#1 (also rds-db-cluster-state-roles-and-capacity-operations#setup#1, rds-multi-cluster-isolation#setup#1 and #setup#2).

Cause 4 of 6: ModifyDBCluster.DBCluster.Port — expected 3306, got 42819. Trace: rds-db-cluster-lifecycle-with-snapshot#task#3.

Cause 5 of 6: DeleteDBCluster.DBCluster.Status is not projected for a delete that takes a final snapshot — expected 'backing-up', got 'available' (same step also shows Port 3306 vs 42819). Recorded behavior only: the model does not enumerate cluster statuses, and the recording shows AWS reporting the cluster already in 'backing-up' in the DeleteDBCluster response, whereas the emulator returns the pre-delete status unchanged. Trace: rds-db-cluster-lifecycle-with-snapshot#teardown#10.

Cause 6 of 6: CreateDBCluster.DBCluster.GlobalClusterResourceId is omitted for a cluster created into a global cluster — expected a resource id (the recording asserts its shape, not its value), got None (same step also shows Port 3306 vs 39077). Trace: rds-global-cluster-delete-with-member#task#7.

Related but NOT covering this: open issue 20261105T154005Z-rds-db-instance-envelopes-report-emulator-defaults-instead.md owns the DB *instance* Endpoint.Port divergence, and 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md owns the CreateGlobalCluster GlobalClusterResourceId shape cluster. The six causes above are the DB *cluster* envelope and the replay reports all six as unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (96 fixtures, 977 steps, 494 mismatched). All six clusters: verdict=mismatch, expected_status=200, actual_status=200.

Cause 1: divergence_key 'service=rds|DescribeDBClusters|status=200->200|keys=$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.Port'; detail '...DBCluster.Port: value_mismatch (expected 5432, got 40553)'; affected_count 7.
Cause 2: 'service=rds|DeleteDBCluster|...DBCluster.Port'; detail 'value_mismatch (expected 5432, got 43285)'; affected_count 4.
Cause 3: 'service=rds|CreateDBCluster|...DBCluster.Port'; detail 'value_mismatch (expected 3306, got 42819)'; affected_count 4.
Cause 4: 'service=rds|ModifyDBCluster|...DBCluster.Port'; detail 'value_mismatch (expected 3306, got 42819)'.
Cause 5: 'service=rds|DeleteDBCluster|...DBCluster.Port,...DBCluster.Status'; detail 'DBCluster.Status: value_mismatch (expected backing-up, got available)' and 'DBCluster.Port: value_mismatch (expected 3306, got 42819)'.
Cause 6: 'service=rds|CreateDBCluster|...DBCluster.GlobalClusterResourceId,...DBCluster.Port'; detail 'DBCluster.GlobalClusterResourceId: missing (expected <shaped-not-asserted>, got None)' and 'DBCluster.Port: value_mismatch (expected 3306, got 39077)'.

The four actual ports (40553/42819/43285/39077) are ephemeral and differ per run; the defect is that any such value appears where the engine/requested port belongs.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay rds-aurora-serverless-v2-config-read-back step task#2: DescribeDBClusters on the aurora-postgresql cluster returns DBCluster.Port = a random high port instead of 5432.
3. Replay rds-db-cluster-lifecycle-with-snapshot steps task#1 and task#3 (CreateDBCluster/ModifyDBCluster on aurora-mysql): Port is a random high port instead of 3306.
4. Replay rds-db-cluster-lifecycle teardown#6 (DeleteDBCluster): Port diverges again.
5. Replay rds-db-cluster-lifecycle-with-snapshot teardown#10 (DeleteDBCluster with a final snapshot): DBCluster.Status is 'available' instead of 'backing-up'.
6. Replay rds-global-cluster-delete-with-member step task#7 (CreateDBCluster into a global cluster): DBCluster.GlobalClusterResourceId is absent.
