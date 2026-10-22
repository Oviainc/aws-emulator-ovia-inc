Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.replication-group-encryption-snapshot-and-tags-round-trip
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateAlias|NotImplemented
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# KMS CreateAlias answers NotImplemented, so the KMS-encrypted ElastiCache replication-group round trip cannot run

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
Cause 1 of 1: kms CreateAlias is a scaffold stub, which blocks the only behavioral check that exercises an ElastiCache replication group created with at-rest encryption under a customer KMS key referenced by alias.
AWS: KMS CreateAlias returns an empty 200 response for a valid alias name and an existing TargetKeyId (recorded behavior only for this flow: CreateKey succeeded, then CreateAlias was called with AliasName='alias/<rg-id>' and TargetKeyId from that key). ElastiCache then resolves the alias and reports ReplicationGroup.KmsKeyId as the key's ARN.
Emulator: CreateAlias answers error code `NotImplemented` with message "operation CreateAlias is scaffolded but not implemented in this emulator", so the check aborts before CreateReplicationGroup is ever issued.
Why: any client that names a KMS key by alias — the normal pattern for encrypted ElastiCache replication groups — cannot proceed at all, and the alias-to-ARN resolution that ElastiCache must perform (KmsKeyId echoed back as the key ARN, immutable across a later ModifyReplicationGroup) is untestable.
Fix: the kms CreateAlias handler and the alias table in kms state (plus alias resolution where ElastiCache stores KmsKeyId).
Traces: elasticache.replication-group-encryption-snapshot-and-tags-round-trip.

Note: this check's service token is `elasticache` (the check id's leading token), but the failing operation belongs to kms; the ElastiCache side of the flow was never reached.

## Evidence
elasticache.replication-group-encryption-snapshot-and-tags-round-trip errored at the kms setup step:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAlias operation: operation CreateAlias is scaffolded but not implemented in this emulator
The preceding kms CreateKey call succeeded (KeyMetadata with KeyId/Arn returned), so only CreateAlias is missing. Expected: CreateAlias succeeds, and the subsequent CreateReplicationGroup(AtRestEncryptionEnabled=True, KmsKeyId='alias/<rg-id>', SnapshotRetentionLimit=7, SnapshotWindow='06:30-07:30', Tags=[...]) settles available with KmsKeyId equal to the key ARN.

## Steps to Reproduce
1. kms CreateKey -> note KeyId/Arn.
2. kms CreateAlias(AliasName='alias/<name>', TargetKeyId=<KeyId>) -> expect a 200 empty response; observe NotImplemented.
3. (blocked) elasticache CreateReplicationGroup with AtRestEncryptionEnabled=True and KmsKeyId='alias/<name>', wait for available, and assert DescribeReplicationGroups reports KmsKeyId as the key ARN.
