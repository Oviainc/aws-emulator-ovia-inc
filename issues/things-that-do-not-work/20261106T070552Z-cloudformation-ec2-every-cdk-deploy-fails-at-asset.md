Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count cloudformation__ec2.cdk-l2-vpc-max-azs-two-deploys-two-nat-gateways cloudformation__ec2.cdk-l2-vpc-nat-gateways-one-overrides-the-per-az-default cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group
Issue-Key: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created|_cdk#17|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift|_cdk#18|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count|_cdk#17|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-l2-vpc-max-azs-two-deploys-two-nat-gateways|_cdk#17|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-l2-vpc-nat-gateways-one-overrides-the-per-az-default|_cdk#18|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route|_cdk#18|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group|_cdk#20|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Commit: 0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9

# cloudformation__ec2: every `cdk deploy` fails at asset publication — the bootstrap file-publishing role is denied s3:ListBucket on the CDK assets bucket

> _Note: this issue was identified against the code base at commit `0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Cause 1 of 1: the role `cdk bootstrap` creates for publishing assets cannot read its own assets bucket, so `cdk deploy` aborts before CloudFormation is called and none of the seven EC2/VPC CDK contracts can be observed.

AWS: recorded behavior only — IAM authorization is not described by the Smithy model. `cdk bootstrap` creates the CDKToolkit stack; its `cdk-<qualifier>-file-publishing-role-<account>-<region>` role is attached a policy granting the asset bucket actions (s3:GetObject*/GetBucket*/List*/Abort*/PutObject*) on `arn:<partition>:s3:::cdk-<qualifier>-assets-<account>-<region>` and `.../*`. The CLI assumes that role, its ListBucket/ListObjectsV2 preflight on the assets bucket is authorized, the template and code assets publish, and the deploy proceeds to CreateChangeSet/ExecuteChangeSet and exits 0.
Emulator: the assumed role is refused — `User: arn:aws:sts::<account>:assumed-role/cdk-<qualifier>-file-publishing-role-<account>-<region>/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-<qualifier>-assets-<account>-<region>" because no identity-based policy allows the s3:ListBucket action`. The CLI reports `Failed to publish asset <stack> Template (<account>-<region>-<hash>)`, raises `AggregateError:` and exits 1. No stack is created.
Why: a client that bootstraps and deploys exactly as AWS documents cannot deploy anything through CDK: VPC max-azs clamping, per-AZ vs single NAT gateway, isolated subnets with no egress route, VPC-from-lookup subnet classification, `cdk destroy` cleanup and `cdk diff` vs drift are all unreachable.
Fix: the CDKToolkit bootstrap template's file-publishing-role policy as the emulator provisions it (its asset-bucket resource statements), and/or the IAM authorization evaluation of s3:ListBucket against a bucket ARN for an assumed role. Same root cause as the cloudformation, cloudformation__ecs and cloudformation__iam__lambda__s3 issues filed in this sweep; fixing the bootstrap role policy once should clear all of them.
Traces: the seven checks listed in trace_ids, each of which fails with the identical denial and a non-zero `cdk deploy`.

## Evidence
All seven checks fail the same way. Representative output tail of `cdk deploy`:
`sim2real-cdkdes-<id>: fail: User: arn:aws:sts::000000000000:assumed-role/cdk-q89ad90692-file-publishing-role-000000000000-us-west-2/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-q89ad90692-assets-000000000000-us-west-2" because no identity-based policy allows the s3:ListBucket action`
`Failed to publish asset sim2real-cdkdes-<id> Template (000000000000-us-west-2-fcaa1e14)`
`AggregateError:`
Expected: rc 0 and a CREATE_COMPLETE stack whose VPC resources can then be read back. Actual: rc 1, nothing created. (Qualifier, stack and asset-hash values differ per run.)

## Steps to Reproduce
`cdk bootstrap --qualifier <q>` against the emulator, then `cdk deploy --require-approval never` for any aws-cdk-lib `aws_ec2.Vpc` app. The deploy exits 1 at the publish step with the s3:ListBucket denial above.
Replay: the seven check ids in trace_ids.
