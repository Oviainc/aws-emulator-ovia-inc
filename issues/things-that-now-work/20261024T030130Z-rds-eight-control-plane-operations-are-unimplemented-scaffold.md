Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.concurrent-modify-db-cluster-conflicting-state-rejected rds.copy-cluster-parameter-group rds.create-blue-green-deployment-contract rds.delete-blue-green-deployment-boundaries rds.create-integration-boundaries rds.delete-tenant-database-boundary rds.deregister-proxy-targets rds.describe-cluster-automated-backups rds.log-file-access-on-not-ready-instance-error
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.concurrent-modify-db-cluster-conflicting-state-rejected|run#14|AssertionError|assert len(successes) == 1, f"expected exactly 1 success, got {len(successes)}: {outcomes!r}"
Issue-Key: rds.copy-cluster-parameter-group|run#13|AssertionError|assert code == ERR_DB_PARAMETER_GROUP_NOT_FOUND, f"unexpected error code: {code!r}"
Issue-Key: rds.create-blue-green-deployment-contract|run#6|AssertionError|assert code == ERR_DB_INSTANCE_NOT_FOUND, f"unexpected error code: {code}"
Issue-Key: rds.delete-blue-green-deployment-boundaries|run#4|AssertionError|assert code == ERR_INVALID_PARAMETER_VALUE, f"unexpected error code: {code}"
Issue-Key: rds.create-integration-boundaries|run#7|AssertionError|assert code == ERR_DB_CLUSTER_NOT_FOUND, f"unexpected error code: {code}"
Issue-Key: rds.delete-tenant-database-boundary|run#4|AssertionError|assert missing_code == ERR_DB_INSTANCE_NOT_FOUND, (
Issue-Key: rds.deregister-proxy-targets|run#4|AssertionError|assert code == ERR_PROXY_NOT_FOUND, f"unexpected error code: {code}"
Issue-Key: rds.describe-cluster-automated-backups|run#4|AssertionError|assert code == ERR_CLUSTER_AUTOMATED_BACKUP_NOT_FOUND, (
Issue-Key: rds.log-file-access-on-not-ready-instance-error|run#13|AssertionError|assert observed == ("DBInstanceNotFound",) * 3, (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# RDS: eight control-plane operations are unimplemented scaffold stubs and answer NotImplemented

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBCluster

## Description
Eight RDS operations that the behavioral suite exercises are still scaffold stubs: each answers HTTP 501-class `NotImplemented` with the body 'operation <X> is scaffolded but not implemented in this emulator' instead of the modeled success shape or the modeled awsQuery error. None of these eight is covered by an auto-filed stub issue in this suite run (the auto-filed stubs name CreateDBParameterGroup, CreateDBSecurityGroup, CopyOptionGroup, DeleteDBSubnetGroup, CreateDBShardGroup, DescribeDBShardGroups, StopDBInstance, DescribeExportTasks, DescribeAccountAttributes, DescribeEngineDefaultParameters, DescribeEngineDefaultClusterParameters, DescribeDBMajorEngineVersions, ModifyEventSubscription, DescribeValidDBInstanceModifications and CreateCustomDBEngineVersion — a different set). Eight causes.

Cause 1 of 8: ModifyDBCluster. AWS: Smithy AmazonRDSv19 models ModifyDBCluster returning DBCluster, with InvalidDBClusterStateFault (client, httpError 400, awsQueryError code InvalidDBClusterStateFault) among its errors; two concurrent ModifyDBCluster calls on one cluster serialize so that exactly one is accepted and the loser gets InvalidDBClusterStateFault. Emulator: BOTH racers get code 'NotImplemented'. Why: a client cannot modify an Aurora cluster at all, and concurrency-sensitive clients never see the state conflict they branch on. Fix: the ModifyDBCluster handler and the cluster record in the RDS state module. Traces: rds.concurrent-modify-db-cluster-conflicting-state-rejected.

Cause 2 of 8: CopyDBClusterParameterGroup. AWS: the operation copies a cluster parameter group, carrying the source's customized parameter values into the target, and rejects an unknown source with DBParameterGroupNotFoundFault (awsQueryError code DBParameterGroupNotFound, httpError 404). Emulator: code 'NotImplemented' for the unknown-source probe. Why: a client that clones a tuned cluster parameter group before an upgrade has no path, and its not-found error handling never fires. Fix: the CopyDBClusterParameterGroup handler, reading/writing the cluster-parameter-group records already in the RDS state module. Traces: rds.copy-cluster-parameter-group.

Cause 3 of 8: CreateBlueGreenDeployment and DeleteBlueGreenDeployment. AWS: Smithy models CreateBlueGreenDeployment with Source constrained to DatabaseArn (length 1..2048, pattern ^arn:[A-Za-z][0-9A-Za-z-:._]*$) and errors including DBInstanceNotFoundFault (404, awsQueryError DBInstanceNotFound); DeleteBlueGreenDeployment constrains BlueGreenDeploymentIdentifier (length 1..255, pattern ^[A-Za-z][0-9A-Za-z-:._]*$) and models BlueGreenDeploymentNotFoundFault (404) plus InvalidBlueGreenDeploymentStateFault (400). Recorded behavior: a well-formed Source ARN naming a missing instance -> DBInstanceNotFound; a Source that is not an ARN -> a DIFFERENT code; an arbitrary (non system-generated) BlueGreenDeploymentIdentifier -> InvalidParameterValue, a well-shaped but unknown one -> BlueGreenDeploymentNotFoundFault. Emulator: both operations answer 'NotImplemented'. Why: the entire blue/green deployment surface is unusable, and the two distinct validation layers collapse into one wrong code. Fix: the CreateBlueGreenDeployment and DeleteBlueGreenDeployment handlers and their input validation. Traces: rds.create-blue-green-deployment-contract, rds.delete-blue-green-deployment-boundaries.

Cause 4 of 8: CreateIntegration (Aurora zero-ETL). AWS: Smithy models CreateIntegration with SourceArn (length 1..255, pattern ^arn:aws[a-z\-]*:rds(-[a-z]*)?:[a-z0-9\-]*:[0-9]*:(cluster|db):...$), TargetArn (length 20..2048) and errors DBClusterNotFoundFault (404, code DBClusterNotFoundFault) and KMSKeyNotAccessibleFault (400); recorded behavior shows TargetArn syntax validation runs BEFORE the source lookup, so a malformed TargetArn with an equally missing SourceArn yields InvalidParameterValue. Emulator: 'NotImplemented' for both probes, so neither code nor ordering can be observed. Fix: the CreateIntegration handler plus its ARN-syntax validation, ordered before the source lookup. Traces: rds.create-integration-boundaries.

Cause 5 of 8: DeleteTenantDatabase. AWS: Smithy models errors DBInstanceNotFoundFault (404, awsQueryError DBInstanceNotFound), TenantDatabaseNotFoundFault (404) and InvalidDBInstanceStateFault (400); recorded behavior answers a nonexistent DBInstanceIdentifier with DBInstanceNotFound and an ordinary non-CDB instance with a code distinct from not-found (InvalidParameterCombination). Emulator: 'NotImplemented' for the nonexistent-instance probe. Fix: the DeleteTenantDatabase handler and its instance-existence / multi-tenant precondition. Traces: rds.delete-tenant-database-boundary.

Cause 6 of 8: DeregisterDBProxyTargets (and the register/describe round trip it anchors). AWS: deregistering against an unknown DBProxyName rejects with DBProxyNotFoundFault; against a real proxy it removes the instance from DescribeDBProxyTargets. Emulator: 'NotImplemented'. Fix: the DeregisterDBProxyTargets handler and the proxy-target records in the RDS state module. Traces: rds.deregister-proxy-targets.

Cause 7 of 8: DescribeDBClusterAutomatedBackups. AWS: filtering by an unknown DBClusterIdentifier rejects with DBClusterAutomatedBackupNotFoundFault; a real available Aurora cluster with BackupRetentionPeriod>0 has at least one automated-backup record describable by DBClusterIdentifier. Emulator: 'NotImplemented'. Fix: the DescribeDBClusterAutomatedBackups handler and the automated-backup record the cluster lifecycle must produce. Traces: rds.describe-cluster-automated-backups.

Cause 8 of 8: DescribeDBLogFiles and DownloadDBLogFilePortion. AWS: Smithy models DescribeDBLogFiles -> DescribeDBLogFilesResponse {DescribeDBLogFiles: DescribeDBLogFilesList, Marker} with errors DBInstanceNotFoundFault (404, awsQueryError DBInstanceNotFound) and DBInstanceNotReadyFault (400); recorded behavior: against an instance still in 'creating', and against an identifier that never existed, BOTH log operations answer DBInstanceNotFound, and DescribeDBLogFiles succeeds with a list once the instance is available. Emulator: all three probes answer 'NotImplemented'. Fix: the DescribeDBLogFiles and DownloadDBLogFilePortion handlers and their instance lookup. Traces: rds.log-file-access-on-not-ready-instance-error.

## Evidence
Cause 1: rds.concurrent-modify-db-cluster-conflicting-state-rejected — AssertionError: expected exactly 1 success, got 0: [{'ok': False, 'value': None, 'code': 'NotImplemented'}, {'ok': False, 'value': None, 'code': 'NotImplemented'}].
Cause 2: rds.copy-cluster-parameter-group — AssertionError: unexpected error code: 'NotImplemented' (expected 'DBParameterGroupNotFound').
Cause 3: rds.create-blue-green-deployment-contract — AssertionError: unexpected error code: NotImplemented (expected 'DBInstanceNotFound'); rds.delete-blue-green-deployment-boundaries — AssertionError: unexpected error code: NotImplemented (expected 'InvalidParameterValue').
Cause 4: rds.create-integration-boundaries — AssertionError: unexpected error code: NotImplemented (expected 'DBClusterNotFoundFault').
Cause 5: rds.delete-tenant-database-boundary — AssertionError: unexpected error code for a nonexistent instance: NotImplemented (expected 'DBInstanceNotFound').
Cause 6: rds.deregister-proxy-targets — AssertionError: unexpected error code: NotImplemented (expected 'DBProxyNotFoundFault').
Cause 7: rds.describe-cluster-automated-backups — AssertionError: expected DBClusterAutomatedBackupNotFoundFault, got NotImplemented.
Cause 8: rds.log-file-access-on-not-ready-instance-error — AssertionError: expected DescribeDBLogFiles/DownloadDBLogFilePortion on the not-yet-ready instance and DescribeDBLogFiles on a never-existed identifier to fail with DBInstanceNotFound, got ('NotImplemented', 'NotImplemented', 'NotImplemented').
All eight answer the scaffold body 'operation <X> is scaffolded but not implemented in this emulator' with error code NotImplemented and Type 'Receiver'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay the behavioral suite and re-run the nine checks listed in trace_ids.
3. Or by hand, against the emulator endpoint: Action=ModifyDBCluster on any cluster; Action=CopyDBClusterParameterGroup with an unknown SourceDBClusterParameterGroupIdentifier; Action=CreateBlueGreenDeployment with Source=arn:aws:rds:<region>:<acct>:db:<missing>; Action=DeleteBlueGreenDeployment with BlueGreenDeploymentIdentifier=anything; Action=CreateIntegration with a missing cluster SourceArn; Action=DeleteTenantDatabase with a missing DBInstanceIdentifier; Action=DeregisterDBProxyTargets with an unknown DBProxyName; Action=DescribeDBClusterAutomatedBackups with an unknown DBClusterIdentifier; Action=DescribeDBLogFiles / DownloadDBLogFilePortion on any identifier.
4. Each returns error code NotImplemented instead of the modeled result or modeled awsQuery error.
