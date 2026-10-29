Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.deploy-cfn-podidentity-association-usable eks.deploy-podidentity-association-cfn-enforced-at-runtime eks.deploy-addon-via-cloudformation-installed eks.deploy-cfn-nodegroup-joins-cluster
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateStack|ValidationError
Issue-Key: eks|DescribeAddon|ResourceNotFoundException
Issue-Key: eks|DescribeNodegroup|ResourceNotFoundException
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# eks: 3 net-new root causes in CloudFormation provisioning of AWS::EKS resources — PodIdentityAssociation type unrecognized, Addon and Nodegroup stacks reach CREATE_COMPLETE without creating the resource

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new root causes in how the emulator provisions AWS::EKS::* CloudFormation resources. All three must be closed. (The existing open issue 20261022T154613Z-eks-clusters-cannot-be-provisioned-at-all-no.md covers the AWS::EKS::Cluster leg of CFN deploys; none of the three causes below is that one, and none of these four checks is marked ALREADY FILED or OWNED.)

Cause 1 of 3 — AWS::EKS::PodIdentityAssociation is not a known resource type, so the stack is rejected at CreateStack.
AWS: recorded behavior only (CloudFormation resource coverage is not in the eks Smithy model): a template whose Resources contain Type: AWS::EKS::PodIdentityAssociation with ClusterName / Namespace / ServiceAccount / RoleArn is ACCEPTED by CreateStack and provisions a pod identity association that is afterwards readable through eks DescribePodIdentityAssociation / ListPodIdentityAssociations; the association it creates is enforced at runtime like an API-created one.
Emulator: CreateStack is refused 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::EKS::PodIdentityAssociation]'.
Why: infrastructure-as-code flows for EKS pod identity cannot be exercised at all — the stack never deploys, so neither the association nor anything that depends on it can be tested.
Fix: register an AWS::EKS::PodIdentityAssociation resource provider in the CloudFormation resource-type registry and wire its create/read/delete to the eks CreatePodIdentityAssociation / DescribePodIdentityAssociation / DeletePodIdentityAssociation handlers.
Traces: eks.deploy-cfn-podidentity-association-usable, eks.deploy-podidentity-association-cfn-enforced-at-runtime.

Cause 2 of 3 — an AWS::EKS::Addon stack reports CREATE_COMPLETE but no add-on exists.
AWS: Smithy: DescribeAddon returns the addon with status and a resolved addonVersion, and models ResourceNotFoundException (404) only when the add-on is absent. Recorded behavior: a stack containing Type: AWS::EKS::Addon with ClusterName/AddonName ('vpc-cni')/ResolveConflicts reaches CREATE_COMPLETE and the add-on is then ACTIVE with a non-empty addonVersion via DescribeAddon and present in ListAddons.
Emulator: the stack create completes, but DescribeAddon(clusterName=<cluster>, addonName='vpc-cni') answers ResourceNotFoundException 'The requested resource does not exist.' — the stack succeeded without creating the resource it declares.
Why: a CREATE_COMPLETE that provisions nothing is the most misleading failure mode available: a client's deploy pipeline reports success and every later EKS call against the declared add-on 404s.
Fix: the AWS::EKS::Addon CloudFormation resource provider — it must call the eks CreateAddon path and fail the stack if the add-on does not reach ACTIVE.
Traces: eks.deploy-addon-via-cloudformation-installed.

Cause 3 of 3 — an AWS::EKS::Nodegroup stack likewise completes without creating the nodegroup.
AWS: recorded behavior: a stack declaring Type: AWS::EKS::Nodegroup (with its VPC/subnets/roles in the same template) provisions a managed node group that reaches ACTIVE and is visible via eks DescribeNodegroup and ListNodegroups under its parent cluster.
Emulator: DescribeNodegroup after the stack answers ResourceNotFoundException 'The requested resource does not exist.'.
Why: same as cause 2 — the stack's declared nodegroup is absent while CloudFormation claims success, so node-group deploy flows cannot be emulated and failures surface far from their cause.
Fix: the AWS::EKS::Nodegroup CloudFormation resource provider, wired to the eks CreateNodegroup handler and its state (note CreateNodegroup itself is still a scaffold stub per cause 5 of 20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md; this cause additionally requires CFN to propagate that failure into the stack status instead of reporting CREATE_COMPLETE).
Traces: eks.deploy-cfn-nodegroup-joins-cluster.

## Evidence
Suite simparity-behavioral-aws-v1: 64/310 passed; eks 60/282.

Cause 1: eks.deploy-cfn-podidentity-association-usable — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EKS::PodIdentityAssociation]. eks.deploy-podidentity-association-cfn-enforced-at-runtime — identical ValidationError on CreateStack.
Cause 2: eks.deploy-addon-via-cloudformation-installed — botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeAddon operation: The requested resource does not exist. (raised after the stack_create_complete waiter returned for a stack whose only resource is AWS::EKS::Addon {ClusterName, AddonName: vpc-cni, ResolveConflicts: OVERWRITE}).
Cause 3: eks.deploy-cfn-nodegroup-joins-cluster — botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeNodegroup operation: The requested resource does not exist.

## Steps to Reproduce
1. Boot the emulator; point boto3 cloudformation and eks clients at it.
2. Cause 1: create_stack with a template whose Resources contain {Type: AWS::EKS::PodIdentityAssociation, Properties: {ClusterName, Namespace: default, ServiceAccount, RoleArn}} -> expect the stack to be accepted and the association to be readable via describe_pod_identity_association; observe CreateStack ValidationError 'Unrecognized resource types'.
3. Cause 2: create an ACTIVE cluster, then create_stack with {Addon: {Type: AWS::EKS::Addon, Properties: {ClusterName, AddonName: vpc-cni, ResolveConflicts: OVERWRITE}}}, wait for CREATE_COMPLETE, then describe_addon(clusterName, addonName='vpc-cni') -> expect status ACTIVE and a non-empty addonVersion, and the name in list_addons.
4. Cause 3: create_stack with a template declaring VPC/subnets/roles plus {Type: AWS::EKS::Nodegroup}, wait for CREATE_COMPLETE, then describe_nodegroup / list_nodegroups -> expect the nodegroup ACTIVE under its cluster. If the underlying create cannot succeed, the stack must NOT report CREATE_COMPLETE.
