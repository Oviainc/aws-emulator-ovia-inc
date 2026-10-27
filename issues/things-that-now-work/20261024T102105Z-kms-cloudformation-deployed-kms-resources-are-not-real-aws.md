Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.deploy-kms-key-cfn-then-encrypt-decrypt kms.deploy-kms-key-cfn-usable kms.deploy-kms-key-policy-from-template-applied kms.deploy-kms-key-rotation-policy-enforced kms.deploy-kms-key-with-policy-cfn-enforced kms.deploy-kms-alias-cfn-resolves-target kms.deploy-kms-alias-cfn-then-resolve kms.deploy-kms-alias-cfn-invalid-target-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|DescribeKey|NotFoundException
Issue-Key: kms|GetKeyPolicy|NotFoundException
Issue-Key: kms.deploy-kms-alias-cfn-invalid-target-rejected|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected rollback for invalid target key, got {status}"
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# CloudFormation-deployed KMS resources are not real: AWS::KMS::Key and AWS::KMS::Alias never exist in KMS after CREATE_COMPLETE, and an alias with an invalid target does not roll back (3 causes)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKey

## Description
Three root causes, all in the CloudFormation-to-KMS provisioning path. Every one of these stacks reaches CREATE_COMPLETE, so the divergence is invisible until the deployed resource is used.

Cause 1 of 3 — AWS::KMS::Key reaches CREATE_COMPLETE but no key is created in KMS.
AWS: an AWS::KMS::Key resource creates a real CMK; the resource's physical id is the key id, Ref yields the key id and Fn::GetAtt Arn/KeyId yield the key's ARN/id, and DescribeKey/GetKeyPolicy/Encrypt/Decrypt on it all work immediately after CREATE_COMPLETE. Smithy (kms): DescribeKey and GetKeyPolicy answer NotFoundException (httpError 404) only for a key that does not exist.
Emulator: the physical id handed back is a synthetic ARN built from the stack and logical resource names — e.g. 'arn:aws:kms:us-west-2:<acct>:key/sim2real-behav-kmskey-c1a1bd09f6c8-Key-7A2D4ED604C8' — and KMS does not know it: DescribeKey answers NotFoundException "Key '<that arn>' does not exist" and GetKeyPolicy the same. Note the id is not even a KMS key-id shape (not a UUID).
Why: any template that provisions a CMK and then uses it (encrypting a bucket, a queue, a secret, an RDS instance) deploys green and then fails on first use; stack Outputs carrying the key ARN are unusable, and policy/rotation properties of the template can never be read back.
Fix: the CloudFormation resource provider for AWS::KMS::Key — it must call the KMS state/CreateKey path and record the real key id as the physical resource id (including Policy and EnableKeyRotation properties).
Traces: kms.deploy-kms-key-cfn-then-encrypt-decrypt, kms.deploy-kms-key-cfn-usable, kms.deploy-kms-key-policy-from-template-applied, kms.deploy-kms-key-rotation-policy-enforced, kms.deploy-kms-key-with-policy-cfn-enforced.

Cause 2 of 3 — AWS::KMS::Alias reaches CREATE_COMPLETE but no alias is created in KMS.
AWS: an AWS::KMS::Alias resource creates the alias; DescribeKey(KeyId='alias/<name>' or the alias ARN) resolves to the target key's metadata.
Emulator: DescribeKey on the deployed alias ARN answers NotFoundException 'Alias arn:aws:kms:us-west-2:<acct>:alias/<name> is not found.'
Why: the common 'deploy a key plus a friendly alias, then address the key by alias' pattern cannot be deployed; nothing can resolve the alias afterwards.
Fix: the CloudFormation resource provider for AWS::KMS::Alias — create the alias through KMS's CreateAlias path and bind it to the resolved TargetKeyId.
Traces: kms.deploy-kms-alias-cfn-resolves-target, kms.deploy-kms-alias-cfn-then-resolve.

Cause 3 of 3 — a stack whose AWS::KMS::Alias names an invalid target key reaches CREATE_COMPLETE instead of failing and rolling back.
AWS: CreateAlias against a nonexistent TargetKeyId is refused NotFoundException (Smithy: CreateAlias errors include NotFoundException httpError 404), so the resource create fails and the stack goes CREATE_FAILED -> ROLLBACK_COMPLETE.
Emulator: the stack reports CREATE_COMPLETE.
Why: templates with dangling KMS references are reported as successful deployments, so a client's rollback/err-handling path is never exercised and broken infrastructure looks healthy.
Fix: the AWS::KMS::Alias provider must surface the KMS refusal as a resource failure so the stack's rollback machinery runs.
Traces: kms.deploy-kms-alias-cfn-invalid-target-rejected.

## Evidence
Cause 1: kms.deploy-kms-key-cfn-then-encrypt-decrypt — "botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeKey operation: Key 'arn:aws:kms:us-west-2:000000000000:key/sim2real-behav-kmskey-c1a1bd09f6c8-Key-7A2D4ED604C8' does not exist"; kms.deploy-kms-key-cfn-usable — same for '.../key/sim2real-kms-cfn-51df3f006e22-TestKey-F52F2A7C6FB9'; kms.deploy-kms-key-policy-from-template-applied — same on GetKeyPolicy for '.../key/sim2real-kms-pol-ec484c94405a-PolicyKey-9AB96183B062'; kms.deploy-kms-key-rotation-policy-enforced — same on DescribeKey for '.../key/sim2real-kms-rot-bc0b4f36211e-RotatingKey-B6600CDF500B'; kms.deploy-kms-key-with-policy-cfn-enforced — same on GetKeyPolicy for '.../key/sim2real-kms-cfn-ce3b7813add7-TestKey-67EB248A94D2'. Expected: HTTP 200 with the key's KeyMetadata / policy document.
Cause 2: kms.deploy-kms-alias-cfn-resolves-target — 'An error occurred (NotFoundException) when calling the DescribeKey operation: Alias arn:aws:kms:us-west-2:000000000000:alias/sim2real-cfn-b2c552a0bccf is not found.'; kms.deploy-kms-alias-cfn-then-resolve — same for 'alias/sim2real-behav-d5cd5b0a43ad'. Expected: 200 with the target key's KeyMetadata.
Cause 3: kms.deploy-kms-alias-cfn-invalid-target-rejected — 'AssertionError: expected rollback for invalid target key, got CREATE_COMPLETE'. Expected terminal stack status: ROLLBACK_COMPLETE.

## Steps to Reproduce
1. CreateStack with a template containing an AWS::KMS::Key (with Policy and EnableKeyRotation set); wait for CREATE_COMPLETE; take the Ref/GetAtt value and call kms:DescribeKey and kms:GetKeyPolicy on it — observe NotFoundException.
2. CreateStack with an AWS::KMS::Key plus an AWS::KMS::Alias targeting it; after CREATE_COMPLETE call kms:DescribeKey with the alias ARN — observe 'Alias ... is not found.'
3. CreateStack with an AWS::KMS::Alias whose TargetKeyId does not exist — observe CREATE_COMPLETE instead of CREATE_FAILED/ROLLBACK_COMPLETE.
