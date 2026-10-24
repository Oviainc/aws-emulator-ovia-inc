Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive apigateway__cloudfront__s3.deploy-cloudfront-apigateway-origin-stack-negative apigateway__cloudfront__s3.deploy-cloudfront-s3-oac-stack-positive apigateway__cloudfront__s3.cli-apigw-cloudfront-custom-domain-missing-cert-negative apigateway__cloudfront__s3.s3-role-trust-policy-validation
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudfront__s3.deploy-cloudfront-apigateway-origin-stack-negative|_deploy_cfn#4|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudfront__s3.deploy-cloudfront-s3-oac-stack-positive|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudfront__s3.cli-apigw-cloudfront-custom-domain-missing-cert-negative|run#15|AssertionError|assert proc.returncode != 0, (
Issue-Key: apigateway__cloudfront__s3|PutBucketReplication|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# apigateway/cloudfront/s3 deploy paths: stack Outputs carry no OutputValue, CreateDistribution accepts an alias with the default certificate, and S3 PutBucketReplication is a stub (3 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Three independent defects surfaced by the apigateway/cloudfront/s3 cross-service behavioral checks. Each is its own root cause and its own work item; this issue bundles THREE causes and all three must be closed before submitting.

Cause 1 of 3: every CloudFormation-deployed scenario in this group fails because the stack's Outputs entries carry no OutputValue.
AWS: recorded behavior only — DescribeStacks on a CREATE_COMPLETE stack whose template declares Outputs returns each output as {OutputKey, OutputValue, ...}; these checks build {o['OutputKey']: o['OutputValue'] for o in Stacks[0]['Outputs']} straight after the wait.
Emulator: the Outputs list is present but each entry has no OutputValue key, so the check dies with KeyError: 'OutputValue' before it can address the deployed bucket, REST API or distribution.
Why: a client cannot consume any stack output, so the whole deploy-then-use workflow (deploy a CloudFront+S3/API Gateway estate by template, then drive it) is unreachable, and the failure looks like a client bug.
Fix: the CloudFormation DescribeStacks output serialization plus the output-resolution step that evaluates Ref/Fn::GetAtt for the stack's resources. Related open issues on the same serializer that do NOT carry these replay keys: 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md.
Traces: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive, apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative, apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive, apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive, apigateway__cloudfront__s3.deploy-cloudfront-apigateway-origin-stack-negative, apigateway__cloudfront__s3.deploy-cloudfront-s3-oac-stack-positive.

Cause 2 of 3: CreateDistribution accepts an alternate domain name (Aliases) on a distribution whose ViewerCertificate is only the default CloudFront certificate.
AWS: Smithy: CreateDistribution models InvalidViewerCertificate(400) and InvalidArgument(400); an Aliases entry with ViewerCertificate={CloudFrontDefaultCertificate:true} is refused before any distribution is created (the recording shows InvalidViewerCertificate, or InvalidArgument when the CNAME/ownership pre-check fires first), so no distribution carries the alias afterwards.
Emulator: the create succeeds (the CLI exits 0) and a distribution carrying the alias exists.
Why: a client gets a distribution AWS would never create — an alias served by a certificate that does not cover it — so a custom-domain misconfiguration is reported as deployed and fails only at request time in reality.
Fix: ViewerCertificate/Aliases validation in the CreateDistribution handler of the cloudfront crate.
Traces: apigateway__cloudfront__s3.cli-apigw-cloudfront-custom-domain-missing-cert-negative.

Cause 3 of 3: S3 PutBucketReplication is an unimplemented scaffold stub.
AWS: PutBucketReplication is a modeled S3 operation (PUT /{Bucket}?replication) returning 200 for a valid ReplicationConfiguration with a role ARN and at least one rule; NotImplemented is not one of its modeled errors.
Emulator: `NotImplemented` — "operation PutBucketReplication is scaffolded but not implemented in this emulator" — so the check cannot establish the replication configuration whose IAM role trust it then validates.
Why: no replication scenario can be set up, and the unmodeled error code is classified by SDK retry logic as a server fault rather than the client-side 'unsupported' it is.
Fix: the PutBucketReplication handler in the s3 service crate (services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_replication.rs) and the bucket replication state it writes.
Traces: apigateway__cloudfront__s3.s3-role-trust-policy-validation.

## Evidence
Cause 1: "KeyError: 'OutputValue'" raised at the outputs comprehension in each of the six checks, immediately after the stack reported CREATE_COMPLETE.
Cause 2: "AssertionError: CLI create-distribution with alias + default cert unexpectedly succeeded" — the check then expects the rejection code to be InvalidViewerCertificate or InvalidArgument and asserts no distribution carries the alias.
Cause 3: "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutBucketReplication operation: operation PutBucketReplication is scaffolded but not implemented in this emulator".

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring an S3 bucket + CloudFront distribution (or API Gateway REST API) and Outputs exposing their ids; wait for CREATE_COMPLETE; read Stacks[0].Outputs[*].OutputValue -> observe the member is absent.
2. Cause 2: `aws cloudfront create-distribution` with DistributionConfig {Aliases:{Quantity:1,Items:['custom.example.com']}, ViewerCertificate:{CloudFrontDefaultCertificate:true}, ...} -> expect a non-zero exit with InvalidViewerCertificate/InvalidArgument and no distribution; observe success.
3. Cause 3: PutBucketReplication(Bucket=<bucket with versioning>, ReplicationConfiguration={'Role': <role arn>, 'Rules': [...]}) -> expect 200; observe NotImplemented.
