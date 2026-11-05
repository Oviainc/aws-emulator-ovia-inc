Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-reject-transit-gateway-peering-attachment ec2.reject-transit-gateway-peering-attachment-after-accept-blocked ec2.transit-gateway-peering-reject-blocks-accept ec2.op-contract-create-transit-gateway-peering-attachment ec2.op-contract-create-transit-gateway-route-table-announcement ec2.transit-gateway-route-table-announcement-over-pending-peering-rejected ec2.op-contract-create-transit-gateway-policy-table-entry ec2.op-contract-delete-route-server ec2.route-server-lifecycle-transitions ec2.route-server-peer-lifecycle-transitions ec2.op-contract-detach-vpn-gateway ec2.create-capacity-reservation-by-splitting-insufficient-capacity-rejected ec2.capacity-reservation-split-produces-child-reservations ec2.op-contract-cancel-reserved-instances-listing ec2.op-contract-delete-dhcp-options ec2.op-contract-delete-internet-gateway-dependency-rejected ec2.image-task-and-fpga-error-semantics ec2.verified-access-and-insights-error-semantics
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateTransitGatewayPeeringAttachment|InvalidTransitGatewayID.NotFound
Issue-Key: ec2.op-contract-create-transit-gateway-peering-attachment|_error_of#19|AssertionError|raise AssertionError("the call real AWS rejects was accepted")
Issue-Key: ec2.op-contract-create-transit-gateway-route-table-announcement|_create_peering_pending_acceptance#8|AssertionError|raise AssertionError("self-accept unexpectedly succeeded")
Issue-Key: ec2.transit-gateway-route-table-announcement-over-pending-peering-rejected|run#12|AssertionError|raise AssertionError("self-accept of a same-account peering attachment unexpectedly succeeded")
Issue-Key: ec2.op-contract-create-transit-gateway-policy-table-entry|run#8|AssertionError|assert code == "DryRunOperation", f"unexpected code: {code}"
Issue-Key: ec2|DescribeRouteServers|InvalidRouteServerId.NotFound
Issue-Key: ec2.route-server-peer-lifecycle-transitions|run#31|AssertionError|assert err.get("Message") == f"Route Server Peer {peer_id} is in an invalid state.", (
Issue-Key: ec2|DetachVpnGateway|InvalidAttachment.NotFound
Issue-Key: ec2.create-capacity-reservation-by-splitting-insufficient-capacity-rejected|run#13|AssertionError|assert message == (
Issue-Key: ec2.capacity-reservation-split-produces-child-reservations|run#25|AssertionError|assert message == (
Issue-Key: ec2.op-contract-cancel-reserved-instances-listing|run#11|AssertionError|assert code2_message == expected_code2_message, f"expected message {expected_code2_message!r}, got {code2_message!r}"
Issue-Key: ec2.op-contract-delete-dhcp-options|run#12|AssertionError|assert code == _DELETE_MALFORMED, code
Issue-Key: ec2.op-contract-delete-internet-gateway-dependency-rejected|run#21|AssertionError|assert not_found_code == "InvalidInternetGatewayId.Malformed", (
Issue-Key: ec2.image-task-and-fpga-error-semantics|run#4|AssertionError|assert not problems, "unexpected image/FPGA error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.verified-access-and-insights-error-semantics|run#4|AssertionError|assert not problems, (
Issue-Commit: a983232b5da502bccde79d757bd972103fe44705

# EC2: 10 net-new behavioral root causes — peering/route-server/vpn-gateway state is lost or unresolvable, three malformed-id refusals are misclassified, and four refusal messages are emulator-authored

> _Note: this issue was identified against the code base at commit `a983232b5da502bccde79d757bd972103fe44705`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTransitGatewayPeeringAttachment, AcceptTransitGatewayPeeringAttachment, CreateTransitGatewayPolicyTableEntry, CreateRouteServer, DeleteRouteServerPeer, DetachVpnGateway, CreateCapacityReservationBySplitting, CancelReservedInstancesListing, DeleteDhcpOptions/DeleteInternetGateway/DeleteFpgaImage, CreateNetworkInsightsPath (10 causes — see description)

## Description
Ten net-new root causes in the EC2 emulator, all grounded in failing behavioral checks listed under `### ec2`. They are numbered below; each carries its own failing check ids. None of these checks is marked ALREADY FILED or OWNED in the replay report. Where an open issue describes a neighbouring symptom it is named, but it does not carry these checks' replay keys.

=== Cause 1 of 10: CreateTransitGatewayPeeringAttachment cannot resolve a peer transit gateway that lives in PeerRegion ===
AWS: Smithy (com-amazonaws-ec2-amazonec2) models CreateTransitGatewayPeeringAttachment with REQUIRED members TransitGatewayId, PeerTransitGatewayId, PeerAccountId and PeerRegion; TGW peering is inherently cross-region, so PeerTransitGatewayId is resolved in the account named by PeerAccountId IN PeerRegion, not in the caller's region. A request naming a transit gateway that exists in PeerRegion is accepted and the attachment is created in `initiatingRequest`/`pendingAcceptance`. The EC2 model declares no per-operation errors (ec2Query common error set), so the refusal semantics here are recorded behavior only.
Emulator: the call is refused 400 `InvalidTransitGatewayID.NotFound` with message "Transit Gateway <tgw-id> was deleted or does not exist." for a peer TGW that was just created through the peer region's endpoint and is `available` there — i.e. PeerTransitGatewayId is looked up only in the caller's region and PeerRegion is ignored.
Why: no client can ever build a transit-gateway peering attachment against this emulator, so every accept/reject/announcement flow behind it is unreachable.
Fix: the CreateTransitGatewayPeeringAttachment handler's peer-gateway resolution (it must resolve PeerTransitGatewayId in PeerRegion/PeerAccountId before refusing NotFound).
Traces: ec2.op-contract-reject-transit-gateway-peering-attachment, ec2.reject-transit-gateway-peering-attachment-after-accept-blocked, ec2.transit-gateway-peering-reject-blocks-accept.

=== Cause 2 of 10: AcceptTransitGatewayPeeringAttachment succeeds when called from the requester side ===
AWS: accept is the accepter's call. For a same-account attachment the requester's own accept is refused with 400 `InvalidParameterValue`, message fixed text "Cannot accept " + the attachment id + " as the source of the peering request", and the attachment stays `pendingAcceptance`. Recorded behavior only (the EC2 model lists no per-operation errors; the model does show AcceptTransitGatewayPeeringAttachment takes only TransitGatewayAttachmentId + DryRun, so the side is decided from stored attachment state).
Emulator: the requester's accept is accepted (2xx) and the attachment moves out of pendingAcceptance; the checks report "the call real AWS rejects was accepted" / "self-accept unexpectedly succeeded".
Why: a client that models the two-sided peering handshake sees a one-sided auto-accept, so IncorrectState paths over a pending attachment can never be observed and an unapproved peering looks established.
Fix: AcceptTransitGatewayPeeringAttachment's handler validation — compare the caller's account/region against the attachment's requester side before mutating state.
Traces: ec2.op-contract-create-transit-gateway-peering-attachment, ec2.op-contract-create-transit-gateway-route-table-announcement, ec2.transit-gateway-route-table-announcement-over-pending-peering-rejected.

=== Cause 3 of 10: CreateTransitGatewayPolicyTableEntry answers NotImplemented instead of honouring DryRun ===
AWS: Smithy models CreateTransitGatewayPolicyTableEntry with required TransitGatewayPolicyTableId, PolicyRuleNumber, TargetRouteTableId and an optional DryRun member. With DryRun=true the request is permission-checked and refused 412 `DryRunOperation` with the fixed message "Request would have succeeded, but DryRun flag is set.", creating nothing; without DryRun the entry is created and is visible through GetTransitGatewayPolicyTableEntries.
Emulator: the DryRun request is answered with error code `NotImplemented` (the scaffold's "operation ... is scaffolded but not implemented" family) rather than DryRunOperation.
Why: a client that uses DryRun as a permission probe before mutating gets an un-handled 501-class error instead of the documented refusal, and the policy-table entry path is unusable.
Fix: the CreateTransitGatewayPolicyTableEntry handler (implement the operation and its DryRun branch). Related but NOT covering this check: open issue 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md.
Traces: ec2.op-contract-create-transit-gateway-policy-table-entry.

=== Cause 4 of 10: a route server created by CreateRouteServer is invisible to DescribeRouteServers ===
AWS: CreateRouteServer returns a RouteServer whose RouteServerId is immediately addressable: DescribeRouteServers(RouteServerIds=[that id]) returns it, State moving `pending` -> `available`, and DeleteRouteServer then drives it to `deleted`. Model: CreateRouteServer output member RouteServer; DescribeRouteServers input RouteServerIds.
Emulator: DescribeRouteServers on the very id CreateRouteServer just returned is refused 400 `InvalidRouteServerId.NotFound` with "The route-server ID '<rs-id>' does not exist" — the created route server is never stored (or is stored under a different identity).
Why: every route-server lifecycle (associate, peer, delete) is unreachable, and a client polling for `available` sees a hard NotFound instead of its own resource.
Fix: CreateRouteServer's state write and/or DescribeRouteServers' lookup in the route-server state store.
Traces: ec2.op-contract-delete-route-server, ec2.route-server-lifecycle-transitions.

=== Cause 5 of 10: DeleteRouteServerPeer's refusal message on a dead peer is emulator-authored ===
AWS: a repeat DeleteRouteServerPeer on an identifier already deleted is refused with the message whose fixed text is "Route Server Peer " + the peer id + " is in an invalid state." (the peer id is the only variable part, taken from the request). Recorded behavior only for the code.
Emulator: the message is "route-server-peer '<rsp-id>' is in the 'deleted' state." — different fixed text, different quoting, and it leaks the internal state name.
Why: clients and operators that match on AWS's message text (or show it to users) see prose AWS never emits.
Fix: the DeleteRouteServerPeer handler's refusal construction (message template).
Traces: ec2.route-server-peer-lifecycle-transitions.

=== Cause 6 of 10: DetachVpnGateway cannot see the attachment AttachVpnGateway just made ===
AWS: AttachVpnGateway(VpnGatewayId, VpcId) records a VpcAttachments entry that moves `attaching` -> `attached`; DetachVpnGateway on that same pair then succeeds (the entry turns `detached`), and a repeat detach of the same VpcId is an accepted no-op.
Emulator: DetachVpnGateway on the pair that was just attached is refused 400 `InvalidAttachment.NotFound` with "The vpnGateway '<vgw-id>' is not attached to the vpc '<vpc-id>'." — the attachment produced by AttachVpnGateway is not persisted where DetachVpnGateway reads it.
Why: a VPN gateway can never be detached, so teardown of any VGW-bearing topology fails and the attachment state machine is unobservable.
Fix: the AttachVpnGateway/DetachVpnGateway handlers and the VPN-gateway attachment state they share.
Traces: ec2.op-contract-detach-vpn-gateway.

=== Cause 7 of 10: CreateCapacityReservationBySplitting's insufficient-capacity refusal message is emulator-authored ===
AWS: splitting more instances than the source reservation has available is refused `InvalidParameterValue` whose message fixed text is "There isn't enough available capacity in the source Capacity Reservation. The current available instance count is " + <available count> + ". You requested to split " + <requested count> + "." — the two numbers come from the request and from the source reservation's state. Model: CreateCapacityReservationBySplitting requires SourceCapacityReservationId and InstanceCount; it declares no per-operation errors, so the code/message are recorded behavior.
Emulator: the code is right but the message is "Invalid value '<n>' for instanceCount. The capacity reservation '<cr-id>' has <m> instances available, and a split leaves at least one of them in it" — entirely different fixed text, and it also encodes a rule ("a split leaves at least one of them in it") AWS does not word that way.
Why: clients that surface or match the refusal text get prose AWS never emits for this error class.
Fix: the CreateCapacityReservationBySplitting handler's validation/refusal message. Related but NOT covering these checks: open issue 20261022T235108Z-ec2-reserved-instance-exchange-capacity-reservation-usage-dedicated.md.
Traces: ec2.create-capacity-reservation-by-splitting-insufficient-capacity-rejected, ec2.capacity-reservation-split-produces-child-reservations.

=== Cause 8 of 10: CancelReservedInstancesListing returns the absent-id message for a MALFORMED listing id ===
AWS: CancelReservedInstancesListing takes a required ReservedInstancesListingId. A well-formed but nonexistent listing id is refused `InvalidInput` with message " Invalid value for 'listingId'"; a value that does not match the listing-id pattern is refused `InvalidInput` with the validation-error message whose fixed text is "1 validation error detected: Value '<value>' at 'reservedInstanceListingId' failed to satisfy constraint: Member must satisfy regular expression pattern: \\w{8}(-\\w{4}){3}-\\w{12}" (the value is echoed from the request). Recorded behavior only for the codes.
Emulator: both inputs get the same " Invalid value for 'listingId'" message, so the malformed case never reports the constraint it violated.
Why: a client cannot distinguish "this id does not exist" from "this id is not an id", which is exactly the distinction AWS's two messages draw.
Fix: the CancelReservedInstancesListing handler's input validation (shape/pattern check before lookup).
Traces: ec2.op-contract-cancel-reserved-instances-listing.

=== Cause 9 of 10: malformed resource ids are classified NotFound instead of Malformed on three delete paths ===
AWS: EC2 validates an id's form (prefix + checksum) before looking it up, so an id that fails that check is refused with the operation's `*.Malformed` code, not its NotFound code: DeleteDhcpOptions -> `InvalidDhcpOptionsId.Malformed`; DeleteInternetGateway -> `InvalidInternetGatewayId.Malformed`; DeleteFpgaImage -> `InvalidFpgaImageID.Malformed`. (The NotFound spellings are reserved for well-formed ids that do not exist — e.g. DescribeDhcpOptions answers `InvalidDhcpOptionID.NotFound`.) Recorded behavior only: the EC2 model carries no per-operation error list.
Emulator: all three answer the NotFound code instead — `InvalidDhcpOptionID.NotFound`, `InvalidInternetGatewayID.NotFound`, `InvalidFpgaImageID.NotFound` — i.e. id-format validation is skipped and every unresolvable id falls through to the lookup miss.
Why: a client that retries on NotFound (treating it as eventual consistency) loops forever on input it should have been told is malformed, and SDK error-class switches on `.Malformed` never fire.
Fix: shared EC2 resource-id validation invoked by these three handlers before state lookup. Related but NOT covering these checks: open issue 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md.
Traces: ec2.op-contract-delete-dhcp-options, ec2.op-contract-delete-internet-gateway-dependency-rejected, ec2.image-task-and-fpga-error-semantics.

=== Cause 10 of 10: CreateNetworkInsightsPath accepts a path between network interfaces that do not exist ===
AWS: Smithy models CreateNetworkInsightsPath with required Source, Protocol and ClientToken; Source/Destination are NetworkInsightsResourceIds that must resolve. A path whose Source and Destination are well-formed but never-issued ENI ids (eni-0123456789abcdef0) is refused with one of `InvalidNetworkInterfaceID.NotFound`, `InvalidParameterValue`, `InvalidParameterCombination` or `MissingParameter` and no path is created. Recorded behavior only for which of those codes AWS picks.
Emulator: the call SUCCEEDS (2xx) and mints a network-insights path over nonexistent endpoints.
Why: Reachability Analyzer flows built on this emulator analyse phantom topology; a client's negative test for an unknown ENI never sees a refusal.
Fix: CreateNetworkInsightsPath's handler validation (resolve Source/Destination against ENI/resource state before creating the path).
Traces: ec2.verified-access-and-insights-error-semantics.

## Evidence
All quoted lines are from the replay report's failing-check details (service heading `### ec2`).

Cause 1 — expected: the attachment is created against the peer-region TGW. Actual: botocore.exceptions.ClientError: An error occurred (InvalidTransitGatewayID.NotFound) when calling the CreateTransitGatewayPeeringAttachment operation: Transit Gateway tgw-00000000000000028 was deleted or does not exist. (the peer TGW had just been created and awaited `available` through the peer region's client).

Cause 2 — expected: requester-side accept refused InvalidParameterValue "Cannot accept <attachment-id> as the source of the peering request", attachment stays pendingAcceptance. Actual: AssertionError: the call real AWS rejects was accepted / AssertionError: self-accept unexpectedly succeeded / AssertionError: self-accept of a same-account peering attachment unexpectedly succeeded.

Cause 3 — expected: code DryRunOperation, message "Request would have succeeded, but DryRun flag is set." Actual: AssertionError: unexpected code: NotImplemented.

Cause 4 — expected: DescribeRouteServers returns the created route server (State pending->available). Actual: botocore.exceptions.ClientError: An error occurred (InvalidRouteServerId.NotFound) when calling the DescribeRouteServers operation: The route-server ID 'rs-0000000000000002f' does not exist (and 'rs-00000000000000026' in the other check) — the id came straight from CreateRouteServer.

Cause 5 — expected message: "Route Server Peer <peer-id> is in an invalid state." Actual: AssertionError: unexpected message: "route-server-peer 'rsp-00000000000000034' is in the 'deleted' state."

Cause 6 — expected: detach of the pair just attached succeeds. Actual: botocore.exceptions.ClientError: An error occurred (InvalidAttachment.NotFound) when calling the DetachVpnGateway operation: The vpnGateway 'vgw-0000000000000002f' is not attached to the vpc 'vpc-00000000000000026'.

Cause 7 — expected message: "There isn't enough available capacity in the source Capacity Reservation. The current available instance count is <available>. You requested to split <requested>." Actual: AssertionError: unexpected error message "Invalid value '3' for instanceCount. The capacity reservation 'cr-00000000000000026' has 2 instances available, and a split leaves at least one of them in it" (and the same template with '1'/1 instance in the split-produces-child-reservations check).

Cause 8 — expected message: "1 validation error detected: Value 'totally-not-a-listing-id' at 'reservedInstanceListingId' failed to satisfy constraint: Member must satisfy regular expression pattern: \\w{8}(-\\w{4}){3}-\\w{12}". Actual: " Invalid value for 'listingId'".

Cause 9 — expected/actual pairs: DeleteDhcpOptions expected InvalidDhcpOptionsId.Malformed, got InvalidDhcpOptionID.NotFound; DeleteInternetGateway expected InvalidInternetGatewayId.Malformed, got 'InvalidInternetGatewayID.NotFound'; delete_fpga_image: expected ['InvalidFpgaImageID.Malformed'], got InvalidFpgaImageID.NotFound.

Cause 10 — create_network_insights_path: expected ['InvalidNetworkInterfaceID.NotFound', 'InvalidParameterCombination', 'InvalidParameterValue', 'MissingParameter'] but the call SUCCEEDED.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids (all under the `ec2` service heading). Per cause:
1. Create a TGW in the caller's region and a second TGW through another region's endpoint, wait for both `available`, then CreateTransitGatewayPeeringAttachment(TransitGatewayId=local, PeerTransitGatewayId=remote, PeerAccountId=own account, PeerRegion=that other region) -> observe InvalidTransitGatewayID.NotFound.
2. Create a same-account, same-region peering attachment and call AcceptTransitGatewayPeeringAttachment with the requester's credentials -> observe success instead of InvalidParameterValue.
3. Create a TGW, a policy table and a TGW route table, then CreateTransitGatewayPolicyTableEntry(..., DryRun=True) -> observe NotImplemented.
4. CreateRouteServer(AmazonSideAsn=64512), then DescribeRouteServers(RouteServerIds=[returned id]) -> observe InvalidRouteServerId.NotFound.
5. Create a route server peer, DeleteRouteServerPeer twice -> compare the second refusal's message.
6. AttachVpnGateway(vgw, vpc), wait for attached, DetachVpnGateway(vgw, vpc) -> observe InvalidAttachment.NotFound.
7. CreateCapacityReservation(InstanceCount=2), then CreateCapacityReservationBySplitting(SourceCapacityReservationId=..., InstanceCount=3) -> compare the message.
8. CancelReservedInstancesListing(ReservedInstancesListingId="totally-not-a-listing-id") -> compare the message with the well-formed-absent case ("00000000-0000-0000-0000-000000000000").
9. DeleteDhcpOptions / DeleteInternetGateway / DeleteFpgaImage with a checksum-failing or all-zero id -> observe the NotFound code instead of Malformed.
10. CreateNetworkInsightsPath(Source="eni-0123456789abcdef0", Destination="eni-0123456789abcdef0", Protocol="tcp") -> observe 2xx.
