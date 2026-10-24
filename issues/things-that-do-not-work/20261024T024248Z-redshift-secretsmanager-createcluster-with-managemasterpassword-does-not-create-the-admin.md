Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__secretsmanager.managed-admin-password-creates-a-rotatable-secret-that-cannot-be-deleted
Issue-Key: secrets-manager|DescribeSecret|ResourceNotFoundException
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CreateCluster with ManageMasterPassword does not create the admin secret in Secrets Manager: DescribeSecret on the reported MasterPasswordSecretArn answers ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: the Redshift-managed admin password secret is reported on the cluster but never registered with Secrets Manager.
AWS: CreateCluster with ManageMasterPassword=true makes Redshift mint a managed admin secret and populate Cluster.MasterPasswordSecretArn; that ARN names a real secret in Secrets Manager, so secretsmanager:DescribeSecret on it succeeds and returns the secret's metadata (with KmsKeyId omitted when no customer key was supplied, and rotation/owning-service metadata set). The secret is owned by Redshift and cannot be deleted directly. Smithy: CreateCluster input member ManageMasterPassword (BooleanOptional) and MasterPasswordSecretKmsKeyId exist on the operation; the cross-service linkage and the DescribeSecret success are recorded behavior.
Emulator: the DescribeSecret call made against the ARN the cluster reported fails with error code `ResourceNotFoundException`, message example "Secrets Manager can't find the specified secret." — the secret the Redshift side advertises does not exist in the Secrets Manager service's state.
Why: a client that follows MasterPasswordSecretArn to fetch or rotate the cluster's admin credential gets a 'not found' for a secret the cluster says it owns. Credential retrieval, rotation and the "this secret is service-managed and cannot be deleted" protection are all unreachable, and nothing distinguishes a dangling ARN from a permissions problem.
Fix: the CreateCluster handler's managed-password path must create the secret through the secretsmanager service's state (owning-service / rotation metadata included) and only then publish its ARN on the Cluster projection; the secretsmanager side must refuse direct deletion of a Redshift-owned secret.
Traces: redshift__secretsmanager.managed-admin-password-creates-a-rotatable-secret-that-cannot-be-deleted.

## Evidence
redshift__secretsmanager.managed-admin-password-creates-a-rotatable-secret-that-cannot-be-deleted:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeSecret operation: Secrets Manager can't find the specified secret.

Expected: after CreateCluster(ManageMasterPassword=True), DescribeSecret(SecretId=<Cluster.MasterPasswordSecretArn>) succeeds and returns the managed secret (KmsKeyId omitted when MasterPasswordSecretKmsKeyId was not supplied, per the check's blessed notes).
Actual: the ARN is reported by Redshift but resolves to no secret in Secrets Manager -> ResourceNotFoundException.

## Steps to Reproduce
1. Boot the emulator; point redshift and secretsmanager clients at it.
2. Call CreateCluster with ManageMasterPassword=true and no MasterPasswordSecretKmsKeyId.
3. Read Cluster.MasterPasswordSecretArn from the response (or from DescribeClusters).
4. Call secretsmanager DescribeSecret with that ARN. Expect the managed secret's metadata; observe ResourceNotFoundException.
5. Then call secretsmanager DeleteSecret on it and expect the service-managed refusal.
6. Replay redshift__secretsmanager.managed-admin-password-creates-a-rotatable-secret-that-cannot-be-deleted to confirm.
