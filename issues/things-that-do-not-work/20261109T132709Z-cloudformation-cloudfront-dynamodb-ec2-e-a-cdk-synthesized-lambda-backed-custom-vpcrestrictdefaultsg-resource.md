Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__dynamodb__ec2__eks__kinesis__lambda__redshift-data__s3.cdk-compute-data-estate-serves-and-destroys
Issue-Key: cloudformation__cloudfront__dynamodb__ec2__eks__kinesis__lambda__redshift-data__s3.cdk-compute-data-estate-serves-and-destroys|_run_cli#14|AssertionError|assert result.returncode == 0, (
Issue-Commit: ab58585e96332c13ed2b8d30023e29e5d03b1c39

# A CDK-synthesized Lambda-backed Custom::VpcRestrictDefaultSG resource never settles: it stays CREATE_IN_PROGRESS until CloudFormation's wait expires, so `cdk deploy --all` of the 25-stack estate fails

> _Note: this issue was identified against the code base at commit `ab58585e96332c13ed2b8d30023e29e5d03b1c39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a Lambda-backed CloudFormation custom resource created inside a stack never reaches CREATE_COMPLETE — CloudFormation leaves it CREATE_IN_PROGRESS until the resource-create wait runs out and the stack create fails.

AWS: recorded behavior only (custom resources are not part of any Smithy shape; CloudFormation's modeled surface here is CreateStack/DescribeStacks/DescribeStackEvents). When a stack declares a resource whose Type is a Custom:: type backed by a Lambda ServiceToken, CloudFormation invokes that function with the Create request and the resource settles when the function PUTs its SUCCESS response to the pre-signed response URL; the resource then reaches CREATE_COMPLETE and the stack CREATE_COMPLETE. CDK emits exactly this for every AWS::EC2::VPC it synthesizes with the default restrictDefaultSecurityGroup behavior (logical id EC2VPCRestrictDefaultSecurityGroupCustomResource…, Type Custom::VpcRestrictDefaultSG), so a plain `cdk deploy` of a VPC depends on it.

Emulator: the custom resource is created but never settles. `cdk deploy --all` exits 1 with 'DeploymentError: Resource updates failed: <stack>/EC2VPC/RestrictDefaultSecurityGroupCustomResource/Default (Custom::VpcRestrictDefaultSG …) was still CREATE_IN_PROGRESS when the wait for it to be created ran out', and the CDK CLI also reports no log group content for the provider function it points at (/aws/lambda/<stack>-CustomVpcRestrictDefaultS-…). The stack names, logical-id suffixes and the region in that output are this run's values; the defect is that the Create lifecycle callback for a Lambda-backed Custom:: resource is never completed.

Why: no CDK application that synthesizes a VPC (which is most of them, including this 25-stack estate) can be deployed against the emulator — the first stack containing a VPC hangs its custom resource and fails the whole `cdk deploy --all`, so every downstream behavior of the estate (its tables, function, stream, warehouse, buckets, clusters, distributions and their refusals) is unreachable and the destroy path is never exercised either.

Fix: the CloudFormation provisioning engine's custom-resource path — invoke the resource's ServiceToken Lambda for Create/Update/Delete and settle the resource from the provider's SUCCESS/FAILED response (and surface a provider failure as CREATE_FAILED with its Reason) rather than leaving it CREATE_IN_PROGRESS until the wait expires. Related open issues describe the same mechanism from other entry points and do not carry this check: 20261108T043811Z-cloudformation-s3-a-lambda-backed-cloudformation-custom-resource-never-completes.md (Custom::CDKBucketDeployment), 20261109T085337Z-cloudformation-iam-lambda-s3-a-custom-resource-backed.md (out-of-band Lambda provider), 20261108T065147Z-cloudformation-lambda-sqs-a-success-answering-lambda-backed.md (SUCCESS-answering AWS::CloudFormation::CustomResource).

Note on attribution: the suite lists this check under the composite heading 'cloudformation__cloudfront__dynamodb__ec2__eks__kinesis__lambda__redshift-data__s3', which is not a single service token; the divergence itself is in CloudFormation's custom-resource provisioning, and the service field carries the heading verbatim.

Traces: cloudformation__cloudfront__dynamodb__ec2__eks__kinesis__lambda__redshift-data__s3.cdk-compute-data-estate-serves-and-destroys.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

Check cloudformation__cloudfront__dynamodb__ec2__eks__kinesis__lambda__redshift-data__s3.cdk-compute-data-estate-serves-and-destroys fails in its deploy step: 'AssertionError: `cdk deploy --all` exited 1, expected 0; output tail: … \u274c sim2real-cad-<id>-EbsVolume failed to deploy / \u2023 DeploymentError: Resource updates failed: sim2real-cad-<id>-EbsVolume/EC2VPC/RestrictDefaultSecurityGroupCustomResource/Default (Custom::VpcRestrictDefaultSG EC2VPCRestrictDefaultSecurityGroupCustomResourceF7FE4A4E) / Custom::VpcRestrictDefaultSG / sim2real-cad-<id>-EbsVolume-EC2VPCRestrictDefaultSecurityGroupCustomResourceF7FE4A4E-… was still CREATE_IN_PROGRESS when the wait for it to be created ran out … Logs from /aws/lambda/sim2real-cad-<id>-CustomVpcRestrictDefaultS-…'.

Expected: `cdk deploy --all` exits 0 — the Custom::VpcRestrictDefaultSG resource reaches CREATE_COMPLETE once its provider Lambda answers, and the EbsVolume stack (and the remaining 24 stacks) reach CREATE_COMPLETE.
Actual: the custom resource remains CREATE_IN_PROGRESS for the whole resource-create wait, the stack create fails, and the CLI exits 1.

Stack-name prefixes, the generated logical-id/physical-id suffixes, the account-specific log-group name and the region in the output are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Synthesize any CDK app containing an AWS::EC2::VPC with CDK's default restrictDefaultSecurityGroup behavior (the suite's cdk_compute_data_estate app does this in its EbsVolume stack) and run `cdk bootstrap` then `cdk deploy --all --ci --no-version-reporting --no-notices` against the emulator.
3. Watch the stack's events: the Custom::VpcRestrictDefaultSG resource (logical id EC2VPCRestrictDefaultSecurityGroupCustomResource…) enters CREATE_IN_PROGRESS and never leaves it; CloudFormation's wait for the resource expires and the stack create fails, so `cdk deploy --all` exits 1.
4. Equivalently, CreateStack a minimal template with a Custom:: resource whose ServiceToken is a Lambda that PUTs SUCCESS to the response URL, and poll DescribeStackResources: the resource stays CREATE_IN_PROGRESS.
5. Re-replay the check in trace_ids to confirm a fix.
