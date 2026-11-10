Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-asg-availability-zones-derived-from-subnets ec2.e2e-ec2mr-usg-unused-security-groups-lifecycle ec2.ec2mr-pip-e2e-multi-eni-inventory-lifecycle cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance dynamodb__ec2.e2e-ddb-ltr-list-tables-all-regions-lifecycle cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set
Issue-Key: autoscaling__cloudformation__ec2.cfn-asg-availability-zones-derived-from-subnets|_deploy_cfn#6|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: ec2.e2e-ec2mr-usg-unused-security-groups-lifecycle|phase#4|_PhaseError|raise _PhaseError(
Issue-Key: ec2.ec2mr-pip-e2e-multi-eni-inventory-lifecycle|phase#7|_PhaseError|raise _PhaseError(
Issue-Key: cloudformation__ec2|DescribeVolumes|InvalidParameterValue
Issue-Key: cloudformation__ec2__ssm|DescribeInstances|InvalidInstanceID.Malformed
Issue-Key: dynamodb__ec2|DescribeTable|ResourceNotFoundException
Issue-Key: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created|_cdk#14|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count|_cdk#11|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group|_cdk#18|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set|run#12|AssertionError|assert before == baseline_azs, (
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# CloudFormation stack outputs are unusable and CREATE_COMPLETE is reported for resources that were never created: no OutputValue, logical names instead of physical ids, missing DynamoDB table, no CDK bootstrap parameter, GetAZs disagrees with the account's default subnets (5 causes)

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Five independent CloudFormation fidelity defects, all reached through stacks the behavioral suite deploys as estate. All five must be closed before submitting. Each one breaks the hand-off from a deployed stack to the services it provisioned, so every IaC-shaped client stops at step one.

Cause 1 of 5: DescribeStacks returns Output entries with no OutputValue member.
AWS: each Stacks[].Outputs[] entry carries OutputKey and OutputValue (and optionally Description/ExportName); OutputValue is how the resolved Ref/GetAtt reaches the caller. Recorded behavior only for the exact set.
Emulator: reading o['OutputValue'] over the returned Outputs raises KeyError 'OutputValue' — the key is absent.
Why: a client cannot read a single value out of a stack it just deployed, so every downstream call is made with nothing. Related open issue 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md names this family but does not carry these three checks.
Fix: Outputs serialization in the DescribeStacks handler (and output resolution in the stack engine).
Traces: autoscaling__cloudformation__ec2.cfn-asg-availability-zones-derived-from-subnets, ec2.e2e-ec2mr-usg-unused-security-groups-lifecycle, ec2.ec2mr-pip-e2e-multi-eni-inventory-lifecycle.

Cause 2 of 5: stack outputs resolve Ref/GetAtt to a synthesized logical name instead of the created resource's physical id.
AWS: Ref on an AWS::EC2::Instance resolves to i-..., on an AWS::EC2::Volume to vol-...; the value is the physical resource id EC2 will accept.
Emulator: the output value is a synthesized '<stack>-<LogicalId>-<hex>' string, so the very next EC2 call rejects it: DescribeVolumes answers InvalidParameterValue "Value (sim2real-xsvc-ec2vol-...-SnapVol-FA6F5FBDE6CD) for parameter volumes is invalid. Expected: 'vol-...'", DescribeInstances answers InvalidInstanceID.Malformed "Invalid id: 'sim2real-xsvc-dec3-sg-...-Target-E8F87D0FAD5F'".
Why: nothing a stack creates can be addressed afterwards; the error surfaces in the neighbouring service, which makes it look like an EC2 defect. Related open issue 20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md names this family but does not carry these two checks.
Fix: physical-id assignment and Ref/GetAtt resolution in the stack engine's resource provisioning.
Traces: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain, cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance.

Cause 3 of 5: a stack reaches CREATE_COMPLETE while a declared resource was never created.
AWS: CREATE_COMPLETE means every resource in the template exists and is describable in its own service; an AWS::DynamoDB::Table in a complete stack is returned by dynamodb:DescribeTable.
Emulator: the stack reports complete, then DescribeTable answers ResourceNotFoundException 'Requested resource not found: Table: <name> not found'.
Why: the terminal stack status lies, so clients that gate on it proceed against resources that do not exist. Related open issue 20261022T175927Z-ds-ec2-e2e-cloudformation-reports-create-complete-but.md names this family but does not carry this check.
Fix: resource provisioning / status transition in the stack engine (AWS::DynamoDB::Table resource type).
Traces: dynamodb__ec2.e2e-ddb-ltr-list-tables-all-regions-lifecycle.

Cause 4 of 5: the CDK bootstrap version parameter never resolves, so no CDK app can deploy.
AWS: `cdk bootstrap` deploys the CDKToolkit stack, which creates the SSM parameter /cdk-bootstrap/<qualifier>/version; `cdk deploy` then reads it and proceeds. Recorded behavior only.
Emulator: `cdk deploy` exits rc=1 with 'SsmParameterNotFound: SSM parameter /cdk-bootstrap/<qualifier>/version not found in aws://<account>/<region>. Has the environment been bootstrapped?' and 'ParameterNotFound: Parameter /cdk-bootstrap/<qualifier>/version not found', with a different qualifier per app.
Why: the entire CDK front door is closed — no CDK-authored stack can be deployed against the emulator.
Fix: the CDKToolkit bootstrap stack path in the stack engine (AWS::SSM::Parameter provisioning and its visibility to ssm:GetParameter).
Traces: cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created, cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count, cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group.

Cause 5 of 5: Fn::GetAZs and the account's default-subnet set disagree, and the default subnets are not scoped to the session's region.
AWS: Fn::GetAZs resolves to the Availability Zones of the stack's region, and the account's default subnets live in that same region, so the two agree; the check reads the account's default subnets with ec2:DescribeSubnets filtered default-for-az=true.
Emulator: a stack deployed in us-west-2 resolved Fn::GetAZs to ['us-west-2a','us-west-2b','us-west-2c'] (the region's AZs) while DescribeSubnets(default-for-az=true) reported default subnets in ['us-east-1a','us-east-1b','us-east-1c'] — a different region entirely.
Why: a template that launches into the AZs GetAZs returns lands in AZs where the account has no default subnet, and region-scoped clients see cross-region state.
Fix: Fn::GetAZs resolution in the stack engine's intrinsic-function evaluation and/or region scoping of the default-VPC/default-subnet baseline the EC2 state exposes.
Traces: cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set.

## Evidence
Cause 1 — KeyError: 'OutputValue' at `outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}`; also raised as _PhaseError: [phase 1/5 deploy] KeyError: 'OutputValue' and [phase 1/4 deploy] KeyError: 'OutputValue'.
Cause 2 — botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DescribeVolumes operation: Value (sim2real-xsvc-ec2vol-94030cf74832-SnapVol-FA6F5FBDE6CD) for parameter volumes is invalid. Expected: 'vol-...'. / An error occurred (InvalidInstanceID.Malformed) when calling the DescribeInstances operation: Invalid id: 'sim2real-xsvc-dec3-sg-7798bbb7427a-Target-E8F87D0FAD5F'.
Cause 3 — _PhaseError: [phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: sim2real-scn-ltr-27687268e037-table1-us-west-2 not found.
Cause 4 — AssertionError: `cdk deploy` rc=1, expected 0: "... SSM parameter /cdk-bootstrap/q9c2d2ab3e/version not found in aws://000000000000/us-west-2. Has the environment been bootstrapped? ... ParameterNotFound: Parameter /cdk-bootstrap/q9c2d2ab3e/version not found" (same with qualifiers q94e21866b and q4338d5d2e).
Cause 5 — AssertionError: Fn::GetAZs must return exactly the AZs where this account has a DEFAULT SUBNET. ec2 DescribeSubnets(defaultForAz=true) says ['us-east-1a', 'us-east-1b', 'us-east-1c']; the template resolved ['us-west-2a', 'us-west-2b', 'us-west-2c'].

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros: (1) deploy any template with an Outputs section, then cloudformation:DescribeStacks and read Stacks[0].Outputs[0]['OutputValue']. (2) deploy a template declaring an AWS::EC2::Volume (or AWS::EC2::Instance) with an Output of Ref on it, then pass that output value to ec2:DescribeVolumes / ec2:DescribeInstances. (3) deploy a template declaring an AWS::DynamoDB::Table, wait for CREATE_COMPLETE, then dynamodb:DescribeTable on the table name. (4) run `cdk bootstrap` then `cdk deploy` for any CDK app, or ssm:GetParameter Name='/cdk-bootstrap/<qualifier>/version'. (5) deploy a template whose output is Fn::GetAZs in us-west-2 and compare with ec2:DescribeSubnets Filters=[{'Name':'default-for-az','Values':['true']}].
