Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-transitgateway-and-vpcendpoint-connection-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-transitgateway-and-vpcendpoint-connection-errors ec2-allocate-address-vpc-and-unowned-address-error ec2-allocate-hosts-error ec2-dedicated-host-lifecycle ec2-assign-ipv6-addresses-not-found-and-malformed-errors ec2-error-paths-invalid-attachment-parents ec2-image-ancestry-malformed-id-rejected ec2-local-gateway-error-handling ec2-ipam-byoasn-and-trunk-interface-association-errors ec2-ipam-transit-gateway-and-instance-association-error-paths
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AcceptTransitGatewayPeeringAttachment|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AcceptTransitGatewayVpcAttachment|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AcceptVpcEndpointConnections|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AllocateAddress|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AllocateHosts|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AssignIpv6Addresses|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateTransitGatewayConnectPeer|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateLocalGatewayRoute|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AssociateIpamByoasn|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261029T200817Z-ec2-id-validation-verdict-is-inverted-malformed-ids.md
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 not-found and malformed refusals emit generic emulator prose instead of AWS's per-resource message

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptTransitGatewayPeeringAttachment

## Description
Ten net-new causes, one defect: the status and the error code are right but the MESSAGE is emulator-authored text rather than AWS's wording for that refusal class. AWS's message is part of the contract; each has fixed text plus variable parts taken from the request or state (ids, addresses, CIDRs). ec2 models no error shapes in Smithy, so each expectation below is recorded behavior only, read off the recorded 400.

Cause 1 of 10 - AcceptTransitGatewayPeeringAttachment, unknown attachment (ec2-accept-transitgateway-and-vpcendpoint-connection-errors, task#3). AWS: "Transit Gateway Attachment <tgw-attach-id> was deleted or does not exist." - fixed prefix 'Transit Gateway Attachment', the id unquoted, fixed suffix 'was deleted or does not exist.' Emulator: "The transit-gateway-attachment ID '<id>' does not exist" (the emulator's generic id template). Fix: the refusal message in the operation's handler/validation.

Cause 2 of 10 - AcceptTransitGatewayVpcAttachment, same fixture step task#4: identical substitution of the generic template for AWS's 'Transit Gateway Attachment <id> was deleted or does not exist.'

Cause 3 of 10 - AcceptVpcEndpointConnections, unknown endpoint service (same fixture, task#5). AWS: "The VpcEndpointService Id '<vpce-svc-id>' does not exist" - note AWS's resource label 'VpcEndpointService Id'. Emulator: "The VPC endpoint service ID '<id>' does not exist".

Cause 4 of 10 - AllocateAddress with an Address the account does not own (ec2-allocate-address-vpc-and-unowned-address-error#task#3). AWS: "The Elastic IP address '<ip>' does not exist or is not available." Emulator: "Address '<ip>' not found".

Cause 5 of 10 - AllocateHosts given both InstanceType and InstanceFamily (ec2-allocate-hosts-error#task#1). AWS: "You cannot set both 'InstanceType' and 'InstanceFamily'." - entirely fixed text naming the two members. Emulator: 'You cannot specify InstanceFamily and InstanceType in the same request'. Smithy: AllocateHosts models InstanceType and InstanceFamily as independent optional members, so the mutual exclusion is enforced in the handler and its message is the only contract a client sees.

Cause 6 of 10 - AssignIpv6Addresses with a malformed ENI id (ec2-assign-ipv6-addresses-not-found-and-malformed-errors#task#2). AWS: "The interface ID '<eni-id>' is malformed". Emulator: 'Invalid id: "<eni-id>"' - a generic parser message that names no resource.

Cause 7 of 10 - CreateTransitGatewayConnectPeer with a malformed parent attachment id (ec2-error-paths-invalid-attachment-parents#task#3). AWS: 'Invalid Transit Gateway Attachment id.' - fixed text with NO value interpolated. Emulator: 'Invalid id: "<id>"'.

Cause 8 of 10 - GetImageAncestry with a malformed AMI id (ec2-image-ancestry-malformed-id-rejected#task#1). AWS: "The image ID '<ami-id>' is malformed". Emulator: 'Invalid id: "<ami-id>"'.

Cause 9 of 10 - CreateLocalGatewayRoute with a malformed route-table id (ec2-local-gateway-error-handling#task#4). AWS: 'Invalid LocalGateway Route Table id <lgwrtb-id>' - fixed prefix, id unquoted at the end. Emulator: "The local-gateway-route-table ID '<id>' is malformed".

Cause 10 of 10 - AssociateIpamByoasn for a CIDR that is not in IPAM (ec2-ipam-byoasn-and-trunk-interface-association-errors#task#1). AWS: 'The requested CIDR could not be found in IPAM.' - fixed text, NO CIDR interpolated. Emulator: 'The CIDR (<cidr>) could not be found in IPAM.'

Why: clients, test suites and operators match on these strings; several AWS messages deliberately omit the request value (causes 7 and 10) or use a resource label that differs from the emulator's generic one, so a generic template is never equivalent. Fix: the refusal-message construction in each named operation's handler/validation, not a shared generic id formatter. Related open issue 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md covers other refusal families and carries no replay key for these steps.

Traces: representative step plus the nine fixture#step bindings in trace_ids.

## Evidence
Replay detail, verbatim per cause (all HTTP 400 expected and returned, code matching, message diverging):
1. AcceptTransitGatewayPeeringAttachment: expected 'Transit Gateway Attachment tgw-attach-00000000000000003 was deleted or does not exist.'; actual "The transit-gateway-attachment ID 'tgw-attach-00000000000000003' does not exist".
2. AcceptTransitGatewayVpcAttachment: expected 'Transit Gateway Attachment tgw-attach-00000000000000004 was deleted or does not exist.'; actual "The transit-gateway-attachment ID 'tgw-attach-00000000000000004' does not exist".
3. AcceptVpcEndpointConnections: expected "The VpcEndpointService Id 'vpce-svc-00000000000000000' does not exist"; actual "The VPC endpoint service ID 'vpce-svc-00000000000000000' does not exist".
4. AllocateAddress: expected "The Elastic IP address '192.0.2.55' does not exist or is not available."; actual "Address '192.0.2.55' not found".
5. AllocateHosts: expected "You cannot set both 'InstanceType' and 'InstanceFamily'."; actual 'You cannot specify InstanceFamily and InstanceType in the same request' (cluster also covers ec2-dedicated-host-lifecycle#task#3).
6. AssignIpv6Addresses: expected "The interface ID 'not-an-eni-id' is malformed"; actual 'Invalid id: "not-an-eni-id"'.
7. CreateTransitGatewayConnectPeer: expected 'Invalid Transit Gateway Attachment id.'; actual 'Invalid id: "tgw-attach-0123456789ab"'.
8. GetImageAncestry: expected "The image ID 'not-a-valid-ami-id' is malformed"; actual 'Invalid id: "not-a-valid-ami-id"'.
9. CreateLocalGatewayRoute: expected 'Invalid LocalGateway Route Table id lgwrtb-00000000000000000'; actual "The local-gateway-route-table ID 'lgwrtb-00000000000000000' is malformed".
10. AssociateIpamByoasn: expected 'The requested CIDR could not be found in IPAM.'; actual 'The CIDR (203.0.113.0/24) could not be found in IPAM.' (cluster also covers ec2-ipam-transit-gateway-and-instance-association-error-paths#task#3).
All ten clusters: verdict=mismatch, stub_path empty, owned_by empty.

## Steps to Reproduce
1. Clone and boot the emulator built from the checkout.
2. replay_wire_corpus on simparity-wire-aws-v1.
3. For ec2-accept-transitgateway-and-vpcendpoint-connection-errors step task#3 and each fixture#step in trace_ids, diff $.Response.Errors.Error.Message against the recorded response.
