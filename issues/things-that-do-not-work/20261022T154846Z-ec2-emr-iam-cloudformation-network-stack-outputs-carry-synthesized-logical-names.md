Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr__iam.e2e-create-emr-cluster-multi-master-lifecycle
Issue-Key: ec2__emr__iam.e2e-create-emr-cluster-multi-master-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# CloudFormation network-stack Outputs carry synthesized logical names instead of real vpc-/subnet-/rtb- ids

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause (the same defect as the ec2__emr filing, observed on this suite's own network stack and recorded here because this check's service token differs and a fixer must re-replay it separately).

Cause 1 of 1: a CREATE_COMPLETE network stack's Outputs carry '<stack-name>-<LogicalId>-<hex>' strings where AWS carries the real EC2 physical ids.
AWS: CloudFormation's Ref/GetAtt of a declared resource resolves to the physical id the owning service minted — AWS::EC2::VPC -> vpc-<17 hex>, AWS::EC2::Subnet -> subnet-<17 hex>, AWS::EC2::RouteTable -> rtb-<17 hex> — and those ids are resolvable through the EC2 API (DescribeVpcs/DescribeSubnets/DescribeRouteTables). Recorded behavior for the id shapes; the contract itself is CloudFormation's documented Ref/GetAtt semantics.
Emulator: VpcId = 'sim2real-scn-emrmm-<hex>-net-Vpc-7C53B723DBA8', SubnetId = '...-net-Subnet-9E3DB5D0D8FA', RouteTableId = '...-net-RouteTable-27CDE8184FD7'. The stack reports its outputs as present, but none is an EC2 id. Names/hex differ per run; the defect is that the value is stack-derived rather than a service id.
Why: the multi-master EMR workflow (and any deploy-then-operate workflow) cannot hand the deployed network to RunJobFlow's Instances.Ec2SubnetId(s), and EC2's own Describe* APIs do not recognise the ids, so the stack is inert even though it reports success.
Fix: the stack-resource provisioning and Ref/GetAtt/Output resolution in services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_stack.rs and the resource-provisioning/state module behind it — declared resources must be created in the owning service so their real ids flow into Outputs.
Traces: ec2__emr__iam.e2e-create-emr-cluster-multi-master-lifecycle (phase 1/6 deploy).

## Evidence
Expected: stack Outputs VpcId='vpc-…', SubnetId='subnet-…', RouteTableId='rtb-…'.
Actual: AssertionError: [phase 1/6 deploy] stack sim2real-scn-emrmm-0a2f32ba49cf-net outputs lack VpcId/SubnetId: {'VpcId': 'sim2real-scn-emrmm-0a2f32ba49cf-net-Vpc-7C53B723DBA8', 'SubnetId': 'sim2real-scn-emrmm-0a2f32ba49cf-net-Subnet-9E3DB5D0D8FA', 'RouteTableId': 'sim2real-scn-emrmm-0a2f32ba49cf-net-RouteTable-27CDE8184FD7'}.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client('cloudformation'); create a stack declaring AWS::EC2::VPC, AWS::EC2::Subnet and AWS::EC2::RouteTable with Outputs that Ref each.
3. Wait for CREATE_COMPLETE and read DescribeStacks Outputs -> expect vpc-/subnet-/rtb- ids; observe '<stack>-<LogicalId>-<hex>' strings.
4. Pass the SubnetId to emr.run_job_flow(Instances={'Ec2SubnetId': ...}) or ec2.describe_subnets -> the value is not a known subnet.
5. Re-replay ec2__emr__iam.e2e-create-emr-cluster-multi-master-lifecycle.
