Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kms__s3.cdk-bootstrap-customer-key-encrypts-the-staging-bucket-and-published-assets cloudformation__kms__s3.cdk-bootstrap-customer-key-disabled-blocks-asset-publish-and-the-deploy
Issue-Key: cloudformation__kms__s3.cdk-bootstrap-customer-key-encrypts-the-staging-bucket-and-published-assets|run#18|AssertionError|assert default["SSEAlgorithm"] == "aws:kms", (
Issue-Key: cloudformation__kms__s3.cdk-bootstrap-customer-key-disabled-blocks-asset-publish-and-the-deploy|run#15|KeyError|key_id = s3.get_bucket_encryption(Bucket=staging_bucket)["ServerSideEncryptionConfiguration"][
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# cloudformation__kms__s3: a bucket created by a stack whose BucketEncryption names a KMS key gets AES256 default encryption instead of aws:kms, so `cdk bootstrap --bootstrap-customer-key` leaves the staging bucket unencrypted by the CMK

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation AWS::S3::Bucket handler does not apply the template's BucketEncryption property when it selects SSE-KMS — the bucket is created with the SSE-S3 (AES256) default instead, and the rule names no KMS key.
AWS: recorded behavior only for the CFN handler; the S3 model is the authority for the readback. A stack whose AWS::S3::Bucket declares BucketEncryption.ServerSideEncryptionConfiguration with ApplyServerSideEncryptionByDefault {SSEAlgorithm: aws:kms, KMSMasterKeyID: <key>} produces a bucket whose GetBucketEncryption returns exactly one rule with SSEAlgorithm 'aws:kms' and KMSMasterKeyID set to that key (resolvable by KMS DescribeKey, KeyManager CUSTOMER for a customer managed key), and every object later written there is reported by HeadObject as ServerSideEncryption 'aws:kms' with that SSEKMSKeyId. `cdk bootstrap --bootstrap-customer-key` is exactly that template: it creates a customer managed CMK and points the staging bucket cdk-<qualifier>-assets-<account>-<region> at it.
Emulator: `cdk bootstrap --bootstrap-customer-key` exits 0 and the staging bucket exists, but GetBucketEncryption reports the rule's ApplyServerSideEncryptionByDefault.SSEAlgorithm as 'AES256' and the rule carries no KMSMasterKeyID at all (reading that member raises KeyError), so the CMK the bootstrap stack created is attached to nothing and no published asset is KMS-encrypted.
Why: a client that bootstraps with a customer-managed key — the documented way to keep CDK assets under a key it controls — believes its templates and Lambda asset zips are encrypted with that CMK; they are stored under the SSE-S3 default instead, and any check or policy that reads the bucket's KMS key id fails or finds nothing. The disabled-key leg of the same contract (a disabled bootstrap CMK must block asset publish and the deploy) is unreachable for the same reason.
Fix: the AWS::S3::Bucket resource handler's translation of BucketEncryption into the bucket's default-encryption state (honour SSEAlgorithm aws:kms and KMSMasterKeyID, including when they arrive through a template Condition/Fn::If as the CDK bootstrap template uses).
Traces: cloudformation__kms__s3.cdk-bootstrap-customer-key-encrypts-the-staging-bucket-and-published-assets, cloudformation__kms__s3.cdk-bootstrap-customer-key-disabled-blocks-asset-publish-and-the-deploy.

## Evidence
cdk-bootstrap-customer-key-encrypts-the-staging-bucket-and-published-assets: "staging bucket 'cdk-<qualifier>-assets-<account>-<region>' default SSE algorithm is 'AES256', expected 'aws:kms' under --bootstrap-customer-key".

cdk-bootstrap-customer-key-disabled-blocks-asset-publish-and-the-deploy: reading ServerSideEncryptionConfiguration.Rules[0].ApplyServerSideEncryptionByDefault['KMSMasterKeyID'] from GetBucketEncryption on the same staging bucket raised KeyError: 'KMSMasterKeyID' — the rule names no key.

## Steps to Reproduce
1. Run `cdk bootstrap aws://<account>/<region> --qualifier <q> --toolkit-stack-name <stack> --bootstrap-customer-key` (exits 0).
2. s3.get_bucket_encryption(Bucket='cdk-<q>-assets-<account>-<region>'): expect exactly one rule with ApplyServerSideEncryptionByDefault.SSEAlgorithm 'aws:kms' and a KMSMasterKeyID that kms.describe_key resolves with KeyManager 'CUSTOMER'. The emulator returns SSEAlgorithm 'AES256' and no KMSMasterKeyID.
3. (Equivalently, CreateStack any template with an AWS::S3::Bucket whose BucketEncryption selects SSEAlgorithm aws:kms with a KMSMasterKeyID, then GetBucketEncryption on the created bucket.)
4. `cdk deploy` an asset-bearing app with that qualifier and HeadObject the published objects: ServerSideEncryption must be 'aws:kms' with SSEKMSKeyId equal to the bootstrap CMK's ARN.
