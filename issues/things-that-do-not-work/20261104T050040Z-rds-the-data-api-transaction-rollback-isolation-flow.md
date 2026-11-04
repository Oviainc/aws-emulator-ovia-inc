Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.data-api-transaction-rollback-isolation
Issue-Key: rds|CreateDBCluster|NotImplemented
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: the Data API transaction-rollback isolation flow cannot be provisioned — CreateDBCluster refuses ManageMasterUserPassword with 501 NotImplemented

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
One root cause behind the unowned rds failure in this suite.

Cause 1 of 1: CreateDBCluster refuses the ManageMasterUserPassword member with HTTP 501 NotImplemented, so an Aurora cluster whose master password RDS manages in Secrets Manager cannot be created and the transaction-rollback isolation flow built on it never starts.

AWS: Smithy (com.amazonaws.rds, CreateDBClusterMessage$ManageMasterUserPassword) documents the member as "Specifies whether to manage the master user password with Amazon Web Services Secrets Manager", "Valid for Cluster Type: Aurora DB clusters and Multi-AZ DB clusters", with exactly one constraint: "Can't manage the master user password with Amazon Web Services Secrets Manager if MasterUserPassword is specified." A request setting it true and omitting MasterUserPassword succeeds with a 2xx CreateDBClusterResult, and the cluster reports DBCluster$MasterUserSecret (com.amazonaws.rds#MasterUserSecret: SecretArn, SecretStatus in creating|active|rotating|impaired, KmsKeyId). CreateDBCluster models no error for an unsupported member, and 'NotImplemented' is not an RDS error code.

Emulator: HTTP 501, error code 'NotImplemented', message "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead." The only variable part of that string is the leading operation$member token; the remainder is emulator-authored text with no AWS counterpart.

Why: an RDS client following the documented managed-master-password path receives a 501 carrying an error code its SDK does not model, instead of a cluster. The modeled MasterUserSecret contract is therefore never served by this service, and the dependent transaction-isolation behavior is unobservable.

Fix: the CreateDBCluster handler and the RDS-managed-secret minting it depends on — mint the secret in Secrets Manager, record it on the cluster, project DBCluster$MasterUserSecret, and drop the 501 branch, keeping only the modeled refusal when MasterUserPassword is supplied alongside.

Note on an existing issue: 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md describes this same refusal as its Cause 1 but is filed for the lambda__rds__secretsmanager group and carries only that group's replay keys; it does not carry rds.data-api-transaction-rollback-isolation, which is therefore unowned and filed here.

Traces: rds.data-api-transaction-rollback-isolation.

## Evidence
From the behavioral replay report (suite simparity-behavioral-aws-v1), service rds: 0/1 passed (0%), 1 failed [error].

rds.data-api-transaction-rollback-isolation:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBCluster operation: CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead.

Expected (AWS): HTTP 200 CreateDBClusterResult with DBCluster.MasterUserSecret{SecretArn, SecretStatus, KmsKeyId}.
Actual (emulator): HTTP 501 with Error.Code 'NotImplemented' and the message above; no cluster is created.

Smithy citations: CreateDBClusterMessage$ManageMasterUserPassword (documented behavior + the MasterUserPassword mutual-exclusion constraint); com.amazonaws.rds#MasterUserSecret {SecretArn, SecretStatus, KmsKeyId}.

## Steps to Reproduce
Build and boot the emulator, then replay rds.data-api-transaction-rollback-isolation.

Manually:
1. rds.create_db_cluster(DBClusterIdentifier='c1', Engine='aurora-mysql', MasterUsername='dbadmin', ManageMasterUserPassword=True, EnableHttpEndpoint=True, DBSubnetGroupName=..., ServerlessV2ScalingConfiguration={'MinCapacity':0.5,'MaxCapacity':2}) with no MasterUserPassword.
   Expected: 200 and DBCluster.MasterUserSecret.SecretArn.
   Actual: 501 (NotImplemented) "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: ...".
2. Verify the same request that ALSO supplies MasterUserPassword is still refused (modeled constraint).
3. With the cluster created, open a Data API transaction, write, roll back, and confirm the write is not visible — unreachable until step 1 succeeds.
