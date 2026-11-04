Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role
Issue-Key: cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role|_wait_for_describe_vpcs#41|Timeout|time.sleep(interval_s)
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudformation__ec2__iam: the CDK VPC context-provider lookup-cache scenario never settles, so a denied lookup role produces no verdict

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAvailabilityZones

## Description
ONE root cause.

Cause 1 of 1 — the CDK context-provider lookup scenario never reaches an observable outcome.
AWS: recorded behavior only. The check bootstraps a CDK environment, builds a VPC, then runs several `cdk synth` passes: a first synth populates cdk.context.json from the EC2 VPC-lookup context provider (DescribeVpcs/DescribeSubnets/DescribeRouteTables/DescribeAvailabilityZones through the assumed lookup role), and a later synth must still succeed from that cached context after the lookup role's permission is removed. Against AWS every one of those calls answers and each synth terminates.
Emulator: the check is killed by the harness with "Error: Check timed out after 60.0s" having reached no assertion — the bootstrap/VPC-build/synth sequence never produces a terminal result, so neither the cache-hit behaviour nor the denied-role behaviour is observable.
Why: a client cannot use CDK environment-aware constructs at all against this emulator: `cdk synth` of a stack with a Vpc.from_lookup never completes, and the context cache that is meant to insulate a later synth from a revoked lookup role cannot be exercised.
Fix: the EC2 context-provider read path the CDK lookup uses (DescribeVpcs/DescribeSubnets/DescribeRouteTables/DescribeAvailabilityZones for a deployed VPC) together with the CloudFormation bootstrap/deploy path those synths depend on — the sequence must terminate with an answer instead of hanging. Note this scenario also reads subnet route-table associations, which are absent (see the cloudformation__ec2 route-table issue filed in this same sweep).
Traces: cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role.

## Evidence
Expected: the check completes — first `cdk synth` writes cdk.context.json from the EC2 VPC-lookup context provider, the lookup role's iam permission is then removed, and a later `cdk synth` still succeeds from the cached context.
Actual: "Error: Check timed out after 60.0s" with no assertion reached; the scenario produced no verdict about the denied lookup role at all.

## Steps to Reproduce
1. cdk bootstrap a qualifier-scoped toolkit against the emulator.
2. Deploy a VPC, then `cdk synth` a stack using ec2.Vpc.from_lookup so the EC2 VPC-lookup context provider runs under the assumed lookup role and cdk.context.json is written.
3. Remove the lookup role's permission and `cdk synth` again, expecting success from the cached context.
4. Observe that the sequence never terminates and the check is killed after 60s.
