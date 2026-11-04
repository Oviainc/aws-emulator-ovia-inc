Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth
Issue-Key: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth|_synth#21|Timeout|r = subprocess.run(cmd, cwd=work, env=env, capture_output=True, text=True,
Issue-Commit: 1b7839c8671744d1c674108436f242e0f48906a3

# cloudformation + ec2 + iam + ssm: the CDK EC2-VPC + SSM-parameter context-provider synth scenario never settles, so a denied lookup role yields no verdict

> _Note: this issue was identified against the code base at commit `1b7839c8671744d1c674108436f242e0f48906a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
ONE root cause.

Cause 1 of 1 — the combined CDK context-provider scenario (EC2 VPC lookup + SSM StringParameter lookup) never reaches an observable outcome against the emulator.
Failing check id: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth

AWS: recorded behavior only — nothing in the Smithy models constrains end-to-end synth timing; the model only confirms the read operations the CDK context providers call exist and are plain, error-free list reads (Smithy, com-amazonaws-ec2-amazonec2: DescribeVpcs -> DescribeVpcsResult.Vpcs (paginated, errors: []), DescribeSubnets -> DescribeSubnetsResult.Subnets, DescribeRouteTables -> DescribeRouteTablesResult.RouteTables; plus SSM GetParameter for the parameter context provider). Against AWS this sequence terminates: `cdk bootstrap` of a qualifier-scoped toolkit completes, a VPC/IGW/subnet/route-table estate is created, and `cdk synth` of a stack holding both `ec2.Vpc.from_lookup(tags={"Name": ...})` and `ssm.StringParameter.value_from_lookup(...)` runs both context providers through the assumed `cdk-<qualifier>-lookup-role-<account>-<region>` role and exits. With `ec2:DescribeVpcs` denied to that lookup role, AWS's behaviour is a *refusal that still terminates*: the VPC-lookup context provider fails, `cdk synth` exits non-zero and writes a template carrying CDK's dummy placeholder vpc id `vpc-12345` instead of the real one; removing the deny and synthesising again restores the real context. The documented contract is the CDK context/lookup behaviour (https://docs.aws.amazon.com/cdk/v2/guide/context.html); the fixed part of the expectation is the non-zero exit plus the literal dummy id `vpc-12345`, while the vpc id, subnet ids, route-table ids, AZ names, qualifier, stack name, parameter name and parameter value are all values taken from the request and from emulator state, not constants of the contract.

Emulator: the check never reaches any assertion. The replay detail is exactly `Error: Check timed out after 60.0s`. No status code, no error code and no synth exit status are produced at all: the bootstrap / VPC-build / dual-context-provider synth sequence does not terminate, so neither the allowed-lookup outcome nor the denied-lookup outcome (non-zero exit with the `vpc-12345` dummy) is observable, and the check's own inline teardown verification (toolkit stack DELETE_COMPLETE, staging bucket 404/NoSuchBucket, `/cdk-bootstrap/<qualifier>/version` and the probe parameter ParameterNotFound, vpc InvalidVpcID.NotFound, no `cdk-<qualifier>-*` roles) is never reached either.

Why: a client cannot use CDK environment-aware constructs against this emulator. `cdk synth` of a stack that combines a VPC lookup with an SSM parameter lookup hangs instead of finishing, so the whole CDK workflow stalls; worse, the *rejection* path is unobservable, so a user whose lookup role genuinely lacks `ec2:DescribeVpcs` gets a hang rather than AWS's deterministic non-zero synth with the `vpc-12345` placeholder, and cannot tell a permissions problem from an unavailable service.

Fix: the EC2 context-provider read path that the CDK VPC lookup drives (DescribeVpcs / DescribeSubnets / DescribeRouteTables / DescribeAvailabilityZones, including subnet-to-route-table associations) and the SSM GetParameter path the parameter context provider drives, together with the CloudFormation bootstrap/deploy path those synths depend on: each must return an answer (or a modelled refusal) promptly instead of leaving the caller waiting. The related open issue 20261104T045949Z-cloudformation-ec2-iam-the-cdk-vpc-context-provider.md reports the same non-termination for the VPC-lookup-cache-only variant (check cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role); it does not carry this check's replay key and does not exercise the SSM parameter context provider or the denied-lookup/dummy-id refusal path, so this check needs its own closure. Also related: 20261028T094216Z-cloudformation-ec2-the-cdk-vpc-diff-drift-scenario.md.

Traces: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth.

## Evidence
Expected (AWS): the scenario terminates. `cdk bootstrap` completes; the VPC/IGW/subnets/route tables are created; a first `cdk synth` resolves both the EC2 VPC-lookup and the SSM StringParameter context providers through the assumed lookup role and writes a template with the real vpc id and the real parameter value; after denying `ec2:DescribeVpcs` to the lookup role, `cdk synth` exits NON-ZERO and writes a template carrying CDK's dummy placeholder vpc id `vpc-12345`; after removing the deny, synth resolves the real context again; the inline teardown then verifies toolkit stack DELETE_COMPLETE, staging bucket gone (404/NoSuchBucket/NotFound), `/cdk-bootstrap/<qualifier>/version` and the probe parameter ParameterNotFound, the vpc InvalidVpcID.NotFound and no remaining `cdk-<qualifier>-*` roles.

Actual (emulator): no outcome at all. The suite reports, verbatim, `Error: Check timed out after 60.0s`. No HTTP status, no error code and no synth exit code were produced; the check's own budgets (_BOOTSTRAP_TIMEOUT_S = 900.0, _SYNTH_TIMEOUT_S = 600.0, _TEARDOWN_DEADLINE_S = 420.0) were never consumed because the sequence never progressed to a terminal result, and the denied-lookup assertion on the literal `vpc-12345` was never evaluated.

## Steps to Reproduce
1. `cdk bootstrap` a qualifier-scoped toolkit against the emulator (qualifier Q, toolkit stack and `cdk-<Q>-assets-<account>-<region>` staging bucket).
2. With boto3, create a VPC (10.42.0.0/16) tagged Name=<vpc-name>, attach an internet gateway, and create two subnets in two available AZs with public/isolated route tables.
3. Put an SSM parameter at /sim2real/cdklookup/<unique> holding a marker value.
4. `cdk synth` a stack whose constructs are `ec2.Vpc.from_lookup(self, "V", tags={"Name": <vpc-name>})` and `ssm.StringParameter.value_from_lookup(self, <param-name>)`, using DefaultStackSynthesizer(qualifier=Q), so both context providers run through the assumed `cdk-<Q>-lookup-role-<account>-<region>` role.
5. Attach a deny on `ec2:DescribeVpcs` to that lookup role and `cdk synth` again; expect a non-zero exit and a template containing the dummy `vpc-12345`.
6. Remove the deny and synth again; expect the real vpc id back, then run the inline teardown verification.
7. Observe that the sequence never terminates and the check is killed with `Error: Check timed out after 60.0s`, having reached no assertion.
