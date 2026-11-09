Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__neptune-graph.deploy-neptune-graph-snapshot-and-private-endpoint-round-trip-and-delete
Issue-Key: cloudformation__ec2__neptune-graph|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation: AWS::NeptuneGraph::Graph and AWS::NeptuneGraph::PrivateGraphEndpoint are rejected as unrecognized resource types, so no Neptune Analytics private-endpoint stack can be deployed

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry does not know the AWS::NeptuneGraph::* resource types, so CreateStack refuses at template validation any stack that declares a Neptune Analytics graph together with a private graph endpoint. Nothing downstream of CreateStack is reachable: the graph is never provisioned, Ref/Fn::GetAtt on the endpoint are never evaluated, and the EC2 VPC endpoint the resource is supposed to mint never exists.

AWS: CloudFormation publishes AWS::NeptuneGraph::Graph and AWS::NeptuneGraph::PrivateGraphEndpoint as first-class resource types in every commercial region, so a template declaring them is accepted by CreateStack and provisioned through the Neptune Analytics control plane (Smithy, com-amazonaws-neptunegraph-amazonneptunegraph: CreateGraph POST /graphs -> 201, required graphName (@length 1..63, @pattern ^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$) and provisionedMemory (@range 16..24576), optional replicaCount (@range 0..2), deletionProtection, publicConnectivity; CreatePrivateGraphEndpoint POST /graphs/{graphIdentifier}/endpoints/ -> 201 returning vpcId, subnetIds, status, vpcEndpointId (@pattern ^vpce-[0-9a-f]{17}$)). The Graph resource's Ref is the graph id (^g-[a-z0-9]{10}$) and its GraphArn attribute is the graph ARN; the PrivateGraphEndpoint resource's Ref is the documented composite <GraphIdentifier>_<VpcId> and it exposes a VpcEndpointId attribute, with SecurityGroupIds write-only (observable only through EC2 DescribeVpcEndpoints). Removing the endpoint resource in an UpdateStack deletes only the endpoint and leaves the graph AVAILABLE; deleting the stack deletes the graph, after which neptune-graph GetGraph answers ResourceNotFoundException "No graph found for customer [<account>] with graph id [<graph id>]" (fixed text, with the account and the graph id taken from the request/state).

Emulator: CreateStack is refused with HTTP 400, error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::NeptuneGraph::Graph, AWS::NeptuneGraph::PrivateGraphEndpoint]". Fixed text of that message is "Template format error: Unrecognized resource types: [" ... "]"; the bracketed list is the set of type names taken from the submitted template. The message/code shape itself is what AWS emits for a genuinely unknown type — the defect is that these two types are classified as unknown at all.

Why: a client that provisions Neptune Analytics through IaC (CloudFormation templates, CDK, SAM) cannot create a stack at all: the call fails synchronously, so there is no stack, no stack id to poll, and no rollback to inspect. Any suite or pipeline that stands up its graph estate through a template cannot even reach the neptune-graph API surface, and a caller that treats ValidationError/"Template format error" as "my template is wrong" will go looking for a template bug that does not exist.

Fix: CloudFormation's template resource-type registry and the resource provisioners it dispatches to in the cloudformation service crate (the type table consulted during CreateStack template validation, and the per-type create/update/delete + Ref/GetAtt provisioner it routes to), plus the EC2 VPC-endpoint state the PrivateGraphEndpoint provisioner must materialize so DescribeVpcEndpoints reports the endpoint's VpcId, Groups and SubnetIds. Note: the open issue 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md reports the same class of gap for AWS::SSM::MaintenanceWindowTarget; it does not cover the NeptuneGraph types.

Traces: cloudformation__ec2__neptune-graph.deploy-neptune-graph-snapshot-and-private-endpoint-round-trip-and-delete (error at the very first CreateStack).

## Evidence
Check cloudformation__ec2__neptune-graph.deploy-neptune-graph-snapshot-and-private-endpoint-round-trip-and-delete

Expected (AWS): CreateStack accepts the template (Vpc, SubnetA, SubnetB, Sg, Graph of type AWS::NeptuneGraph::Graph, Pge of type AWS::NeptuneGraph::PrivateGraphEndpoint) and returns a StackId; DescribeStacks reaches CREATE_COMPLETE; Outputs carry GraphId (Ref Graph = g-<10 chars>), PgeId = "<graph id>_<vpc id>" and PgeVpcEndpointId (Fn::GetAtt Pge.VpcEndpointId = vpce-<17 hex>). neptune-graph ListGraphs/GetGraph then read the graph back as (name, provisionedMemory=16, replicaCount=0, publicConnectivity=false, deletionProtection=false, status=AVAILABLE), ListPrivateGraphEndpoints/GetPrivateGraphEndpoint report the endpoint AVAILABLE on both subnets, and EC2 DescribeVpcEndpoints([vpce id]) reports the same VpcId, the template's security group and both subnets.

Actual (emulator): the first CreateStack is refused —
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::NeptuneGraph::Graph, AWS::NeptuneGraph::PrivateGraphEndpoint]
No stack is created, so every later assertion in the check is unreachable.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client("cloudformation"); call CreateStack with StackName="sim2real-ngpge-<unique>" and a TemplateBody of:
   {"AWSTemplateFormatVersion":"2010-09-09","Resources":{
     "Vpc":{"Type":"AWS::EC2::VPC","Properties":{...}},
     "SubnetA":{"Type":"AWS::EC2::Subnet",...},"SubnetB":{"Type":"AWS::EC2::Subnet",...},
     "Sg":{"Type":"AWS::EC2::SecurityGroup",...},
     "Graph":{"Type":"AWS::NeptuneGraph::Graph","Properties":{"GraphName":"<name>","ProvisionedMemory":16,"ReplicaCount":0,"DeletionProtection":false,"PublicConnectivity":false}},
     "Pge":{"Type":"AWS::NeptuneGraph::PrivateGraphEndpoint","Properties":{"GraphIdentifier":{"Ref":"Graph"},"VpcId":{"Ref":"Vpc"},"SubnetIds":[{"Ref":"SubnetA"},{"Ref":"SubnetB"}],"SecurityGroupIds":[{"Ref":"Sg"}]}}},
    "Outputs":{"GraphId":{"Value":{"Ref":"Graph"}},"PgeId":{"Value":{"Ref":"Pge"}},"PgeVpcEndpointId":{"Value":{"Fn::GetAtt":["Pge","VpcEndpointId"]}}}}
3. Observe the call raise ValidationError "Template format error: Unrecognized resource types: [AWS::NeptuneGraph::Graph, AWS::NeptuneGraph::PrivateGraphEndpoint]" instead of returning a StackId.
