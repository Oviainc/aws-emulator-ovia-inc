Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__eks__emr-containers.deploy-cfn-virtualcluster-invalid-eks-cluster-rejected
Issue-Key: cloudformation__eks__emr-containers|CreateStack|ValidationError
Issue-Commit: d115796fe5713602f1adc3e09d553e6de30aa6e5

# cloudformation__eks__emr-containers: AWS::EMRContainers::VirtualCluster is an unregistered resource type, so CreateStack refuses the template with ValidationError instead of provisioning (and failing) the virtual cluster

> _Note: this issue was identified against the code base at commit `d115796fe5713602f1adc3e09d553e6de30aa6e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the emulator's CloudFormation template parser does not know the resource type AWS::EMRContainers::VirtualCluster, so a stack containing one is rejected outright at CreateStack time.
AWS: recorded behavior only (resource-type registration is not in the Smithy model). AWS CloudFormation supports AWS::EMRContainers::VirtualCluster: CreateStack accepts the template, the stack proceeds, and the EMR-on-EKS handler then refuses the resource because the referenced EKS cluster does not exist — the stack reaches a CREATE_FAILED/ROLLBACK_COMPLETE terminal state with the handler's reason, which is what the check grades. 'Template format error: Unrecognized resource types: [...]' (ValidationError, HTTP 400) is AWS's answer only for a type it genuinely does not support.
Emulator: CreateStack answers 400 ValidationError — "An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EMRContainers::VirtualCluster]" — so the stack is never created and the invalid-EKS-cluster refusal behind it is unreachable.
Why: a client deploying an EMR-on-EKS virtual cluster by template cannot create the stack at all; worse, a genuine provisioning failure is reported as a template-format error, so a caller cannot tell an unsupported type from a bad parameter and no rollback path can be exercised.
Fix: CloudFormation's resource-type registry/template validation must register AWS::EMRContainers::VirtualCluster and route it to an emr-containers CreateVirtualCluster/DeleteVirtualCluster resource handler that reports the service's refusal as the resource's failure reason.
Traces: cloudformation__eks__emr-containers.deploy-cfn-virtualcluster-invalid-eks-cluster-rejected. Same shape as the already-open 20261109T115129Z-cloudformation-ec2-neptune-graph-... and 20261109T123922Z-acm-cloudformation-ec2-aws-ec2-clientvpnendpoint-... unregistered-type issues; neither covers this type or this check.

## Evidence
Check cloudformation__eks__emr-containers.deploy-cfn-virtualcluster-invalid-eks-cluster-rejected:
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EMRContainers::VirtualCluster]
Expected: CreateStack returns 200 with a StackId; the stack then reaches a terminal failure state whose resource-status reason is the EMR-containers handler's refusal for a nonexistent EKS cluster.
Actual: HTTP 400, error code ValidationError, message 'Template format error: Unrecognized resource types: [AWS::EMRContainers::VirtualCluster]' — fixed text 'Template format error: Unrecognized resource types: [' + the type names from the template + ']'.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. Replay cloudformation__eks__emr-containers.deploy-cfn-virtualcluster-invalid-eks-cluster-rejected: call CreateStack with a template whose only resource is of type AWS::EMRContainers::VirtualCluster, with ContainerProvider pointing at an EKS cluster name that does not exist.
3. Observe the immediate 400 ValidationError on CreateStack instead of a created stack that later fails on the resource.
