Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elasticache.e2e-tgwc-ecs-elasticache-transit-gateway-connectivity-lifecycle
Issue-Key: ec2__ecs__elasticache|CreateStack|ValidationError
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# CloudFormation rejects AWS::EC2::TransitGatewayRouteTableAssociation and AWS::EC2::TransitGatewayRouteTablePropagation as unrecognized resource types

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1 — the CloudFormation resource-type registry is missing the two transit-gateway route-table binding types, so a template that wires a TGW attachment into a route table cannot be deployed at all.
AWS: recorded behavior only (CloudFormation's resource-type catalogue is not in the Smithy model). `AWS::EC2::TransitGatewayRouteTableAssociation` and `AWS::EC2::TransitGatewayRouteTablePropagation` are supported CloudFormation resource types; a template declaring them passes template validation and CreateStack proceeds, creating the association/propagation through the EC2 AssociateTransitGatewayRouteTable and EnableTransitGatewayRouteTablePropagation paths. Both EC2 operations exist in this emulator's EC2 surface, so only the CFN type registry is missing them.
Emulator: CreateStack is refused with error code `ValidationError` and the message `Template format error: Unrecognized resource types: [AWS::EC2::TransitGatewayRouteTableAssociation, AWS::EC2::TransitGatewayRouteTablePropagation]` — the fixed text is `Template format error: Unrecognized resource types: [` ... `]`, the bracketed list being the types taken from the template. The check fails in its first phase (`[phase 1/4 deploy] ClientError: ... CreateStack ...`), so nothing downstream of the deploy is exercised.
Why: any stack that attaches a VPC to a transit gateway and binds the attachment to a route table — the standard hub-and-spoke template — fails at template validation. A client gets a template-format error for a template real CloudFormation accepts, and the entire multi-service scenario behind it is unreachable.
Fix: the CloudFormation service's resource-type registry and the resource handlers it dispatches to: register AWS::EC2::TransitGatewayRouteTableAssociation and AWS::EC2::TransitGatewayRouteTablePropagation and map their Create/Delete to the existing EC2 associate/propagate operations.
Traces: ec2__ecs__elasticache.e2e-tgwc-ecs-elasticache-transit-gateway-connectivity-lifecycle.

## Evidence
Expected: CreateStack accepts the template and the stack reaches CREATE_COMPLETE, with the TGW attachment associated to and propagating into the route table.
Actual: _PhaseError: [phase 1/4 deploy] ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EC2::TransitGatewayRouteTableAssociation, AWS::EC2::TransitGatewayRouteTablePropagation]

## Steps to Reproduce
Replay ec2__ecs__elasticache.e2e-tgwc-ecs-elasticache-transit-gateway-connectivity-lifecycle against the emulator. Equivalently: call cloudformation CreateStack with a template whose Resources include an AWS::EC2::TransitGatewayRouteTableAssociation (TransitGatewayRouteTableId + TransitGatewayAttachmentId) and an AWS::EC2::TransitGatewayRouteTablePropagation; the call is refused with ValidationError / "Template format error: Unrecognized resource types: [...]" instead of creating the stack.
