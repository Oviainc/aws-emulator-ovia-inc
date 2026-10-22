Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.eks-access-entry-deletion-leaves-the-principal-unauthorized eks.eks-delete-cluster-with-a-live-node-is-rejected eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule eks.eks-irsa-projected-token-assumed-inside-the-pod-and-sub-condition-denied eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod eks.certificate-authority-lifecycle-transitions eks.nodegroup-full-lifecycle-transitions eks.certificate-authority-data-base64-round-trip eks.nodegroup-labels-empty-vs-absent-preserved eks.deploy-cfn-cluster-active-and-usable eks.deploy-cfn-cluster-behaves eks.deploy-cfn-podidentity-association-usable
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.eks-access-entry-deletion-leaves-the-principal-unauthorized|_fixture_subnets#3|AssertionError|raise AssertionError(
Issue-Key: eks.eks-delete-cluster-with-a-live-node-is-rejected|_fixture_subnets#2|AssertionError|raise AssertionError(
Issue-Key: eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule|_fixture_subnets#1|AssertionError|raise AssertionError(
Issue-Key: eks.eks-irsa-projected-token-assumed-inside-the-pod-and-sub-condition-denied|_fixture_subnets#23|AssertionError|assert len(by_az) >= 2, (
Issue-Key: eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster|_fixture_subnets#1|AssertionError|raise AssertionError(
Issue-Key: eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied|_fixture_subnets#3|AssertionError|raise AssertionError(
Issue-Key: eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni|_fixture_subnets#16|AssertionError|assert len(by_az) >= 2, (
Issue-Key: eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod|_fixture_subnets#2|AssertionError|assert len(by_az) >= 2, (
Issue-Key: eks.certificate-authority-lifecycle-transitions|_create_active_cluster#2|KeyError|subnet_ids = _prep["subnet_ids"]
Issue-Key: eks.nodegroup-full-lifecycle-transitions|_create_active_cluster#6|KeyError|subnet_ids = _prep["subnet_ids"]
Issue-Key: eks|CreateSecurityGroup|NotImplemented
Issue-Key: eks|DescribeCluster|ResourceNotFoundException
Issue-Key: eks.deploy-cfn-podidentity-association-usable|run#11|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in cfn.describe_stacks(StackName=stack_name)["Stacks"][0]["Outputs"]}
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EKS clusters cannot be provisioned at all: no default VPC/subnet state, EC2 CreateVpc/CreateSecurityGroup stubs, and CloudFormation reports CREATE_COMPLETE without creating the cluster (5 causes)

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Every EKS check that needs real network prerequisites, and every check that provisions EKS through CloudFormation, fails before it can assert anything about EKS. There are FIVE distinct root causes; all five must be closed.

Cause 1 of 5 — the emulator's EC2 account state has no default VPC subnets, so `DescribeSubnets` with the `default-for-az` filter returns nothing.
AWS: recorded behavior only — in a normal account/region DescribeSubnets(Filters=[{'Name':'default-for-az','Values':['true']}]) returns one default subnet per availability zone of the default VPC, which is how these fixtures pick two subnets in two EKS-supported AZs. DescribeAvailabilityZones and DescribeInstanceTypeOfferings DO answer in the emulator (the fixtures' AZ/offering assertions pass), so only the subnet state is missing.
Emulator: the filtered DescribeSubnets returns an empty Subnets list, and the fixtures abort with "EKS needs subnets in >=2 supported AZs; the default VPC offers []". (Related and already filed elsewhere: DescribeVpcs rejects the `isDefault` filter with InvalidParameterValue "The filter 'isDefault' is invalid" — same default-VPC gap, owned by another issue.)
Why: nothing that requires a cluster can be exercised — access entries, IRSA, pod identity, EBS CSI, pod networking and nodegroup taints are all unreachable, and a client that bootstraps from the default VPC gets an empty result where AWS always has subnets.
Fix: EC2 account-state seeding in services/service-com-amazonaws-ec2-amazonec2 (a default VPC with a default subnet per AZ) and the `default-for-az` subnet filter in DescribeSubnets.
Traces: eks.eks-access-entry-deletion-leaves-the-principal-unauthorized, eks.eks-delete-cluster-with-a-live-node-is-rejected, eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule, eks.eks-irsa-projected-token-assumed-inside-the-pod-and-sub-condition-denied, eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster, eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied, eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni, eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod.

Cause 2 of 5 — the fallback path that mints its own network dies on EC2 CreateVpc.
AWS: recorded behavior only — CreateVpc returns a Vpc with a VpcId, after which the fixture creates an IGW, route table and subnets.
Emulator: DescribeVpcs for the run's shared network returns no VPC, so the fixture falls back to ec2.create_vpc, which answers 501 `NotImplemented` ("operation CreateVpc is scaffolded but not implemented"). Because that call runs on a worker thread the exception is swallowed and the check surfaces as `KeyError: 'subnet_ids'`. NOTE: an open issue already names ec2 CreateVpc and its handler file; it does not carry these two checks' replay keys, so they are listed here and must be re-replayed when CreateVpc lands.
Why: there is no way to obtain a usable VPC in the emulator, by discovery or by creation.
Fix: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc.rs (plus the IGW/route-table/subnet path the fixture uses next).
Traces: eks.certificate-authority-lifecycle-transitions, eks.nodegroup-full-lifecycle-transitions.

Cause 3 of 5 — EC2 CreateSecurityGroup is a scaffold stub.
AWS: recorded behavior only — CreateSecurityGroup returns a GroupId that the fixture then attaches to the cluster's resourcesVpcConfig.
Emulator: 501 `NotImplemented` ("operation CreateSecurityGroup is scaffolded but not implemented in this emulator").
Why: cluster creation with an explicit security group — the normal EKS pattern — cannot be expressed.
Fix: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_security_group.rs.
Traces: eks.certificate-authority-data-base64-round-trip, eks.nodegroup-labels-empty-vs-absent-preserved.

Cause 4 of 5 — a CloudFormation stack containing AWS::EKS::Cluster reaches CREATE_COMPLETE but no cluster exists.
AWS: recorded behavior only — when a stack whose resources are AWS::EC2::VPC + two AWS::EC2::Subnet + AWS::IAM::Role + AWS::EKS::Cluster reaches CREATE_COMPLETE, the named cluster is independently describable through the EKS API (DescribeCluster status ACTIVE, arn ending in cluster/<name>, present in ListClusters).
Emulator: the stack reports CREATE_COMPLETE, then eks.describe_cluster(name=<the stack's cluster>) answers 404 ResourceNotFoundException "No cluster found for name: <name>." — the CFN resource is a no-op that reports success.
Why: this is a silent success: IaC-driven clients believe their cluster exists, and every later call fails on a resource CloudFormation said it created. Stack status becomes meaningless as a readiness signal.
Fix: the CloudFormation resource provisioning for AWS::EKS::Cluster — it must drive the EKS CreateCluster path (and the referenced AWS::EC2::VPC/Subnet resources) and fail the stack when it cannot.
Traces: eks.deploy-cfn-cluster-active-and-usable, eks.deploy-cfn-cluster-behaves.

Cause 5 of 5 — DescribeStacks returns stack Outputs entries without OutputValue.
AWS: recorded behavior only — each member of Stacks[].Outputs carries both OutputKey and OutputValue (the resolved value of the template's Outputs block).
Emulator: the Outputs entries lack the OutputValue member entirely, so building {OutputKey: OutputValue} raises KeyError: 'OutputValue'.
Why: a client cannot read anything out of a stack — every outputs-driven hand-off (here, the pod-identity association's ids) breaks, and the omission is of a member AWS always populates.
Fix: the CloudFormation DescribeStacks output serializer / stack-output resolution.
Traces: eks.deploy-cfn-podidentity-association-usable.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/306 passed.

Cause 1 (8 checks, identical): AssertionError: EKS needs subnets in >=2 supported AZs; the default VPC offers []  — raised from the fixtures' _fixture_subnets() after ec2.describe_subnets(Filters=[{'Name':'default-for-az','Values':['true']}]) returned []. The preceding assertion on AZs offering the node instance type passed, so DescribeAvailabilityZones/DescribeInstanceTypeOfferings answered.
Cause 2: eks.certificate-authority-lifecycle-transitions and eks.nodegroup-full-lifecycle-transitions — KeyError: 'subnet_ids' at `subnet_ids = _prep["subnet_ids"]`; _prep is filled by a thread running _create_vpc_infra(), whose create_vpc call raises botocore ClientError (NotImplemented) "operation CreateVpc is scaffolded but not implemented in this emulator" (the same stub the suite reports as blocking 217 checks).
Cause 3: eks.certificate-authority-data-base64-round-trip, eks.nodegroup-labels-empty-vs-absent-preserved — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSecurityGroup operation: operation CreateSecurityGroup is scaffolded but not implemented in this emulator.
Cause 4: eks.deploy-cfn-cluster-active-and-usable — after _deploy_cfn() returned (stack CREATE_COMPLETE): botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeCluster operation: No cluster found for name: sim2real-eks-cfncl-b56e68816d66. eks.deploy-cfn-cluster-behaves — same, after _wait_stack_status(..., 'CREATE_COMPLETE'): No cluster found for name: sim2real-eks-cfn-ec80351a76e5.
Cause 5: eks.deploy-cfn-podidentity-association-usable — KeyError: 'OutputValue' at `{o["OutputKey"]: o["OutputValue"] for o in cfn.describe_stacks(StackName=stack)["Stacks"][0]["Outputs"]}`.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: ec2.describe_subnets(Filters=[{'Name':'default-for-az','Values':['true']}]) -> expect one default subnet per AZ of the default VPC; observe an empty list. Then try eks.create_cluster with two of those subnet ids (impossible).
3. Cause 2: ec2.create_vpc(CidrBlock='10.81.0.0/16', TagSpecifications=[...]) -> expect a Vpc.VpcId; observe 501 NotImplemented.
4. Cause 3: ec2.create_security_group(GroupName=..., Description=..., VpcId=...) -> expect a GroupId; observe 501 NotImplemented.
5. Cause 4: cfn.create_stack with a template containing AWS::EC2::VPC, two AWS::EC2::Subnet, AWS::IAM::Role and AWS::EKS::Cluster (Name=<n>, RoleArn, ResourcesVpcConfig.SubnetIds), wait for CREATE_COMPLETE, then eks.describe_cluster(name=<n>) -> expect status ACTIVE; observe 404 ResourceNotFoundException.
6. Cause 5: for any stack whose template declares Outputs, cfn.describe_stacks(StackName=...)['Stacks'][0]['Outputs'] -> expect every entry to carry OutputKey AND OutputValue; observe OutputValue missing.
