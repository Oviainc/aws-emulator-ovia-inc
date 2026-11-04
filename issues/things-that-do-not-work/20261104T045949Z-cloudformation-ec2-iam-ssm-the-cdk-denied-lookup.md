Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth
Issue-Key: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth|_synth#21|Timeout|return rc, out
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudformation__ec2__iam__ssm: the CDK denied-lookup-role synth scenario never settles, so the expected synth failure is never observed

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
ONE root cause.

Cause 1 of 1 — the CDK VPC context-lookup scenario never reaches an observable outcome, so the refusal it is about is never seen.
AWS: recorded behavior only. The check bootstraps a CDK environment, builds a VPC, and runs three `cdk synth` passes: one where the lookup role may read the VPC (the EC2 context provider answers DescribeVpcs/DescribeSubnets/DescribeRouteTables and synth succeeds) and one where the lookup role is denied, where the context provider's EC2 read is refused and `cdk synth` fails with that authorization error rather than silently synthesizing a dummy VPC.
Emulator: the check is killed by the harness with "Error: Check timed out after 60.0s" having reached no assertion — neither the successful lookup nor the denied-role synth failure is produced.
Why: environment-aware CDK apps cannot be synthesized against this emulator, and the security-relevant distinction between "the lookup role was denied" and "the lookup returned a dummy VPC" is unobservable — exactly the case that silently deploys a stack into the wrong network.
Fix: the EC2 context-provider read path used by the CDK VPC lookup (and its authorization decision for the assumed lookup role), plus the CloudFormation bootstrap/deploy path those synths depend on — the sequence must terminate with either an answer or the authorization refusal instead of hanging.
Traces: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth.

## Evidence
Expected: `cdk synth` with a permitted lookup role succeeds, and `cdk synth` with a denied lookup role fails, surfacing the EC2 authorization refusal from the context provider.
Actual: "Error: Check timed out after 60.0s" with no assertion reached; neither synth outcome was produced.

## Steps to Reproduce
1. cdk bootstrap a qualifier-scoped toolkit against the emulator and deploy a VPC.
2. `cdk synth` a stack using ec2.Vpc.from_lookup with a lookup role that is permitted to read EC2 — expect success.
3. `cdk synth` the same stack with the lookup role denied the EC2 describe permissions — expect synth to fail with that authorization error.
4. Observe that the sequence never terminates and the check is killed after 60s.
