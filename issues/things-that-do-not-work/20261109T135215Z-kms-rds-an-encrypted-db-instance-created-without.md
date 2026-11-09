Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__rds.storage-encrypted-without-kms-key-uses-aws-managed-key
Issue-Key: kms__rds.storage-encrypted-without-kms-key-uses-aws-managed-key|run#8|KeyError|key_arn = inst["KmsKeyId"]
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# kms__rds: an encrypted DB instance created without KmsKeyId reports no KmsKeyId at all instead of the aws/rds managed key ARN

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Cause 1 of 1: StorageEncrypted=true with no KmsKeyId leaves KmsKeyId absent from the DBInstance.

AWS: CreateDBInstance with StorageEncrypted=true and no KmsKeyId encrypts the instance under the account's AWS-managed key for RDS (alias aws/rds in the instance's region), and every subsequent DescribeDBInstances projection carries KmsKeyId set to that key's ARN - the member is always present on an encrypted instance. Smithy: CreateDBInstance.KmsKeyId is optional and the DBInstance output member KmsKeyId is the key identifier the instance is encrypted under; the documented default when the member is omitted is the account's default (AWS-managed) RDS key.

Emulator: the created instance's DescribeDBInstances projection has no KmsKeyId member at all (KeyError: 'KmsKeyId'), so the key the instance is supposedly encrypted under is unknowable.

Why: a client that reads KmsKeyId to audit encryption, to grant the key to another account, or to copy the snapshot cross-region crashes on a missing field, and cannot tell an encrypted instance from an unencrypted one by its key.

Fix: the CreateDBInstance handler's encryption defaulting (resolve the aws/rds managed key ARN when StorageEncrypted is true and KmsKeyId is absent) and the DBInstance serializer that must then emit KmsKeyId.

Traces: kms__rds.storage-encrypted-without-kms-key-uses-aws-managed-key.

## Evidence
Expected: DescribeDBInstances -> DBInstances[0]['KmsKeyId'] is the ARN of the account's aws/rds AWS-managed key.
Actual: KeyError: 'KmsKeyId' - the member is absent from the instance body.

## Steps to Reproduce
1. CreateDBInstance with StorageEncrypted=true and no KmsKeyId.
2. Wait for 'available' and call DescribeDBInstances; observe KmsKeyId is missing.
