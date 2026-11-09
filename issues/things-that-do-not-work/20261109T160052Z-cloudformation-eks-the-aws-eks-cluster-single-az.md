Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__eks.deploy-cfn-cluster-single-az-subnets-rejected
Issue-Key: cloudformation__eks.deploy-cfn-cluster-single-az-subnets-rejected|run#26|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: d115796fe5713602f1adc3e09d553e6de30aa6e5

# cloudformation__eks: the AWS::EKS::Cluster single-AZ subnet refusal surfaces an emulator-authored handler message instead of EKS's "Subnets specified must be in at least two different AZs" wrapped SDK sentence

> _Note: this issue was identified against the code base at commit `d115796fe5713602f1adc3e09d553e6de30aa6e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: a CloudFormation stack whose AWS::EKS::Cluster names subnets that do not span two Availability Zones does roll back, but the resource-status reason carries the emulator's own validation sentence rather than the EKS refusal the CloudFormation EKS handler propagates.
AWS: Smithy: CreateCluster models InvalidParameterException/InvalidRequestException (HTTP 400) for resourcesVpcConfig; the CloudFormation handler re-reports the service refusal verbatim inside 'Resource handler returned message: "<service message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)'. Recorded behavior: HandlerErrorCode InvalidRequest and the message 'Subnets specified must be in at least two different AZs (Service: Eks, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)'. Fixed text: "Subnets specified must be in at least two different AZs" plus the SDK wrapper "(Service: Eks, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)"; the only variable parts are the per-request Request ID and the attempt count.
Emulator: the same HandlerErrorCode (InvalidRequest) but the message is 'resourcesVpcConfig.subnetIds must specify at least 2 subnets, got 1' — an emulator-authored sentence that names a member path and a count from the request, and carries none of the (Service: Eks, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...) wrapper the EKS resource handler adds.
Why: a client or operator diagnosing a rolled-back stack from DescribeStackEvents/DescribeStackResources sees neither AWS's reason (the subnets are not in two different AZs — not merely "fewer than 2 subnets") nor the service/status/request-id wrapper that identifies which downstream service refused; template-lint and rollback-analysis tooling that matches on that text fails.
Fix: the AWS::EKS::Cluster resource handler's propagation of the EKS CreateCluster refusal (the eks CreateCluster resourcesVpcConfig validation message plus the CloudFormation handler's SDK-exception wrapper).
Traces: cloudformation__eks.deploy-cfn-cluster-single-az-subnets-rejected. Related open issues 20261104T105252Z-eks-9-net-new-root-causes-in-parameter.md and 20261104T090203Z-eks-createcluster-refuses-with-generic-framework-wording-instead.md cover the eks-heading direct-API checks only; neither carries this CloudFormation check's replay key.

## Evidence
Check cloudformation__eks.deploy-cfn-cluster-single-az-subnets-rejected:
AssertionError: handler reason ('resourcesVpcConfig.subnetIds must specify at least 2 subnets, got 1', 'InvalidRequest'), expected ('Subnets specified must be in at least two different AZs (Service: Eks, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')
Expected: HandlerErrorCode InvalidRequest with message fixed text 'Subnets specified must be in at least two different AZs' followed by the SDK wrapper '(Service: Eks, Status Code: 400, Request ID: <per-request id>) (SDK Attempt Count: <per-request n>)'.
Actual: HandlerErrorCode InvalidRequest with message 'resourcesVpcConfig.subnetIds must specify at least 2 subnets, got 1' (the count is a request value; the member path and the sentence are the emulator's own).

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. Replay cloudformation__eks.deploy-cfn-cluster-single-az-subnets-rejected: CreateStack with an AWS::EKS::Cluster whose ResourcesVpcConfig.SubnetIds do not span two AZs, wait for the terminal (ROLLBACK_COMPLETE) status, then read the failing resource's ResourceStatusReason and parse it with 'Resource handler returned message: "(?P<message>.*)" (RequestToken: ..., HandlerErrorCode: (?P<code>...))'.
3. Compare the extracted message with AWS's sentence.
