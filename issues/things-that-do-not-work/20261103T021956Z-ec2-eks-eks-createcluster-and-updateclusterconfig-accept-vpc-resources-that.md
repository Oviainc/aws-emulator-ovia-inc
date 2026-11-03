Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__eks.ekscc-create-cluster-with-subnets-from-two-vpcs-rejected ec2__eks.ekscc-update-cluster-config-security-group-from-another-vpc-rejected
Issue-Key: ec2__eks.ekscc-create-cluster-with-subnets-from-two-vpcs-rejected|_error#7|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: ec2__eks.ekscc-update-cluster-config-security-group-from-another-vpc-rejected|_refusal#20|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# EKS CreateCluster and UpdateClusterConfig accept VPC resources that span two VPCs (2 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Two net-new root causes in EKS's resourcesVpcConfig validation, replayed under the `ec2__eks` suite heading.

Cause 1 of 2: CreateCluster accepts subnets drawn from two different VPCs.
AWS: recorded behavior only. Every subnet in resourcesVpcConfig.subnetIds must belong to ONE VPC; a subnet set spanning two VPCs is refused and no cluster is created.
Emulator: the call SUCCEEDS.
Why: the cluster's VPC is derived from its subnets; a cross-VPC subnet set has no single VPC, so the control-plane ENIs and the cluster security group cannot be placed. The emulator creates a cluster that is unbuildable on AWS, and a client relying on the refusal to validate its subnet selection ships the bad selection.
Fix: the subnet/VPC consistency validation in the EKS CreateCluster handler, before cluster state is written.
Traces: ec2__eks.ekscc-create-cluster-with-subnets-from-two-vpcs-rejected

Cause 2 of 2: UpdateClusterConfig accepts a security group from another VPC.
AWS: recorded behavior only. resourcesVpcConfig.securityGroupIds must name groups in the cluster's own VPC; a group from a different VPC is refused.
Emulator: the call SUCCEEDS.
Why: a security group cannot be attached to an ENI outside its VPC, so the update is accepted here and rejected by AWS. Worse, the stored config now claims a group that will never apply, so a subsequent read suggests the cluster's network access was narrowed when it was not - a security-relevant divergence.
Fix: the securityGroupIds VPC-membership validation in the EKS UpdateClusterConfig handler.
Traces: ec2__eks.ekscc-update-cluster-config-security-group-from-another-vpc-rejected

## Evidence
Replayed failing checks under `### ec2__eks` (neither marked ALREADY FILED / OWNED by):

1. ec2__eks.ekscc-create-cluster-with-subnets-from-two-vpcs-rejected -> AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code
2. ec2__eks.ekscc-update-cluster-config-security-group-from-another-vpc-rejected -> AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; both checks appear under `### ec2__eks`, unmarked.
2. Standalone repros (two VPCs, each with subnets and a security group):
   - eks.create_cluster(name=N, roleArn=R, resourcesVpcConfig={'subnetIds': ['<subnet in VPC A>', '<subnet in VPC B>']}): expect a refusal and no cluster.
   - Create a cluster wholly in VPC A, then eks.update_cluster_config(name=N, resourcesVpcConfig={'securityGroupIds': ['<sg in VPC B>']}): expect a refusal, and the stored config must be unchanged.
