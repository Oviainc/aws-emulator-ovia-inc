Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-reservation-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-reservation-error-paths ec2-capacity-reservation-split-workflow ec2-coip-pool-mistaken-usage ec2-dhcp-options-lifecycle ec2-ebs-volume-delete-and-snapshot-not-found ec2-volume-snapshot-copy-lifecycle ec2-local-gateway-create-error-paths ec2-local-gateway-vif-group-local-gateway-not-found ec2-local-gateway-virtual-interface-group-route-table-association ec2-network-insights-access-scope-analysis-lifecycle ec2-vpce-traj-endpoint-lookup-mistakes ec2-error-paths-invalid-attachment-parents
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateCapacityReservationBySplitting|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateCapacityReservationBySplitting|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateCoipPool|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateCoipCidr|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteDhcpOptions|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteDhcpOptions|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateSnapshot|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateSnapshot|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateLocalGatewayVirtualInterfaceGroup|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteNetworkInsightsAccessScope|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteNetworkInsightsAccessScope|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteVpcEndpoints|2xx->2xx|$.DeleteVpcEndpointsResponse.unsuccessful.item.error.code
Issue-Key: ec2|DeleteVpcEndpoints|2xx->2xx|$.DeleteVpcEndpointsResponse.unsuccessful.item.error.message
Issue-Key: ec2|CreateTransitGatewayPeeringAttachment|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# EC2 invalid-identifier error paths return the wrong error code and message

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityReservationBySplitting

## Description
Shared fault: when an EC2 request names a resource identifier that is syntactically well formed but absent, or is rejected by a parameter rule, the emulator picks the wrong error class (malformed vs not-found vs InvalidParameterValue) and words the message with its own internal resource token instead of AWS's. Ten distinct causes, all ec2, all status-matched or status-diverging error bodies. The EC2 Smithy model declares no modeled errors for these operations (ec2Query wraps every failure in Response.Errors.Error with Code/Message), so the requirement for each is the recorded AWS error class and message template; the variable parts are the identifier from the request and the counts from emulator state.

Cause 1 of 10: CreateCapacityReservationBySplitting with an absent capacity-reservation id.
AWS: recorded behavior only (Smithy: CreateCapacityReservationBySplitting, errors=[] — ec2Query error envelope). 400, Code 'InvalidCapacityReservationId.NotFound', Message "The capacity-reservation ID '<id>' does not exist" — fixed text is "The capacity-reservation ID '" ... "' does not exist"; <id> is the SourceCapacityReservationId from the request.
Emulator: 400 with Code 'InvalidCapacityReservationId.Malformed' and Message "Capacity Reservation ID '<id>' is malformed". The id is a well-formed 17-hex-digit cr- id, so the malformed class is wrong.
Why: a client that branches on '...NotFound' to create the reservation it is missing instead sees a malformed-input error and treats its own id generation as broken.
Fix: the identifier-resolution/validation step of the CreateCapacityReservationBySplitting handler — syntax check must pass a well-formed id through to the existence check.
Traces: ec2-capacity-reservation-error-paths#task#3 (also ec2-capacity-reservation-split-workflow#task#2).

Cause 2 of 10: CreateCapacityReservationBySplitting when the source reservation has too little available capacity.
AWS: recorded behavior only. 400, Message "There isn't enough available capacity in the source Capacity Reservation. The current available instance count is <available>. You requested to split <requested>." — the two sentences are fixed text; <available> comes from the reservation in state and <requested> from InstanceCount.
Emulator: 400 with Message "Invalid value '<requested>' for instanceCount. The capacity reservation '<id>' has <available> instances available, and a split leaves at least one of them in it" — entirely different wording and it names the reservation id, which AWS's message does not.
Why: operators and tests that match on AWS's capacity sentence do not recognise the condition; the error reads like a malformed-parameter error rather than a capacity shortfall.
Fix: the capacity pre-check in the CreateCapacityReservationBySplitting handler's validation.
Traces: ec2-capacity-reservation-error-paths#task#2.

Cause 3 of 10: CreateCoipPool with a bad local-gateway-route-table id.
AWS: recorded behavior only. 400, Message "Invalid LocalGateway Route Table id <id>" — fixed text "Invalid LocalGateway Route Table id " followed by the raw id, unquoted.
Emulator: 400 with Message "The local-gateway-route-table ID '<id>' is malformed".
Why: the message is the only distinguishing signal here, and callers matching AWS's phrasing see an unknown error.
Fix: CreateCoipPool validation message template.
Traces: ec2-coip-pool-mistaken-usage#task#2.

Cause 4 of 10: CreateCoipCidr against a pool that does not exist.
AWS: recorded behavior only. 400, Code 'InvalidPoolID.NotFound'.
Emulator: 400 with Code 'InvalidParameterValue'.
Why: a not-found condition is reported as a malformed-input condition, so retry/create-if-missing logic keyed on 'InvalidPoolID.NotFound' never fires.
Fix: CreateCoipCidr handler's pool lookup — emit the resource-specific not-found code.
Traces: ec2-coip-pool-mistaken-usage#task#3.

Cause 5 of 10: DeleteDhcpOptions with a malformed dhcp-options id.
AWS: recorded behavior only. 400, Code 'InvalidDhcpOptionsId.Malformed', Message "The dhcp-options ID '<id>' is malformed" — note the hyphenated resource token 'dhcp-options'.
Emulator: 400 with Code 'InvalidDhcpOptionID.NotFound' and Message "The dhcpOption ID '<id>' does not exist" — wrong error class (existence instead of syntax), wrong code spelling, and a camel-cased resource token.
Why: syntax errors and existence errors are distinguished by clients to decide whether to retry with a different id or fix the caller; collapsing them hides input bugs.
Fix: DeleteDhcpOptions validation — run the id-syntax check before the state lookup, and use AWS's code/resource-token spelling.
Traces: ec2-dhcp-options-lifecycle#task#6.

Cause 6 of 10: CreateSnapshot naming a volume that does not exist.
AWS: recorded behavior only. 400, Code 'InvalidVolume.NotFound', Message "The volume '<volume-id>' does not exist." (trailing period is part of the fixed text).
Emulator: 400 with Code 'InvalidVolumeID.Malformed' and Message "Value ( <volume-id> ) for parameter VOLUME is invalid.".
Why: snapshot tooling that tolerates a deleted volume by matching 'InvalidVolume.NotFound' instead aborts on what looks like a client-side input defect.
Fix: CreateSnapshot handler's volume resolution and its error mapping.
Traces: ec2-ebs-volume-delete-and-snapshot-not-found#task#5 (also ec2-volume-snapshot-copy-lifecycle#task#1).

Cause 7 of 10: CreateLocalGatewayVirtualInterfaceGroup with an absent local gateway.
AWS: recorded behavior only. 400, Message "The local-gateway ID '<id>' does not exist" — hyphenated resource token 'local-gateway'.
Emulator: 400 with Message "The LocalGateway ID '<id>' does not exist" — camel-cased token.
Why: the code matches, but message assertions and log scrapers keyed to AWS's token mismatch.
Fix: the shared not-found message template for local-gateway ids used by this handler.
Traces: ec2-local-gateway-create-error-paths#task#3 (also ec2-local-gateway-vif-group-local-gateway-not-found#task#1, ec2-local-gateway-virtual-interface-group-route-table-association#task#2).

Cause 8 of 10: DeleteNetworkInsightsAccessScope with an absent scope id.
AWS: recorded behavior only. 400, Code 'InvalidParameterValue', Message "NetworkInsightsAccessScopeId (<id>) is not valid" — fixed text with the id in parentheses.
Emulator: 400 with Code 'InvalidNetworkInsightsAccessScopeId.NotFound' and Message "The Network Access Scope ID '<id>' does not exist".
Why: here the emulator invents a resource-specific not-found code where AWS reports a parameter-value error; clients switching on the code take the wrong branch.
Fix: DeleteNetworkInsightsAccessScope validation/error mapping.
Traces: ec2-network-insights-access-scope-analysis-lifecycle#task#7.

Cause 9 of 10: DeleteVpcEndpoints per-item failure for an absent endpoint.
AWS: Smithy: DeleteVpcEndpoints output member Unsuccessful (xmlName 'unsuccessful', UnsuccessfulItemSet) carries the per-item error; recorded behavior gives 200 with unsuccessful.item.error.code 'InvalidVpcEndpoint.NotFound' and message "The Vpc Endpoint Id '<id>' does not exist" (fixed text with the space-separated token 'Vpc Endpoint Id').
Emulator: 200 with code 'InvalidVpcEndpointId.NotFound' and message "The VpcEndpoint Id '<id>' does not exist".
Why: batch-delete callers reconcile per-item codes; an unknown code string is treated as an unhandled failure rather than an already-absent endpoint.
Fix: the UnsuccessfulItem construction in the DeleteVpcEndpoints handler.
Traces: ec2-vpce-traj-endpoint-lookup-mistakes#task#8 (also #task#9).

Cause 10 of 10: CreateTransitGatewayPeeringAttachment where PeerTransitGatewayId equals TransitGatewayId.
AWS: recorded behavior only. 400, Message "Invalid value <peer-id> for PeerTransitGatewayId. The value of PeerTransitGatewayId needs to be different to TransitGatewayId" — AWS applies the peer-must-differ parameter rule BEFORE any existence lookup; <peer-id> is taken from the request, unquoted.
Emulator: 400 with Message "Invalid value '<peer-id>' for transitGatewayId. The transit gateway to request the peering from could not be found" — it skips the rule and reports a lookup failure against the wrong parameter name.
Why: the caller is told the gateway is missing when the real defect is that it asked a gateway to peer with itself, so it retries forever against a gateway that does exist.
Fix: CreateTransitGatewayPeeringAttachment validation ordering — the peer-differs rule precedes gateway resolution.
Traces: ec2-error-paths-invalid-attachment-parents#task#4.

## Evidence
Cause 1: expected 400 Code 'InvalidCapacityReservationId.NotFound' / "The capacity-reservation ID 'cr-00000000000000000' does not exist"; actual 400 Code 'InvalidCapacityReservationId.Malformed' / "Capacity Reservation ID 'cr-00000000000000000' is malformed".
Cause 2: expected Message "There isn't enough available capacity in the source Capacity Reservation. The current available instance count is 0. You requested to split 5."; actual "Invalid value '5' for instanceCount. The capacity reservation 'cr-00000000000000026' has 1 instances available, and a split leaves at least one of them in it".
Cause 3: expected Message 'Invalid LocalGateway Route Table id lgw-rtb-0000000000000000'; actual "The local-gateway-route-table ID 'lgw-rtb-0000000000000000' is malformed".
Cause 4: expected Code 'InvalidPoolID.NotFound'; actual 'InvalidParameterValue'.
Cause 5: expected Code 'InvalidDhcpOptionsId.Malformed' / "The dhcp-options ID 'dopt-0123456789abcdef0' is malformed"; actual 'InvalidDhcpOptionID.NotFound' / "The dhcpOption ID 'dopt-0123456789abcdef0' does not exist".
Cause 6: expected Code 'InvalidVolume.NotFound' / "The volume 'vol-00000000000000026' does not exist."; actual 'InvalidVolumeID.Malformed' / 'Value ( vol-00000000000000026 ) for parameter VOLUME is invalid.'.
Cause 7: expected "The local-gateway ID 'lgw-0123456789abcdef0' does not exist"; actual "The LocalGateway ID 'lgw-0123456789abcdef0' does not exist".
Cause 8: expected Code 'InvalidParameterValue' / 'NetworkInsightsAccessScopeId (nis-00000000000000000) is not valid'; actual 'InvalidNetworkInsightsAccessScopeId.NotFound' / "The Network Access Scope ID 'nis-00000000000000000' does not exist".
Cause 9: expected unsuccessful.item.error.code 'InvalidVpcEndpoint.NotFound' / "The Vpc Endpoint Id 'vpce-0a1b2c3d4e5f60718' does not exist"; actual 'InvalidVpcEndpointId.NotFound' / "The VpcEndpoint Id 'vpce-0a1b2c3d4e5f60718' does not exist".
Cause 10: expected 'Invalid value tgw-0123456789abcdef0 for PeerTransitGatewayId. The value of PeerTransitGatewayId needs to be different to TransitGatewayId'; actual "Invalid value 'tgw-0123456789abcdef0' for transitGatewayId. The transit gateway to request the peering from could not be found".
All ten are mismatch verdicts from trace set simparity-wire-aws-v1 with no owning issue and no stub.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture ec2-capacity-reservation-error-paths step task#3 (representative) and compare Response.Errors.Error.Code/Message against the recorded response.
3. Replay the other nine bindings listed in trace_ids, one per cause: ec2-capacity-reservation-error-paths#task#2, ec2-coip-pool-mistaken-usage#task#2 and #task#3, ec2-dhcp-options-lifecycle#task#6, ec2-ebs-volume-delete-and-snapshot-not-found#task#5, ec2-local-gateway-create-error-paths#task#3, ec2-network-insights-access-scope-analysis-lifecycle#task#7, ec2-vpce-traj-endpoint-lookup-mistakes#task#8, ec2-error-paths-invalid-attachment-parents#task#4.
4. Each step must match the recorded status, error code and message template before this issue is closed.
