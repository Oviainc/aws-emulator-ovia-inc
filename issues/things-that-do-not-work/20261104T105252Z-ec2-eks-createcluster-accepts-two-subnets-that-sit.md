Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__eks.ekscc-create-cluster-with-two-subnets-in-one-az-rejected
Issue-Key: ec2__eks.ekscc-create-cluster-with-two-subnets-in-one-az-rejected|_error_code#7|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# ec2__eks: CreateCluster accepts two subnets that sit in the same Availability Zone

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: CreateCluster does not resolve its subnets' Availability Zones, so two distinct subnets of one VPC that share an AZ are accepted.
  AWS: an EKS control plane requires subnets in at least two different AZs. EKS reads each subnet's AZ from EC2 (DescribeSubnets) rather than counting subnet ids, so two subnets of the same VPC in the SAME AZ are refused InvalidParameterException 400 with 'Subnets specified must be in at least two different AZs' — the whole sentence is fixed text — and no cluster is created. The identical request with the two subnets in two AZs is accepted (the check's positive control), which is what makes the AZ resolution, not the subnet count, the requirement. Smithy: CreateCluster.resourcesVpcConfig is required; CreateCluster's client errors are ClientException 400, InvalidParameterException 400, ResourceInUseException 409, ResourceLimitExceededException 400 and UnsupportedAvailabilityZoneException 400.
  Emulator: the call SUCCEEDS (the check's error helper reports 'the call SUCCEEDED; real AWS rejects it, so there is no error code'), creating a cluster whose two subnets share one AZ.
  Why: a cluster with a single-AZ control plane cannot exist in AWS. A caller that validates its VPC layout against the emulator ships a template that AWS refuses, and the emulator reports a highly-available control plane where there is none.
  Fix: CreateCluster's resourcesVpcConfig validation — resolve each subnet id through EC2 state and require at least two distinct Availability Zones before accepting the create.
  Traces: ec2__eks.ekscc-create-cluster-with-two-subnets-in-one-az-rejected. The same missing AZ resolution shows up under the eks heading (single subnet, and the same subnet id twice) and is filed there; this check is the cross-service one that proves the AZ, not the count, is read from EC2.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout; service ec2__eks 2/5 passed.

ec2__eks.ekscc-create-cluster-with-two-subnets-in-one-az-rejected [mismatch]:
  check_ec2__eks_ekscc_create_cluster_with_two_subnets_in_one_az_rejected.py line 30 -> got = _error_code(lambda: _cluster(ctx, eks, bad, role_arn, [a1, a2]))
  _error_code line 113 -> AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code
The check builds its own VPC with subnets laid out [az[0], az[0], az[1]] and passes the two same-AZ subnets (a1, a2) to CreateCluster; the sibling expectation in this suite's eks checks states the refusal message as 'Subnets specified must be in at least two different AZs' (InvalidParameterException).

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateCluster POST /clusters, resourcesVpcConfig -> VpcConfigRequest required; errors ClientException 400, InvalidParameterException 400, ResourceInUseException 409, ResourceLimitExceededException 400, UnsupportedAvailabilityZoneException 400, ServerException 500, ServiceUnavailableException 503.

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run ec2__eks.ekscc-create-cluster-with-two-subnets-in-one-az-rejected.
3. Manually: with EC2, create a VPC and three subnets laid out as [azA, azA, azB]. Call eks CreateCluster with the two azA subnets -> expect 400 InvalidParameterException 'Subnets specified must be in at least two different AZs' and no cluster (ListClusters must not contain the name). Then call CreateCluster with one azA and the azB subnet -> expect success (status CREATING).
