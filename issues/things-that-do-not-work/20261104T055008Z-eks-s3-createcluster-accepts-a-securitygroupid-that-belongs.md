Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__s3.mbes-cluster-with-a-security-group-from-another-vpc-rejected
Issue-Key: eks__s3.mbes-cluster-with-a-security-group-from-another-vpc-rejected|_refusal#13|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# eks__s3: CreateCluster accepts a securityGroupId that belongs to another VPC

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: EKS CreateCluster does not cross-check resourcesVpcConfig.securityGroupIds against resourcesVpcConfig's VPC.
AWS: Smithy (eks, awswesleyfrontend) models CreateCluster (POST /clusters) with required resourcesVpcConfig and the refusals InvalidParameterException (400), ClientException (400), ResourceInUseException, ResourceLimitExceededException, UnsupportedAvailabilityZoneException. A securityGroupIds entry whose security group lives in a different VPC than the subnets is refused with InvalidParameterException (HTTP 400); the message's variable parts are the offending security-group id and the VPC ids taken from the request and from EC2 state.
Emulator: the call is ACCEPTED and a cluster is created, so the check's refusal assertion fails with 'the call was expected to be refused, but it succeeded'.
Why: a caller gets a cluster whose control-plane security group cannot apply to its network, and the configuration error is never surfaced at the API boundary where AWS reports it.
Fix: the CreateCluster handler's resourcesVpcConfig validation in the eks service crate (resolve each securityGroupId and compare its VpcId to the subnets' VPC).
Traces: eks__s3.mbes-cluster-with-a-security-group-from-another-vpc-rejected

## Evidence
Replay detail: 'AssertionError: the call was expected to be refused, but it succeeded'. Expected: HTTP 400 InvalidParameterException naming the security group that is not in the cluster's VPC. Actual: 200 with a created cluster.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite. 2. Replay eks__s3.mbes-cluster-with-a-security-group-from-another-vpc-rejected. 3. Directly: create two VPCs, take a security group from VPC B, and call eks CreateCluster with subnets from VPC A and that security group in resourcesVpcConfig.securityGroupIds.
