Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__s3.e2e-amplify-deploy-frontend-from-s3-lifecycle
Issue-Key: amplify__s3|GetBucketPolicy|NoSuchBucketPolicy
Issue-Commit: fec48c0e4a4ef614c94c9aeb73915e609cba5726

# amplify__s3: a CREATE_COMPLETE stack's AWS::S3::BucketPolicy is never applied — GetBucketPolicy answers NoSuchBucketPolicy on the deployed source bucket

> _Note: this issue was identified against the code base at commit `fec48c0e4a4ef614c94c9aeb73915e609cba5726`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack that declares an AWS::S3::BucketPolicy alongside its AWS::S3::Bucket reaches CREATE_COMPLETE, but the bucket carries no policy at all, so the whole S3-sourced Amplify deployment scenario cannot start.
  AWS: recorded behavior only (the template is the check's own fixture; the model that matters is CloudFormation's resource provisioning plus S3's GetBucketPolicy, whose NoSuchBucketPolicy — 404, message "The bucket policy does not exist" — is the correct answer ONLY for a bucket that has no policy). The stack declares two resources: `FrontendSourceBucket` (AWS::S3::Bucket, BucketName from a parameter, VersioningConfiguration Status=Enabled, SSE-S3 BucketEncryption, PublicAccessBlockConfiguration with BlockPublicAcls/BlockPublicPolicy/IgnorePublicAcls/RestrictPublicBuckets all true) and `FrontendSourceBucketPolicy` (AWS::S3::BucketPolicy, Bucket: {"Ref": "FrontendSourceBucket"}, PolicyDocument with exactly one Deny statement — Principal {"AWS": "*"}, Action "s3:*", Resource the bucket ARN and ARN/*, Condition Bool aws:SecureTransport false). After CREATE_COMPLETE, s3:GetBucketPolicy on the bucket returns that document, whose Statement list has exactly one entry with Effect 'Deny'. BlockPublicPolicy does not block it: the statement grants nothing.
  Emulator: the stack reaches CREATE_COMPLETE and the bucket's other properties took effect (GetBucketVersioning reports 'Enabled'), but s3.get_bucket_policy on the same bucket is refused with NoSuchBucketPolicy "The bucket policy does not exist" — the AWS::S3::BucketPolicy resource left no policy on the bucket, and the stack reported success anyway.
  Why: the stack's declared security posture is simply absent — a bucket a template says only accepts TLS requests accepts everything, and nothing in the stack's status says so. Downstream, every step of this scenario is built on reading and amending that policy (the solve path does GetBucketPolicy, appends an Allow for amplify.amazonaws.com, and PutBucketPolicy), so the Amplify BUCKET_PREFIX deployment from S3, its serving at the branch URL, and the restore/cleanup/destroy phases are all unreachable: the check dies in phase 1 of 7.
  Fix: the CloudFormation provisioner for AWS::S3::BucketPolicy — resolve the Bucket reference and the PolicyDocument (including Fn::GetAtt/Fn::Join inside Resource) and apply it through the same state S3's GetBucketPolicy/PutBucketPolicy read, and fail the resource (rather than completing the stack) when it cannot be applied.
  Related but NOT covering this: open issue 20261029T180026Z-cloudformation-dynamodb-s3-cloudformation-s3-template-fetch-authorization-deletionpolicy-retain-on.md covers CFN/S3 template-fetch authorization, DeletionPolicy=Retain on buckets, nested-stack outputs and a replication-role trust policy; open issue 20261029T174654Z-cloudwatch-dynamodb-ecr-events-iam-logs-s3-bucket-policy-does-not-round-trip-getbucketpolicy.md covers GetBucketPolicy REWRITING a policy that exists. Neither covers a CFN-declared bucket policy that is never created at all.
  Traces: amplify__s3.e2e-amplify-deploy-frontend-from-s3-lifecycle.

## Evidence
amplify__s3.e2e-amplify-deploy-frontend-from-s3-lifecycle, phase 1/7 'deploy': after cloudformation.create_stack with the fixture template (deployments/amplify-source-bucket/template.json) settles to CREATE_COMPLETE and DescribeStacks reports the SourceBucketName output, the check asserts s3.get_bucket_versioning(Bucket=<bucket>)['Status'] == 'Enabled' (this passes) and then json.loads(s3.get_bucket_policy(Bucket=<bucket>)['Policy'])['Statement'] with [s['Effect'] for s in statements] == ['Deny'].

Actual: _PhaseError: [phase 1/7 deploy] ClientError: An error occurred (NoSuchBucketPolicy) when calling the GetBucketPolicy operation: The bucket policy does not exist.

Expected: a policy document with exactly one statement whose Effect is 'Deny' (the template's aws:SecureTransport=false deny). The bucket name is a per-run ctx.unique_name value; the requirement is that the declared policy exists on whatever bucket the stack created. The remaining six phases (setup, pre_invoke, solve, post_invoke, cleanup, destroy) were never reached, so no verdict exists for the Amplify BUCKET_PREFIX deployment from S3 in this check.

## Steps to Reproduce
1. cloudformation.create_stack(StackName=<unique>, TemplateBody=<the check's amplify-source-bucket template>, Parameters=[{'ParameterKey': 'BucketName', 'ParameterValue': <unique bucket name>}]) — the template declares AWS::S3::Bucket 'FrontendSourceBucket' (versioning enabled, SSE-S3, all public access blocked) and AWS::S3::BucketPolicy 'FrontendSourceBucketPolicy' whose PolicyDocument is a single Deny of s3:* on the bucket ARN and ARN/* when Bool aws:SecureTransport is 'false'.
2. Poll describe_stacks until StackStatus == CREATE_COMPLETE; read the SourceBucketName output.
3. s3.get_bucket_versioning(Bucket=<bucket>) -> 'Enabled' (holds).
4. s3.get_bucket_policy(Bucket=<bucket>) -> expected: the declared document, Statement == one entry with Effect 'Deny'. Observed: ClientError NoSuchBucketPolicy 'The bucket policy does not exist'.
5. (Blocked downstream by step 4) the solve path appends an Allow statement for Principal {'Service': 'amplify.amazonaws.com'} and PutBucketPolicy, then amplify.start_deployment(sourceUrl='s3://<bucket>/src/', sourceUrlType='BUCKET_PREFIX') and serves the branch URL.
