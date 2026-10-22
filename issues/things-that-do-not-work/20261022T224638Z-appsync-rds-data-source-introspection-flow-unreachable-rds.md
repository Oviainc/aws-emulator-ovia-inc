Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.e2e-data-source-introspection-driven-resolver-flow
Issue-Key: appsync|CreateDBCluster|NotImplemented
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# AppSync RDS data-source introspection flow unreachable: RDS CreateDBCluster answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue carries ONE root cause.

Cause 1 of 1: RDS CreateDBCluster is an unimplemented scaffold stub, so AppSync's RDS-backed data-source surface (StartDataSourceIntrospection -> CreateDataSource -> CreateResolver -> GraphQL query) can never be exercised. The defective handler lives in the rds crate, but the failing check id is an appsync.* one, so it is filed under service appsync.
AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model): CreateDBCluster requires DBClusterIdentifier and Engine, accepts EnableHttpEndpoint, ServerlessV2ScalingConfiguration, DatabaseName, MasterUsername/MasterUserPassword, StorageEncrypted and Tags, and returns CreateDBClusterResult {DBCluster} on HTTP 200 with DBClusterArn populated; its modeled errors are the awsQuery RDS fault family (DBClusterAlreadyExistsFault 400, DBSubnetGroupNotFoundFault 404, ...). NotImplemented is not one of them.
Emulator: answers HTTP 501 with error code NotImplemented and message "operation CreateDBCluster is scaffolded but not implemented in this emulator" (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster.rs carries the SCAFFOLDED marker).
Why: an AppSync client that points StartDataSourceIntrospection at an Aurora cluster to discover a table's columns, then builds a resolver over the RDS Data API from those discovered field names, cannot reach any AppSync call: provisioning fails first with an unmodeled error code and a 501 status. The consequence for fidelity is that the emulator's AppSync introspection/resolver execution path is asserted by nothing — a stubbed 200 on CreateResolver would look identical to a working resolver.
Fix: the operation's handler plus DB-cluster state in the rds crate (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster.rs and that crate's cluster store); once a cluster with an HTTP endpoint exists, the AppSync data-source introspection and resolver path becomes testable.
Traces: appsync.e2e-data-source-introspection-driven-resolver-flow.
NOTE: the replay report auto-filed a CreateDBCluster stub issue against the single rds.* check it blocked; that issue does not carry this appsync replay key.

## Evidence
appsync.e2e-data-source-introspection-driven-resolver-flow: "AssertionError: could not create Aurora Serverless v2 cluster: An error occurred (NotImplemented) when calling the CreateDBCluster operation: operation CreateDBCluster is scaffolded but not implemented in this emulator" — raised at the rds.create_db_cluster(...) call the check wraps, before CreateGraphqlApi/StartDataSourceIntrospection.
Expected: HTTP 200 with DBCluster.DBClusterArn (CreateDBClusterResult in the RDS Smithy model).
Per-service pass rate from the replay: appsync 0/1 (0%).

## Steps to Reproduce
1. secretsmanager.create_secret with {"username":...,"password":...} -> 200 (works).
2. rds.create_db_cluster(DBClusterIdentifier='appsync-dsi', Engine='aurora-mysql', MasterUsername='appsyncadmin', MasterUserPassword='<pw>', DatabaseName='testdb', EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration={'MinCapacity':0.5,'MaxCapacity':2}, StorageEncrypted=True) -> expect HTTP 200 with DBCluster.DBClusterArn; observe HTTP 501 error code NotImplemented.
3. (unreachable) appsync.start_data_source_introspection(rdsDataApiConfig={resourceArn, secretArn, databaseName}) and then query the resolver through the GraphQL endpoint.
