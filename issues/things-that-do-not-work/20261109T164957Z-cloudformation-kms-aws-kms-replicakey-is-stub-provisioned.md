Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kms.deploy-kms-replicakey-cfn-provisions-replica-then-tombstones cloudformation__kms.deploy-kms-replicakey-cfn-non-multiregion-primary-rejected
Issue-Key: cloudformation__kms.deploy-kms-replicakey-cfn-provisions-replica-then-tombstones|run#26|AssertionError|assert replicas == [{"Arn": replica_arn, "Region": replica_region}], (
Issue-Key: cloudformation__kms.deploy-kms-replicakey-cfn-non-multiregion-primary-rejected|run#15|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# cloudformation__kms: AWS::KMS::ReplicaKey is stub-provisioned — no KMS replica key is created, the primary lists no replica, and a non-multi-Region PrimaryKeyArn is never refused

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack containing an AWS::KMS::ReplicaKey resource reaches CREATE_COMPLETE without the KMS ReplicateKey call ever happening, so no replica key exists, the resource's identifiers are synthesized, and the property validation KMS performs is never reached.
AWS: recorded behavior only for the CloudFormation registry handler (the KMS model itself is the authority for the underlying call). AWS::KMS::ReplicaKey's handler calls KMS ReplicateKey against PrimaryKeyArn, so: Ref is the replica's key id and Fn::GetAtt Arn is a real key ARN of the form arn:<partition>:kms:<replica-region>:<account>:key/<key-id>; DescribeKey on the primary then lists MultiRegionConfiguration.ReplicaKeys containing exactly {"Arn": <replica key ARN>, "Region": <replica region>}, and DescribeKey on the replica reports MultiRegionConfiguration.MultiRegionKeyType REPLICA with PrimaryKey.Arn = the primary's ARN. When PrimaryKeyArn names an ordinary single-Region key, the ReplicaKey resource goes CREATE_FAILED and the stack ends ROLLBACK_COMPLETE, with the resource-status reason in the registry-handler form: Resource handler returned message: "<primary key ARN> is not a multi-region key. (Service: Kms, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest) — fixed text apart from the key ARN taken from the template and the per-call ids; the single-Region key is left untouched (MultiRegion false, no MultiRegionConfiguration, KeyState Enabled).
Emulator: the replica stack reaches CREATE_COMPLETE but nothing was created in KMS — Fn::GetAtt Arn hands back a synthesized identifier built from the stack and logical id, 'arn:aws:kms:<region>:<account>:replica-key/<stack-name>-ReplicaKey-<suffix>' (not a key/<key-id> ARN), and DescribeKey on the primary reports MultiRegionConfiguration.ReplicaKeys = []. With a single-Region PrimaryKeyArn the stack also reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE, so neither the KMS refusal nor its handler reason is produced.
Why: every multi-Region-key deployment goes green while no replica exists — a client then cannot decrypt the primary's ciphertext in the replica Region, cannot find the replica by Ref/GetAtt, sees no PendingDeletion/PendingReplicaDeletion tombstones on delete, and gets a false pass for a template whose PrimaryKeyArn is invalid.
Fix: the CloudFormation AWS::KMS::ReplicaKey resource handler — provision through KMS ReplicateKey (and tombstone it on delete) instead of synthesizing a physical id, and surface the KMS refusal as the resource's CREATE_FAILED reason so the stack rolls back.
Traces: cloudformation__kms.deploy-kms-replicakey-cfn-provisions-replica-then-tombstones, cloudformation__kms.deploy-kms-replicakey-cfn-non-multiregion-primary-rejected.

## Evidence
Check deploy-kms-replicakey-cfn-provisions-replica-then-tombstones: expected the primary's MultiRegionConfiguration.ReplicaKeys == [{"Arn": <replica ARN>, "Region": <replica region>}]; actual "the primary lists replicas []; GetAtt Arn 'arn:aws:kms:us-west-2:000000000000:replica-key/sim2real-behav-cfnrepl-replica-<id>-ReplicaKey-<id>'" — the GetAtt value is not a KMS key ARN at all.

Check deploy-kms-replicakey-cfn-non-multiregion-primary-rejected: expected stack status ROLLBACK_COMPLETE with the ReplicaKey CREATE_FAILED reason 'Resource handler returned message: "<arn> is not a multi-region key. (Service: Kms, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'; actual "rejected stack ended 'CREATE_COMPLETE'".

## Steps to Reproduce
1. In region A create a multi-Region KMS key (MultiRegion=True) and an ordinary single-Region key.
2. In region B, CreateStack with a template whose only resource is AWS::KMS::ReplicaKey with PrimaryKeyArn = the multi-Region key's ARN, plus Outputs for Ref and Fn::GetAtt Arn. Wait for CREATE_COMPLETE, then DescribeKey the primary in region A: MultiRegionConfiguration.ReplicaKeys must list the replica by ARN and Region, and DescribeKey on the GetAtt Arn in region B must report MultiRegionKeyType REPLICA with PrimaryKey.Arn = the primary.
3. CreateStack the same template with PrimaryKeyArn = the single-Region key's ARN: the stack must end ROLLBACK_COMPLETE with the ReplicaKey resource CREATE_FAILED and the handler reason above, and the single-Region key must be unchanged.
4. DeleteStack the replica stack then the primary stack: the replica must read PendingDeletion and the primary PendingReplicaDeletion.
