Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-nat-gateway-private-connectivity-workflow
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-nat-gateway-private-connectivity-workflow ec2-nat-gateway-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateNatGateway|2xx->2xx|$.CreateNatGatewayResponse.natGateway.natGatewayAddressSet.item.status
Issue-Key: ec2|CreateNatGateway|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item.attachedApplianceSet
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# EC2 NAT gateway create and describe paths report wrong address status, wrong validation message and omit attachedApplianceSet

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNatGateway

## Description
This issue bundles 3 causes; all 3 must be closed. They are the three net-new wire divergences on the EC2 NAT gateway path in this replay: the create response reports a terminal address status at creation time, the create-time validation failure uses a message AWS does not use, and the describe projection drops a modelled list member. No open issue carries these replay keys.

Cause 1 of 3: CreateNatGateway reports natGatewayAddressSet.item.status as a terminal 'succeeded' at creation time.
AWS: Smithy: NatGateway.NatGatewayAddresses (xmlName natGatewayAddressSet) -> NatGatewayAddress.Status (xmlName status) targets the enum NatGatewayAddressStatus with values assigning, unassigning, associating, disassociating, succeeded, failed. Recorded behavior: the CreateNatGateway response carries status 'assigning' for the freshly created address - the address has not been assigned yet at the moment the create call returns; 'succeeded' is reached later, as the gateway's own state advances.
Emulator: the CreateNatGateway response carries status 'succeeded' immediately (expected 'assigning', got 'succeeded').
Why: a client that waits for the assigning -> succeeded transition before using the gateway sees a terminal state in the create response and never observes the transition, so it treats the address, the network interface and the private IP as live before anything has been assigned; conversely a client that asserts the create-time state is 'assigning' fails against the emulator.
Fix: the CreateNatGateway handler and the NAT gateway's state in the emulator - the initial NatGatewayAddress.Status must be 'assigning', and the gateway's state machine must advance it to 'succeeded' as it advances the gateway's own state out of 'pending'.
Traces: ec2-nat-gateway-private-connectivity-workflow#task#3.

Cause 2 of 3: CreateNatGateway rejects a public NAT gateway with no AllocationId using a generic missing-parameter message instead of AWS's connectivity-type message.
AWS: Smithy: CreateNatGateway input member AllocationId is not @required - it is conditionally required at runtime, required when ConnectivityType is public and not accepted for private. Recorded behavior: HTTP 400 whose Response.Errors.Error.Message is 'AllocationId is required for a NAT Gateway with Connectivity Type public. Please provide the AllocationId and retry the request.' The fixed text is that whole two-sentence template; its only variable part is the connectivity type word taken from the request's ConnectivityType ('public' here). The HTTP status (400) and the error code already match; only the message diverges.
Emulator: returns 400 with Response.Errors.Error.Message 'The request must contain the parameter allocationId' - the emulator's generic required-parameter message, which names neither the connectivity type nor the remedy.
Why: the message is part of the error contract. A client or operator surfacing it loses the reason the parameter is required (it is required only for public connectivity), and clients that match on AWS's wording to distinguish this conditional requirement from an ordinary missing parameter no longer match.
Fix: CreateNatGateway's input validation - the connectivity-type-conditional AllocationId check must raise the connectivity-type message template rather than falling through to the generic missing-parameter path.
Traces: ec2-nat-gateway-private-connectivity-workflow#task#4.

Cause 3 of 3: DescribeNatGateways omits the modelled attachedApplianceSet member.
AWS: Smithy: NatGateway.AttachedAppliances (xmlName attachedApplianceSet, ec2QueryName AttachedApplianceSet) targets the list NatGatewayAttachedApplianceList. Recorded behavior: for a NAT gateway with no attached proxy appliances the member is still present in the DescribeNatGateways projection, serialized as an empty element.
Emulator: the member is absent from natGatewaySet.item entirely (expected '', got None).
Why: an absent member and an empty list are not the same answer - a client reading attachedApplianceSet to decide whether traffic is being inspected cannot distinguish 'no appliances attached' from 'this field was not reported', and strict deserializers that expect the modelled member to be present fail on the emulator's body.
Fix: the DescribeNatGateways response serializer / NatGateway projection - emit attachedApplianceSet for every NAT gateway, empty when the gateway has no attached appliances.
Traces: ec2-nat-gateway-lifecycle#task#7 (same cluster: ec2-nat-gateway-private-connectivity-workflow#task#7).

## Evidence
Cause 1 (ec2-nat-gateway-private-connectivity-workflow#task#3, CreateNatGateway, 200 -> 200): $.CreateNatGatewayResponse.natGateway.natGatewayAddressSet.item.status value_mismatch - expected 'assigning', got 'succeeded'.
Cause 2 (ec2-nat-gateway-private-connectivity-workflow#task#4, CreateNatGateway, 400 -> 400): $.Response.Errors.Error.Message value_mismatch - expected 'AllocationId is required for a NAT Gateway with Connectivity Type public. Please provide the AllocationId and retry the request.', got 'The request must contain the parameter allocationId'. Fixed text: everything except the connectivity type word; variable part: 'public', taken from the request's ConnectivityType.
Cause 3 (ec2-nat-gateway-lifecycle#task#7, DescribeNatGateways, 200 -> 200, cluster of 2 steps - also ec2-nat-gateway-private-connectivity-workflow#task#7): $.DescribeNatGatewaysResponse.natGatewaySet.item.attachedApplianceSet missing - expected '' (present, empty), got None.

## Steps to Reproduce
1. Boot the emulator and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay ec2-nat-gateway-private-connectivity-workflow#task#3 and read natGateway.natGatewayAddressSet.item.status in the CreateNatGateway response - it must be 'assigning' at create time, not 'succeeded'.
3. Cause 2: replay ec2-nat-gateway-private-connectivity-workflow#task#4 - CreateNatGateway with ConnectivityType public and no AllocationId - and compare Response.Errors.Error.Message against AWS's connectivity-type template.
4. Cause 3: replay ec2-nat-gateway-lifecycle#task#7 (and ec2-nat-gateway-private-connectivity-workflow#task#7, the other member of that cluster) and check that natGatewaySet.item carries an attachedApplianceSet element even when no appliance is attached.
