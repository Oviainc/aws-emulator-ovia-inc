Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes
Issue-Key: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes|run#16|AssertionError|assert secret_arn, (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# RDS ManageMasterUserPassword=true returns no MasterUserSecret, so no managed master secret is ever created

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
One cause. AWS: Smithy AmazonRDSv19 models ManageMasterUserPassword (BooleanOptional) and MasterUserSecretKmsKeyId on CreateDBInstance/ModifyDBInstance, and the DBInstance shape carries MasterUserSecret: MasterUserSecret {SecretArn ('The Amazon Resource Name (ARN) of the secret'), SecretStatus (creating|active|rotating|impaired), KmsKeyId ('The Amazon Web Services KMS key identifier that is used to encrypt the secret')}. When ManageMasterUserPassword=true, RDS creates a Secrets Manager secret it owns, encrypts it under the given (or default) KMS key, and reports it back on the instance as MasterUserSecret.SecretArn — that ARN is the only way a client can find the generated password, and the secret RDS owns rejects direct PutSecretValue/UpdateSecret writes by the customer.

Emulator: CreateDBInstance with ManageMasterUserPassword=true returns 200 with a DBInstance that has NO MasterUserSecret member at all (the returned structure contains DBInstanceIdentifier, Endpoint, DBParameterGroups, OptionGroupMemberships, StorageEncrypted=false, DbiResourceId, DBInstanceArn and so on, but no MasterUserSecret). Nothing is created in Secrets Manager, so the downstream assertions about the managed secret — that it exists, that it is encrypted under the named CMK, and that direct writes to it are refused — cannot even be attempted.

Why: a client that opts into RDS-managed master credentials has no way to obtain the credentials it needs to connect, because the SecretArn it must read is absent; tooling that reads MasterUserSecret.SecretStatus to wait for the secret, or that grants its application read access to that ARN, gets null.

Fix: the CreateDBInstance handler (and the matching ModifyDBInstance path) must, when ManageMasterUserPassword is true, create the managed secret through the Secrets Manager service state under the resolved MasterUserSecretKmsKeyId, mark it RDS-owned so direct customer writes are refused, and record it on the instance record so the DBInstance projection in the RDS state module emits MasterUserSecret {SecretArn, SecretStatus, KmsKeyId}.

Traces: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes.

## Evidence
AssertionError: ManageMasterUserPassword=True must return a MasterUserSecret.SecretArn: {'DBInstanceIdentifier': 'sim2real-xsvc-msec-...', 'DBInstanceClass': 'db.t4g.micro', 'Engine': 'postgres', 'DBInstanceStatus': 'creating', 'MasterUsername': 'sim2real', ... 'StorageEncrypted': False, 'DbiResourceId': 'db-...', 'CACertificateIdentifier': 'rds-ca-rsa2048-g1', ... 'DBInstanceArn': 'arn:aws:rds:us-west-2:...:db:sim2real-xsvc-msec-...', ...} — the response carries no MasterUserSecret member.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. kms: CreateKey (to use as MasterUserSecretKmsKeyId).
3. rds: CreateDBSubnetGroup, then Action=CreateDBInstance with ManageMasterUserPassword=true, MasterUserSecretKmsKeyId=<key>, and no MasterUserPassword.
4. Read the returned DBInstance (and DescribeDBInstances afterwards): there is no MasterUserSecret member, and secretsmanager ListSecrets shows no RDS-managed secret for the instance.
