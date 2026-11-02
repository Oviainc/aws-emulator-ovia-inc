Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.refarch-aur-e2e-review-aurora-production-readiness-lifecycle rds__secretsmanager.refarch-aur-protected-cluster-blocks-stack-delete-then-succeeds-after-unprotect
Issue-Key: rds__secretsmanager|DescribeDBClusters|DBClusterNotFoundFault
Issue-Commit: 2bfbb03c7bd3982d98fd2c53f75176784e229027

# rds__secretsmanager: a CloudFormation-created AWS::RDS::DBCluster does not exist in RDS — the stack reaches CREATE_COMPLETE and outputs the cluster identifier, but DescribeDBClusters answers 404 DBClusterNotFoundFault

> _Note: this issue was identified against the code base at commit `2bfbb03c7bd3982d98fd2c53f75176784e229027`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
Cause 1 of 1: a CloudFormation stack that declares an AWS::RDS::DBCluster reaches CREATE_COMPLETE and serves outputs derived from that resource (Ref -> the cluster identifier, Fn::GetAtt Endpoint.Address / ReadEndpoint.Address), but the RDS service holds no such cluster: the very next DescribeDBClusters on the identifier the stack reported is refused as not found.

AWS: once a stack containing AWS::RDS::DBCluster reports CREATE_COMPLETE, the resource's PhysicalResourceId / Ref value names a DB cluster that RDS itself resolves. Smithy (AmazonRDSv19) models DescribeDBClusters(DBClusterIdentifier) -> DBClusterMessage{DBClusters} and DBClusterNotFoundFault (errorKind client, httpError 404, awsQueryError code DBClusterNotFoundFault) as the answer for an identifier that does NOT exist; CreateDBCluster -> DBCluster is the path a provisioner must take, and an Aurora Serverless v2 cluster created this way settles at Status 'available' carrying DeletionProtection, ServerlessV2ScalingConfiguration, MasterUsername, Endpoint, ReaderEndpoint, ClusterCreateTime and DBClusterMembers. The not-found refusal itself is correctly shaped — code DBClusterNotFoundFault at HTTP 404 with the fixed text "DBCluster <identifier> not found." where <identifier> is the DBClusterIdentifier from the request — so the divergence is that this refusal is returned at all for a cluster CloudFormation has just reported as created.

Emulator: both failing checks deploy the carved Aurora Serverless v2 template (AWS::RDS::DBCluster with Engine=aurora-postgresql, ServerlessV2ScalingConfiguration MinCapacity 0.5 / MaxCapacity 1, DeletionProtection=true, StorageEncrypted=true, EnableHttpEndpoint=true, plus an AWS::SecretsManager::Secret), get past CREATE_COMPLETE and read the ClusterIdentifier/SecretArn outputs, and then every rds.DescribeDBClusters on that identifier raises DBClusterNotFoundFault ("DBCluster <prefix>-cluster not found."). So the stack claims the cluster exists while RDS does not know it.

Why: a client that provisions Aurora through CloudFormation and then addresses the cluster through the RDS API — to wait for Status 'available', read ServerlessV2ScalingConfiguration / MasterUsername / DBClusterMembers / Endpoint / ReaderEndpoint, audit DeletionProtection, call ModifyDBCluster to clear protection, or confirm the cluster is gone after the stack delete — breaks immediately against a resource the stack reported as created. It also makes the whole deletion-protection contract unreachable: DeleteStack on a protected cluster can never end DELETE_FAILED naming the cluster, nor can DeleteDBCluster be refused with InvalidParameterCombination "Cannot delete protected Cluster, please disable deletion protection and try again.", because there is no cluster to protect.

Fix: the CloudFormation resource provisioner for AWS::RDS::DBCluster (the resource-provider/provisioning module of services/service-com-amazonaws-cloudformation-cloudformation) — it must create the cluster through the RDS service state (the same state CreateDBCluster writes, services/service-com-amazonaws-rds-amazonrdsv19) and record that identifier as the resource's PhysicalResourceId, so RDS answers DescribeDBClusters for it; its delete path must likewise go through RDS and honour DeletionProtection. A closely related but DIFFERENT resource type is already filed as 20261024T030130Z-rds-s3-rds-snapshot-export-to-s3-and-restore-from.md (cause 3: a CFN-created AWS::RDS::DBClusterParameterGroup is absent from RDS); that issue does not carry AWS::RDS::DBCluster or either check below. 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md covers CreateDBCluster's 501 refusal of ManageMasterUserPassword and a CFN-created AWS::Lambda::Function being absent from Lambda — neither is this cause (this template passes MasterUserPassword by hand and declares no function).

Traces: rds__secretsmanager.refarch-aur-e2e-review-aurora-production-readiness-lifecycle, rds__secretsmanager.refarch-aur-protected-cluster-blocks-stack-delete-then-succeeds-after-unprotect.

## Evidence
Both quoted from the behavioral replay report (suite simparity-behavioral-aws-v1), service group rds__secretsmanager (0/15 passed); neither check is marked ALREADY FILED or OWNED.

1) rds__secretsmanager.refarch-aur-e2e-review-aurora-production-readiness-lifecycle [error]:
_PhaseError: [phase 1/4 deploy] DBClusterNotFoundFault: An error occurred (DBClusterNotFoundFault) when calling the DescribeDBClusters operation: DBCluster sim2real-aur-est-4e5c6a21af52-cluster not found.
The phase had already asserted that the stack output BOTH the cluster identifier and the secret ARN (assert est["cluster"] and est["secret"]) and that the stack reached CREATE_COMPLETE, so the stack reported the cluster as created before RDS denied it. Expected: describe_db_clusters(DBClusterIdentifier=<stack output>) returns a DBCluster that polls to Status 'available' with DeletionProtection True, ServerlessV2ScalingConfiguration {MinCapacity 0.5, MaxCapacity 1.0}, MasterUsername 'dbadmin', no MasterUserSecret, 0 DBClusterMembers and a ReaderEndpoint carrying the 'cluster-ro-' infix.

2) rds__secretsmanager.refarch-aur-protected-cluster-blocks-stack-delete-then-succeeds-after-unprotect [error]:
botocore.errorfactory.DBClusterNotFoundFault: An error occurred (DBClusterNotFoundFault) when calling the DescribeDBClusters operation: DBCluster sim2real-aur-dp-7d423c289bee-cluster not found.
Raised immediately after _wait_stack(..., "CREATE_COMPLETE") returned and outs["ClusterIdentifier"] was read. Expected: the cluster is describable with DeletionProtection True; DeleteStack then ends DELETE_FAILED with StackStatusReason "The following resource(s) failed to delete: [AuroraCluster]. ", a DELETE_FAILED stack event for logical id AuroraCluster whose PhysicalResourceId is that cluster identifier and whose ResourceStatusReason cites deletion protection, the cluster still 'available', and the unrelated secret removed (DescribeSecret -> ResourceNotFoundException).

The error code, HTTP status and message wording of the refusal match AWS (DBClusterNotFoundFault / 404 / "DBCluster <identifier> not found.", with <identifier> taken from the request); the divergence is that it is returned for a cluster CloudFormation reported as created. The cluster identifiers above are per-run generated names and are not part of the requirement.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateStack with a template whose Resources contain an AWS::RDS::DBCluster (Engine aurora-postgresql, EngineVersion from DescribeDBEngineVersions, DatabaseName appdb, MasterUsername dbadmin, MasterUserPassword supplied, ServerlessV2ScalingConfiguration {MinCapacity 0.5, MaxCapacity 1}, DeletionProtection true, StorageEncrypted true, EnableHttpEndpoint true) and an AWS::SecretsManager::Secret, with Outputs ClusterIdentifier {Ref AuroraCluster}, ClusterEndpoint/ReaderEndpoint via Fn::GetAtt and SecretArn {Ref MasterSecret}.
3. Poll DescribeStacks until CREATE_COMPLETE and read the ClusterIdentifier output.
4. Call Action=DescribeDBClusters with DBClusterIdentifier=<that output> — the emulator answers HTTP 404 DBClusterNotFoundFault "DBCluster <identifier> not found." instead of a DBCluster record that reaches Status 'available'.
5. Consequently DeleteStack on the stack can never end DELETE_FAILED naming AuroraCluster for deletion protection, and ModifyDBCluster(DeletionProtection=false)/DeleteDBCluster on that identifier are unreachable.
6. Or replay the two checks in trace_ids.
