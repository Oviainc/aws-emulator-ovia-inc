Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth
Issue-Key: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth|_poll_for_denied_synth#21|Timeout|rc, out = _synth(work, env, expect_rc=None, timeout_s=_SYNTH_TIMEOUT_S)
Issue-Commit: c0478c2dc62a949b47f12552e33c2facb0cc4b79

# cloudformation__ec2__iam__ssm: the CDK VPC + SSM context-provider synth sequence never terminates, so the denied-lookup-role arm produces no verdict

> _Note: this issue was identified against the code base at commit `c0478c2dc62a949b47f12552e33c2facb0cc4b79`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
ONE root cause. This is the only failing check in this sweep that is neither marked ALREADY FILED nor OWNED by an open issue.

Cause 1 of 1 — the CDK context-provider synth sequence (EC2 VPC lookup + SSM parameter lookup) never reaches an observable outcome, so the check is killed by the harness before any assertion runs.
Failing check ids: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth

AWS: recorded behavior only for the end-to-end sequence; the individual calls it drives are modeled. The check cdk-bootstraps a qualifier-scoped toolkit, builds a VPC with an internet gateway, two public subnets associated with a route table carrying a 0.0.0.0/0 route to that IGW and two isolated subnets associated with a route table with no egress route, writes an SSM String parameter, then runs `cdk synth` three times. Against AWS each synth terminates: the first resolves the EC2 VPC-lookup context provider through the assumed CDK lookup role (Smithy, EC2: DescribeVpcs -> DescribeVpcsResult.Vpcs (vpcSet), DescribeSubnets -> Subnets (subnetSet), DescribeRouteTables -> RouteTables (routeTableSet), DescribeAvailabilityZones, all Filters-driven, all paginated with NextToken; none declares a modeled error) plus the SSM parameter-lookup provider (SSM GetParameter), writes a single vpc-provider: key and a single ssm: key into cdk.context.json and a template whose SecurityGroup VpcId is the LITERAL real vpc id; the second synth, run with ec2:DescribeVpcs explicitly denied to the lookup role, exits NON-ZERO with output naming ec2:DescribeVpcs and the lookup role, writes NO vpc-provider: context entry and substitutes CDK's dummy literal vpc-12345 in the template; the third synth, after the deny is removed, terminates again and resolves to exactly the first context. Every one of those outcomes is a terminal process exit.

Emulator: the check never terminates. The replay reports only `Error: Check timed out after 60.0s`, with no assertion reached — the bootstrap / VPC-build / synth sequence produces no terminal result against the emulator. The check's own budgets (900s bootstrap, 600s per synth) are never approached; it is killed while still driving the sequence, so neither the positive resolution nor the denied-lookup-role refusal arm is observable. No status, error code or error message is produced by the emulator at all for this scenario — the absence of any response is the divergence.

Why: a client cannot use CDK environment-aware constructs against this emulator. `cdk synth` of a stack containing ec2.Vpc.from_lookup and ssm.StringParameter.value_from_lookup hangs rather than completing, so neither the resolved-context path (real vpc id, route-table-derived Public/Isolated partition, per-subnet routeTableId and availabilityZone, resolved parameter value) nor the refusal path (non-zero exit naming ec2:DescribeVpcs and the lookup role, dummy vpc-12345 substituted, no fabricated vpc-provider context entry) can be relied on. A denied lookup role is indistinguishable from an emulator that simply never answers.

Fix: the EC2 context-provider read path the CDK VPC lookup drives — the DescribeVpcs / DescribeSubnets / DescribeRouteTables / DescribeAvailabilityZones handlers, including subnet-to-route-table associations and tag-based Filters — together with the SSM GetParameter read the parameter provider uses and the CloudFormation bootstrap/deploy path the toolkit stack needs. The sequence must terminate with an answer (or a modeled refusal) instead of hanging, and the denied arm must surface the authorization refusal for ec2:DescribeVpcs rather than stalling.

Related open issue, which does NOT cover this check: 20261104T045949Z-cloudformation-ec2-iam-the-cdk-vpc-context-provider.md describes the same class of hang for the sibling check cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role, but its replay key names only that check and its service heading is cloudformation__ec2__iam; it does not carry this check's replay key, and this check additionally exercises the SSM parameter context provider and the denied-synth/dummy-id refusal arm.

Traces: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth.

## Evidence
Expected (AWS): three `cdk synth` runs all terminate. (1) First synth exits 0 and writes cdk.context.json containing exactly one `vpc-provider:` key whose vpcId is the created VPC, vpcCidrBlock 10.42.0.0/16, ownerAccountId the caller's account, subnetGroups partitioned exactly into ['Isolated', 'Public'] by route table, each subnet reporting its real routeTableId and availabilityZone, plus exactly one `ssm:` key equal to the parameter value written; the emitted template's SecurityGroup VpcId is the literal real vpc id and the Marker output is the resolved parameter value. (2) With `Deny ec2:DescribeVpcs` on the CDK lookup role, synth exits non-zero, its output names both `ec2:DescribeVpcs` and the lookup role name, no `vpc-provider:` key is written, and the template carries CDK's dummy literal `vpc-12345`. (3) After the deny is removed, synth exits 0 and resolves to byte-identical context.

Actual (emulator): `Error: Check timed out after 60.0s`. No assertion was reached and no response was observed for any of the three arms — the emulator produced no terminal outcome for the CDK context-provider sequence.

## Steps to Reproduce
1. `cdk bootstrap` a qualifier-scoped toolkit stack against the emulator (this mints the cdk-<qualifier>-lookup-role-<account>-<region> lookup role and the staging bucket).
2. With boto3, create a VPC (10.42.0.0/16) tagged Name=<vpc-name>, attach an internet gateway, create two subnets per AZ across two AZs, associate one pair with a route table carrying 0.0.0.0/0 -> the IGW and the other pair with a route table with no routes; put an SSM String parameter.
3. Synthesize a CDK app whose stack calls ec2.Vpc.from_lookup(tags={'Name': <vpc-name>}) and ssm.StringParameter.value_from_lookup(<param-name>) and declares a SecurityGroup in the looked-up VPC. Run `cdk synth --ci`.
4. Observe that the sequence never terminates: the check is killed after 60s with no assertion reached, so the resolved cdk.context.json is never produced.
5. (Unreachable today) put an inline `Deny ec2:DescribeVpcs` policy on the lookup role, clear cdk.out/cdk.context.json and re-synth, expecting a non-zero exit naming ec2:DescribeVpcs and the lookup role, no vpc-provider context entry, and the dummy vpc-12345 in the template; then remove the deny and re-synth, expecting the identical original context.
