Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle
Issue-Key: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# CloudFormation stack Outputs return synthesized logical names instead of the real physical resource ids of the EC2/EMR resources the stack declares

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause.

Cause 1 of 1: a CREATE_COMPLETE stack's Outputs carry fabricated "<stack-name>-<LogicalId>-<hex>" strings where AWS carries the created resource's real physical id.
AWS: CloudFormation creates each declared resource in its owning service and the resource's physical id is that service's own id — Ref of an AWS::EC2::VPC resolves to a vpc-<17 hex> id, AWS::EC2::Subnet to subnet-<17 hex>, AWS::EC2::SecurityGroup (GroupId) to sg-<17 hex>, AWS::EMR::Cluster to a j-<13 upper-alnum> job-flow id. An Output whose Value is a Ref/GetAtt of such a resource therefore carries that id, and the id is resolvable by the owning service's Describe* API. (Recorded behavior for the exact id shapes; the stack-Outputs-carry-physical-ids contract is CloudFormation's documented Ref/GetAtt semantics.)
Emulator: the Outputs of the deployed estate stack carry 'sim2real-scn-emrdiag-<hex>-estate-Vpc-14095215865C' for VpcId, '...-estate-Subnet-D9BC2BFE6B80' for SubnetId and '...-estate-PrimarySg-8E1B9D13F518'/'...-estate-CoreSg-...' for the security groups, and 'sim2real-scn-emrdiag-<hex>-spark' for ClusterId — i.e. the physical id is synthesized from the stack name and the logical id rather than taken from the resource the stack was supposed to create in EC2/EMR. Generated names and hex suffixes differ per run; the defect is the SHAPE (a stack-derived name, not a service id).
Why: every client that deploys infrastructure with CloudFormation and then drives it through the service APIs breaks at the first hand-off: the VpcId/SubnetId cannot be passed to RunJobFlow or DescribeSubnets, the GroupIds are not security groups EC2 knows, and the ClusterId is not a job-flow id DescribeCluster accepts. Any deploy-then-operate workflow is unreachable even though the stack reports CREATE_COMPLETE.
Fix: the stack-resource provisioning and Ref/GetAtt/Output resolution in the CloudFormation service crate, services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_stack.rs and the resource-provisioning/state module behind it — each declared resource must be created through the owning service's state so its real physical id is what Ref/GetAtt and Outputs return.
Traces: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle (phase 1/4 deploy).

## Evidence
Expected: stack Outputs {VpcId: 'vpc-…', SubnetId: 'subnet-…', PrimarySgId/CoreSgId: 'sg-…', ClusterId: 'j-…'} resolvable via ec2.describe_subnets / emr.describe_cluster.
Actual: AssertionError: [phase 1/4 deploy] stack sim2real-scn-emrdiag-572a47e35e9c-estate outputs are missing or malformed: {'ClusterId': 'sim2real-scn-emrdiag-572a47e35e9c-spark', 'VpcId': 'sim2real-scn-emrdiag-572a47e35e9c-estate-Vpc-14095215865C', 'SubnetId': 'sim2real-scn-emrdiag-572a47e35e9c-estate-Subnet-D9BC2BFE6B80', 'PrimarySgId': 'sim2real-scn-emrdiag-572a47e35e9c-estate-PrimarySg-8E1B9D13F518', 'CoreSgId': 'sim2real-scn-emrdiag-572a47e35e9c-estate-CoreSg-B240AACA35A6'}.
The same defect is visible from the other side in the emr__glue__s3 CFN checks, where DescribeCluster on the stack-derived ClusterId answers 400 InvalidRequestException "Cluster id '…' is not valid or does not exist."

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client('cloudformation'); create a stack declaring AWS::EC2::VPC, AWS::EC2::Subnet, two AWS::EC2::SecurityGroup and an AWS::EMR::Cluster, with Outputs that Ref/GetAtt each of them.
3. Wait for CREATE_COMPLETE, then read DescribeStacks Outputs -> expect vpc-/subnet-/sg-/j- ids; observe '<stack>-<LogicalId>-<hex>' strings.
4. Pass the VpcId/SubnetId to ec2.describe_subnets and the ClusterId to emr.describe_cluster -> both refuse the value.
5. Re-replay ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle.
