Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys
Issue-Key: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys|_sles_image#11|AssertionError|assert images, "no Amazon-owned x86_64 suse-sles-16-0-* image is available in the region"
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# ec2 image catalog has no Amazon-owned x86_64 suse-sles-16-0-* AMI, so a CDK estate cannot resolve a SLES image

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Cause 1 of 1: DescribeImages serves no Amazon-owned SUSE SLES 16 image.

AWS: DescribeImages with Owners=[amazon], a name filter of the form suse-sles-16-0-* and Architecture x86_64 returns at least one public, Amazon-owned SLES 16 AMI in every commercial region; that is how a template or CDK app that pins a SLES image resolves its ImageId. Smithy: DescribeImages -> DescribeImagesResult.Images (list of Image), filtered by Filters/Owners/ExecutableUsers; no modeled error covers 'catalog is empty' - an unmatched filter simply returns an empty Images list. Recorded behavior only for the exact AMI set a region publishes.

Emulator: the same DescribeImages call returns an empty Images list, so the check's image lookup finds nothing and asserts 'no Amazon-owned x86_64 suse-sles-16-0-* image is available in the region'. The call itself answers 200; the catalog behind it simply does not contain the SLES family.

Why: a client that resolves a distribution image by owner+name filter (every CDK/CloudFormation app that pins a non-Amazon-Linux OS) gets an empty list and cannot launch. It has no way to distinguish 'this region has no such image' from 'the emulator does not model that image family', so the whole estate deploy aborts before any API misbehaviour is reachable.

Fix: the EC2 image catalog/seed data behind the DescribeImages handler (its static AMI table), not the filter logic - the filter is applied correctly to an incomplete catalog.

Traces: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys.

## Evidence
Expected: DescribeImages(Owners=[amazon], Filters name=suse-sles-16-0-*, architecture=x86_64) returns >=1 Image.
Actual: AssertionError: no Amazon-owned x86_64 suse-sles-16-0-* image is available in the region (check helper _sles_image, line 949).

## Steps to Reproduce
1. Call ec2 DescribeImages with Owners=['amazon'] and Filters [{Name:'name',Values:['suse-sles-16-0-*']},{Name:'architecture',Values:['x86_64']}].
2. Observe Images is empty, where real AWS returns at least one SLES 16 AMI.
