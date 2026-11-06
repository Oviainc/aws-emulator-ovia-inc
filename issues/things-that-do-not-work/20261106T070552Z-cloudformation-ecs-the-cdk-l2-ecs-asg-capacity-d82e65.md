Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api
Issue-Key: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api|_cdk#18|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Commit: 0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9

# cloudformation__ecs: the CDK L2 ECS/ASG capacity-provider deploy fails at asset publication — the bootstrap file-publishing role is denied s3:ListBucket on the CDK assets bucket

> _Note: this issue was identified against the code base at commit `0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Cause 1 of 1: the role `cdk bootstrap` creates for publishing assets cannot read its own assets bucket, so `cdk deploy` aborts before CloudFormation is called and the ECS capacity-provider contract is never exercised.

AWS: recorded behavior only — IAM authorization is not described by the Smithy model. `cdk bootstrap` creates the CDKToolkit stack whose `cdk-<qualifier>-file-publishing-role-<account>-<region>` role is granted the asset bucket actions (s3:GetObject*/GetBucket*/List*/Abort*/PutObject*) on `arn:<partition>:s3:::cdk-<qualifier>-assets-<account>-<region>` and `.../*`. The CLI assumes that role, its ListBucket preflight succeeds, the assets publish and the deploy exits 0, after which the ASG capacity provider is visible through the ECS API.
Emulator: the assumed role is refused — `User: arn:aws:sts::<account>:assumed-role/cdk-<qualifier>-file-publishing-role-<account>-<region>/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-<qualifier>-assets-<account>-<region>" because no identity-based policy allows the s3:ListBucket action`; the CLI prints `Failed to publish asset <stack> Template (...)`, raises `AggregateError:` and exits 1. No stack, cluster or capacity provider is created.
Why: a client following AWS's documented bootstrap-then-deploy flow cannot deploy an ECS cluster with managed scaling at all.
Fix: the CDKToolkit bootstrap template's file-publishing-role policy as the emulator provisions it, and/or IAM's evaluation of s3:ListBucket on a bucket ARN for an assumed role. Same root cause as the cloudformation, cloudformation__ec2 and cloudformation__iam__lambda__s3 issues filed in this sweep. Note the open issue 20261028T235427Z-cloudformation-ecs-the-cdk-l2-ecs-asg-capacity.md describes this scenario producing no verdict at all; it does not cover this denial.
Traces: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api.

## Evidence
`cdk deploy` rc=1, expected 0. Output tail: `sim2real-cdkecs-<id>: fail: User: arn:aws:sts::000000000000:assumed-role/cdk-q758d12dba-file-publishing-role-000000000000-us-west-2/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-q758d12dba-assets-000000000000-us-west-2" because no identity-based policy allows the s3:ListBucket action` / `Failed to publish asset sim2real-cdkecs-<id> Template (000000000000-us-west-2-d960db5c)` / `AggregateError:`. (Qualifier and asset hash differ per run.)

## Steps to Reproduce
`cdk bootstrap --qualifier <q>` against the emulator, then `cdk deploy --require-approval never` for an aws-cdk-lib app building an `aws_ecs.Cluster` with an `AsgCapacityProvider` with managed scaling. The deploy exits 1 at the publish step with the denial above.
Replay: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api.
