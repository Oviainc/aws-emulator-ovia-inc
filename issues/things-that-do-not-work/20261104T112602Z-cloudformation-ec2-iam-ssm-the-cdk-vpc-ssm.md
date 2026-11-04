Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth
Issue-Key: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth|_poll_for_denied_synth#21|Timeout|return rc, out
Issue-Commit: aa67ef9b7926b9d86487a1c336b627f99e898509

# cloudformation__ec2__iam__ssm: the CDK VPC/SSM context-provider synth scenario never terminates, so a denied lookup role produces no verdict

> _Note: this issue was identified against the code base at commit `aa67ef9b7926b9d86487a1c336b627f99e898509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
ONE net-new root cause of the `cloudformation__ec2__iam__ssm` group, grounded in the failing behavioral check listed in trace_ids. The replay report marked it neither ALREADY FILED nor OWNED. The open issue 20261104T045949Z-cloudformation-ec2-iam-the-cdk-vpc-context-provider.md records the same shape for the sibling check cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role; it does not carry this check's replay key and does not cover the SSM-parameter leg or the denied-synth verdict graded here.

Cause 1 of 1: the CDK context-provider lookup path never reaches an observable outcome.
AWS: recorded behavior only (no Smithy operation models CDK's synth-time context resolution). The check cdk-bootstraps a qualifier-scoped toolkit, builds a real VPC with subnets, route tables and an internet gateway via boto3, writes an SSM parameter, then runs `cdk synth` on a stack that uses ec2.Vpc.from_lookup(tags={"Name": ...}) and ssm.StringParameter.value_from_lookup(...). Against AWS each synth terminates: the EC2 context provider resolves the VPC through the assumed `cdk-<qualifier>-lookup-role-<account>-<region>` (DescribeVpcs / DescribeSubnets / DescribeRouteTables / DescribeAvailabilityZones) and partitions the subnets into Public/Isolated by their route tables, and the SSM provider resolves the parameter. With ec2:DescribeVpcs denied to that lookup role, `cdk synth` still writes a template — carrying CDK's dummy placeholder VPC id `vpc-12345` instead of a real one — and exits non-zero; removing the deny restores the original resolved context.
Emulator: the check is killed by the harness with `Error: Check timed out after 60.0s` having reached no assertion. The bootstrap / VPC-build / three-synth sequence never produces a terminal result, so neither the resolved-context behaviour, nor the dummy-`vpc-12345`-plus-non-zero-exit behaviour of a denied lookup role, nor the restore-after-removing-the-deny behaviour is ever observable.
Why: a client cannot use CDK environment-aware constructs against this emulator at all — `cdk synth` of any stack containing a Vpc.from_lookup or a StringParameter.value_from_lookup hangs instead of completing, so the whole context-provider workflow (and the IAM-gated failure mode CDK documents for it) is unusable.
Fix: the EC2 and SSM read paths the CDK context providers call during synth (DescribeVpcs / DescribeSubnets / DescribeRouteTables / DescribeAvailabilityZones for a just-created VPC, and GetParameter) together with the CloudFormation bootstrap/deploy path those synths depend on — each call must answer, and the sequence must terminate with a result instead of hanging. The denied-role arm must leave synth free to emit the dummy placeholder id and a non-zero exit.
Traces: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth.

## Evidence
Check: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth [mismatch]
Expected: the scenario completes — a first `cdk synth` resolves the VPC-lookup and SSM-parameter context providers and writes the real VPC id with its subnets partitioned Public/Isolated by route table; after ec2:DescribeVpcs is denied to the `cdk-<qualifier>-lookup-role-<account>-<region>` role, `cdk synth` exits non-zero and writes CDK's dummy placeholder id `vpc-12345`; after the deny is removed the original resolved context is restored.
Actual: `Error: Check timed out after 60.0s` with no assertion reached. The check's own bounds allow a 900s bootstrap and 600s per synth, and it calls require_tools("cdk") up front (so a missing tool would be reported as such, not as a timeout) — the sequence simply never terminates against the emulator.
The qualifier, stack/toolkit/VPC names and parameter path are per-run values; the requirement is that the lookup calls answer and each synth terminates, and that the denied-role synth yields the literal placeholder `vpc-12345` with a non-zero exit.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth from suite simparity-behavioral-aws-v1; observe it is killed at the 60s harness ceiling with no assertion reached.
2. By hand: `cdk bootstrap` a qualifier-scoped toolkit against the emulator.
3. With boto3, CreateVpc(CidrBlock=10.42.0.0/16) tagged Name=<vpc-name>, add subnets, route tables and an internet gateway, and PutParameter a String parameter.
4. Write a CDK app whose stack calls ec2.Vpc.from_lookup(self, "V", tags={"Name": <vpc-name>}) and ssm.StringParameter.value_from_lookup(self, <param-name>), with a DefaultStackSynthesizer(qualifier=<q>) and an explicit cdk.Environment(account, region); run `cdk synth`. Observe that it does not terminate.
5. Attach a Deny for ec2:DescribeVpcs to cdk-<qualifier>-lookup-role-<account>-<region> and `cdk synth` again, expecting a non-zero exit and a template containing vpc-12345; then remove the deny and `cdk synth` once more, expecting the originally resolved context.
