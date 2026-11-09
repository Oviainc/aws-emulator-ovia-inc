Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__ecr__iam__s3__ssm.cdk-bootstrap-provisions-toolkit-and-serves-default-synth-deploy
Issue-Key: cloudformation__dynamodb__ecr__iam__s3__ssm.cdk-bootstrap-provisions-toolkit-and-serves-default-synth-deploy|run#41|AssertionError|assert defaults == [{"SSEAlgorithm": "aws:kms"}], (
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__dynamodb__ecr__iam__s3__ssm: the CDK bootstrap staging bucket is created with SSE-S3 (AES256) defaults instead of the aws:kms default the toolkit stack declares

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause.

Cause 1 of 1: the bucket provisioned for the CDK toolkit stack does not carry the default-encryption configuration the template asks for.

AWS: `cdk bootstrap` deploys CDKToolkit, whose staging bucket declares BucketEncryption with ServerSideEncryptionByDefault.SSEAlgorithm 'aws:kms' and no KMSMasterKeyID (the AWS managed aws/s3 key) — so GetBucketEncryption on the staging bucket returns exactly one rule whose ApplyServerSideEncryptionByDefault is {'SSEAlgorithm': 'aws:kms'}. Smithy: s3 GetBucketEncryption returns ServerSideEncryptionConfiguration.Rules[].ApplyServerSideEncryptionByDefault{SSEAlgorithm, KMSMasterKeyID}; CloudFormation's contract is that AWS::S3::Bucket BucketEncryption is applied as declared.

Emulator: the staging bucket reports [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}] — the emulator's own bucket default, not the template's rule. Two divergences in one value: SSEAlgorithm is 'AES256' where the template declares 'aws:kms', and extra members (BucketKeyEnabled, BlockedEncryptionTypes) appear that the declared rule does not set.

Why: everything the toolkit stages (templates, Lambda asset zips, Docker-less file assets) is then encrypted differently from what the account's bootstrap declares, so any check or policy that requires KMS-encrypted staging assets — and any later `cdk deploy` that reads the bootstrap outputs to decide key access — draws the wrong conclusion.

Fix: the CloudFormation AWS::S3::Bucket provisioner must apply the template's BucketEncryption to the created bucket's encryption state (rather than leaving the bucket at the emulator's default), and GetBucketEncryption must echo only the members the configuration sets.

Traces: cloudformation__dynamodb__ecr__iam__s3__ssm.cdk-bootstrap-provisions-toolkit-and-serves-default-synth-deploy.

## Evidence
Expected: GetBucketEncryption(staging bucket).ServerSideEncryptionConfiguration.Rules == [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}}] (one aws:kms default, no KMSMasterKeyID).
Actual: [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}].
AssertionError: staging bucket encryption rules are [...], expected one aws:kms default with no KMSMasterKeyID.

## Steps to Reproduce
1. Run `cdk bootstrap` against the emulator (deploys the CDKToolkit stack).
2. Read the StagingBucket from the toolkit stack's resources/outputs.
3. s3.get_bucket_encryption(Bucket=<staging bucket>) -> AES256 default instead of the declared aws:kms default.
