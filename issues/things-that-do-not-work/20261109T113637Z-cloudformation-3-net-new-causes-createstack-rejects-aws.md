Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__networkmanager.deploy-customer-gateway-association-round-trip-and-delete cloudformation__networkmanager.deploy-networkmanager-core-resources-round-trip-and-delete cloudformation__networkmanager.deploy-networkmanager-transit-gateway-registration-round-trip-and-delete
Issue-Key: cloudformation__ec2__networkmanager|CreateStack|ValidationError
Issue-Key: cloudformation__networkmanager|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation: 3 net-new causes — CreateStack rejects AWS::NetworkManager::TransitGatewayRegistration, ::LinkAssociation and ::Connection as "Unrecognized resource types", so no NetworkManager estate can be deployed by template

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
CloudFormation's template-validation resource-type registry knows only part of the AWS::NetworkManager:: namespace. Three public registry resource types are missing, and because the refusal happens in CreateStack template validation, the stack is never created at all — every check that deploys a NetworkManager estate as infrastructure-as-code dies on its first call. The emulator already accepts AWS::NetworkManager::GlobalNetwork, ::Site, ::Device and ::Link (those names are absent from the refusal lists), so this is a gap in a partially-populated namespace, not a wholly absent one. Each cause below is one missing resource type; all three must be recognised AND provisioned (validation that merely stops refusing the type, without a provisioner that calls the underlying networkmanager operation and publishes the type's Ref/GetAtt values, converts a CreateStack refusal into a stack that never reaches CREATE_COMPLETE).

Cause 1 of 3: AWS::NetworkManager::TransitGatewayRegistration is not a recognized resource type.
AWS: CloudFormation accepts AWS::NetworkManager::TransitGatewayRegistration (a public registry type) and provisions it by calling networkmanager RegisterTransitGateway, whose model exists in this repo — Smithy: RegisterTransitGateway, POST /global-networks/{GlobalNetworkId}/transit-gateway-registrations, required input members GlobalNetworkId and TransitGatewayArn, output TransitGatewayRegistration; the registration is then readable through GetTransitGatewayRegistrations (paginated on TransitGatewayRegistrations) with State.Code AVAILABLE, and removed on stack delete via DeregisterTransitGateway. CloudFormation's "Template format error: Unrecognized resource types: [...]" ValidationError is reserved for types that genuinely do not exist in the registry; this type does exist, so AWS never answers that for it.
Emulator: CreateStack is refused 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::NetworkManager::TransitGatewayRegistration]". Fixed text of that message is "Template format error: Unrecognized resource types: [" + the comma-separated type names from the template + "]"; the bracketed names are values taken from the request's template. No stack id is minted, so the check's whole round trip (register, read back through GetTransitGatewayRegistrations, delete) is unreachable.
Why: a client deploying a Cloud WAN / global-network estate by template — the normal way these resources are created — cannot create the stack at all. There is no workaround inside the template: the deploy fails before any resource is provisioned, so CDK/CFN-based test and provisioning flows for NetworkManager are entirely blocked, and a template that AWS deploys is reported malformed.
Fix: the CloudFormation crate's template-validation resource-type registry (the table of supported AWS::* types consulted by CreateStack/ValidateTemplate), plus a resource provisioner for this type that calls networkmanager RegisterTransitGateway on create and DeregisterTransitGateway on delete and exposes Ref.
Traces: cloudformation__networkmanager.deploy-networkmanager-transit-gateway-registration-round-trip-and-delete, cloudformation__ec2__networkmanager.deploy-customer-gateway-association-round-trip-and-delete (the latter's template pairs the registration with AWS::EC2::TransitGateway, AWS::EC2::CustomerGateway, AWS::EC2::VPNConnection and AWS::NetworkManager::CustomerGatewayAssociation — Hypothesis: AWS::NetworkManager::CustomerGatewayAssociation, which that template only adds in its second phase, is likely missing from the same registry; it was never reached because validation stopped at the first phase, so recognise it together with this type).

Cause 2 of 3: AWS::NetworkManager::LinkAssociation is not a recognized resource type.
AWS: CloudFormation accepts AWS::NetworkManager::LinkAssociation and provisions it by calling networkmanager AssociateLink — Smithy: AssociateLink, POST /global-networks/{GlobalNetworkId}/link-associations, required input members GlobalNetworkId, DeviceId and LinkId, output LinkAssociation; the association is read back through GetLinkAssociations and removed with DisassociateLink on stack delete.
Emulator: CreateStack is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::NetworkManager::LinkAssociation, AWS::NetworkManager::Connection]" — the type name is listed as unknown even though the GlobalNetwork/Site/Device/Link resources in the same template are accepted.
Why: a template can declare the global network, its sites, devices and links, but cannot wire a device to a link, so the deployed topology is incomplete and the stack cannot be created at all. Clients see a valid AWS template reported as a template format error.
Fix: the CloudFormation template-validation resource-type registry, plus a provisioner calling AssociateLink/DisassociateLink and publishing the association's Ref/GetAtt values.
Traces: cloudformation__networkmanager.deploy-networkmanager-core-resources-round-trip-and-delete.

Cause 3 of 3: AWS::NetworkManager::Connection is not a recognized resource type.
AWS: CloudFormation accepts AWS::NetworkManager::Connection and provisions it by calling networkmanager CreateConnection — Smithy: CreateConnection, POST /global-networks/{GlobalNetworkId}/connections, required input members GlobalNetworkId, DeviceId and ConnectedDeviceId, optional LinkId, ConnectedLinkId, Description (max 256) and Tags, output Connection; it is read back through GetConnections and removed with DeleteConnection on stack delete.
Emulator: the same CreateStack refusal, 400 ValidationError, lists AWS::NetworkManager::Connection among the unrecognized types.
Why: device-to-device connections — the point of a core NetworkManager template — cannot be declared, and because the refusal is at template-validation time it also takes down the rest of an otherwise-supported template.
Fix: the CloudFormation template-validation resource-type registry, plus a provisioner calling CreateConnection/DeleteConnection (and UpdateConnection for updates) and exposing ConnectionId/ConnectionArn through Ref/GetAtt.
Traces: cloudformation__networkmanager.deploy-networkmanager-core-resources-round-trip-and-delete.

Note on scope: the three checks are listed by the suite under the composite headings `cloudformation__networkmanager` and `cloudformation__ec2__networkmanager`; the failing call in every one of them is CloudFormation's CreateStack, so this issue is filed against cloudformation. No open issue covers these types — the nearest is 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md, which reports the same class of gap for AWS::SSM::MaintenanceWindowTarget and does not cover the NetworkManager namespace.

## Evidence
Check cloudformation__networkmanager.deploy-networkmanager-transit-gateway-registration-round-trip-and-delete
  Template Resources: GlobalNetwork (AWS::NetworkManager::GlobalNetwork), TransitGateway (AWS::EC2::TransitGateway), Registration (AWS::NetworkManager::TransitGatewayRegistration with GlobalNetworkId={Ref: GlobalNetwork} and TransitGatewayArn).
  Expected: CreateStack accepted, StackId returned, stack reaches CREATE_COMPLETE, GetTransitGatewayRegistrations lists the TGW arn with State.Code=AVAILABLE, DeleteStack deregisters it.
  Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::NetworkManager::TransitGatewayRegistration]

Check cloudformation__ec2__networkmanager.deploy-customer-gateway-association-round-trip-and-delete
  Template Resources: GlobalNetwork, Tgw (AWS::EC2::TransitGateway), TgwRegistration (AWS::NetworkManager::TransitGatewayRegistration), Cgw (AWS::EC2::CustomerGateway), VpnConn (AWS::EC2::VPNConnection), Device (AWS::NetworkManager::Device), and in the second phase CgwAssoc (AWS::NetworkManager::CustomerGatewayAssociation).
  Expected: CreateStack accepted; the registration and later the customer-gateway association round-trip and are torn down by DeleteStack.
  Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::NetworkManager::TransitGatewayRegistration]

Check cloudformation__networkmanager.deploy-networkmanager-core-resources-round-trip-and-delete
  Template Resources: GlobalNetwork, Site, DeviceA, DeviceB, Link (all accepted), LinkAssociation (AWS::NetworkManager::LinkAssociation over DeviceA + Link), Connection (AWS::NetworkManager::Connection over DeviceA + DeviceB).
  Expected: CreateStack accepted, stack reaches CREATE_COMPLETE, the link association and connection are readable through GetLinkAssociations/GetConnections, and DeleteStack removes the children before the global network.
  Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::NetworkManager::LinkAssociation, AWS::NetworkManager::Connection]

In all three the status/code/message triple is 400 / ValidationError / "Template format error: Unrecognized resource types: [<types from the submitted template>]"; the fixed text is "Template format error: Unrecognized resource types: [" ... "]" and the bracketed type names are values from the request.

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudformation client at it.
2. CreateStack with a template whose Resources declare an AWS::NetworkManager::GlobalNetwork plus an AWS::NetworkManager::TransitGatewayRegistration (GlobalNetworkId={"Ref": "GlobalNetwork"}, TransitGatewayArn=<any tgw arn>). The call is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::NetworkManager::TransitGatewayRegistration]" instead of returning a StackId.
3. CreateStack with a template declaring GlobalNetwork, Site, DeviceA, DeviceB, Link, then an AWS::NetworkManager::LinkAssociation (GlobalNetworkId/DeviceId/LinkId) and an AWS::NetworkManager::Connection (GlobalNetworkId/DeviceId/ConnectedDeviceId). The call is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::NetworkManager::LinkAssociation, AWS::NetworkManager::Connection]" — note the sibling GlobalNetwork/Site/Device/Link types in the same template are accepted.
4. Replay the three checks named in trace_ids.
