Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-pst-traj-detached-igw-leaves-blackhole-default-route
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-pst-traj-detached-igw-leaves-blackhole-default-route ec2-pst-traj-route-filters-match-per-table-not-per-route ec2-route-table-association-swap ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3 ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes ec2-subnet-route-table-lifecycle ec2-t12-post-delete-not-found-probes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item.routeSet.item[*].state
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item[routeTableId="rtb-00000000000000042"].routeSet.item[*].state
Issue-Key: ec2|DescribeInternetGateways|2xx->2xx|$.DescribeInternetGatewaysResponse.internetGatewaySet.item.attachmentSet
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item.associationSet
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item.routeSet.item
Issue-Key: ec2|DescribeRouteTables|4xx->2xx|$
Issue-Key: ec2|DescribeRouteTables|4xx->2xx|$.Response
Issue-Key: ec2|DescribeRouteTables|4xx->2xx|status
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# EC2 route-table reads lose routes, associations, blackhole state and deletion

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRouteTables

## Description
Six net-new causes, all in the EC2 route-table / internet-gateway read plane: the route table a client reads back does not reflect the VPC state the preceding calls created. Every cause is a separate defect that must be closed before this issue is submitted.

Cause 1 of 6: a route whose target gateway is no longer attached to the VPC keeps state 'active'.
AWS: Smithy com.amazonaws.ec2#Route member State targets com.amazonaws.ec2#RouteState, enum values [active, blackhole, filtered], documented as "The state of the route. The blackhole state indicates that the route's target isn't available (for example, the specified gateway isn't attached to the VPC...)". So after the internet gateway named by a route's gatewayId is detached from the VPC, DescribeRouteTables must report that route with state 'blackhole'.
Emulator: the route is still reported with state 'active' ($.DescribeRouteTablesResponse...routeSet.item[1].state expected 'blackhole', got 'active'; same for routeSet.item[2].state in the per-table-filter fixture).
Why: a client diagnosing broken egress (or any tool that keys on blackhole routes) concludes the default route is healthy when traffic is in fact dropped, so the detached-gateway fault is invisible.
Fix: the route-state derivation in the EC2 route-table state/projection: route state must be recomputed from the current attachment state of the route's target rather than frozen at CreateRoute time.
Traces: ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#10 (cluster also covers task#15), ec2-pst-traj-route-filters-match-per-table-not-per-route#task#11 (also task#12, task#13), ec2-pst-traj-route-filters-match-per-table-not-per-route#task#14.

Cause 2 of 6: a detached internet gateway still reports an attachment.
AWS: Smithy com.amazonaws.ec2#InternetGatewayAttachment.State is documented "For an internet gateway, the state is available when attached to a VPC; otherwise, this value is not returned". After DetachInternetGateway the gateway's attachmentSet is empty.
Emulator: $.DescribeInternetGatewaysResponse.internetGatewaySet.item.attachmentSet expected '' (empty set), got {'item': {'state': 'available', 'vpcId': 'vpc-...'}} — the attachment survives the detach.
Why: a client cannot tell an attached gateway from a detached one, so attachment-driven logic (and the blackhole derivation in cause 1) stays wrong.
Fix: the DetachInternetGateway handler's state mutation and the DescribeInternetGateways projection of attachmentSet.
Traces: ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#12. Related open issue 20261103T215504Z-ec2-detachinternetgateway-stub.md (DetachInternetGateway scaffold) does not carry this replay key.

Cause 3 of 6: DescribeRouteTables answers an empty routeTableSet for a route table that exists.
AWS: a DescribeRouteTables call that selects an existing route table returns it with its routeSet, associationSet, propagatingVgwSet and tagSet (recorded behavior only for the selection used here; the recorded 200 body carries the single route table).
Emulator: status 200 but $.DescribeRouteTablesResponse.routeTableSet is '' (empty) where the recording returns the route table, its two routes, its subnet association and its Name tag.
Why: the route table becomes undiscoverable mid-trajectory, so every follow-up read or edit keyed on that id fails even though the resource exists.
Fix: the DescribeRouteTables filter/selection logic and the route-table projection in the EC2 state layer.
Traces: ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#11.

Cause 4 of 6: a route-table association swap is not reflected on read.
AWS: after ReplaceRouteTableAssociation the new route table reports the association in associationSet with the new routeTableAssociationId, its routeTableId, the subnetId, main 'false' and associationState.state 'associated'.
Emulator: $.DescribeRouteTablesResponse.routeTableSet.item.associationSet is '' (empty) instead of carrying the swapped association.
Why: a client that moves a subnet between route tables cannot confirm the move and sees the subnet as unassociated, so the swap looks to have failed.
Fix: the route-table association state (association create/replace must be stored and projected by DescribeRouteTables).
Traces: ec2-route-table-association-swap#task#7.

Cause 5 of 6: a prefix-list route to a gateway VPC endpoint never appears in routeSet.
AWS: when a Gateway VPC endpoint is created with RouteTableIds, the associated route table gains a route with destinationPrefixListId set to the service prefix list, gatewayId set to the endpoint id, state 'active' and origin 'CreateRoute', alongside the local route.
Emulator: routeSet.item carries only the local route ({'destinationCidrBlock': '10.22.0.0/16', 'gatewayId': 'local', 'origin': 'CreateRouteTable', 'state': 'active'}); the endpoint route is absent (type_mismatch: expected a two-element list, got the single local route).
Why: a client cannot verify that its gateway endpoint is reachable from the subnet, and route-table-driven reachability checks report no path to the service.
Fix: the CreateVpcEndpoint handler's route-table side effect plus the route-table projection. See also the CreateVpcEndpoint 501 cause filed separately in this sweep.
Traces: ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3#task#7 (cluster also covers ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#11, ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#18, ec2-subnet-route-table-lifecycle#task#16).

Cause 6 of 6: a deleted route table stays readable instead of being refused.
AWS: DescribeRouteTables naming a deleted route table is refused with HTTP 400 and error code InvalidRouteTableID.NotFound. Message template: "The routeTable ID '<routeTableId>' does not exist" — the text outside the quotes is fixed, the quoted id is the RouteTableId value taken from the request (the recording shows it for a route table deleted earlier in the same trajectory).
Emulator: HTTP 200 with the full DescribeRouteTablesResponse for the deleted route table (routeTableId, its local route and its Name tag), and the post-delete poll never converges: "error code None not in ['InvalidRouteTableID.NotFound'] after 12 attempt(s)".
Why: delete is not durable from the client's point of view — a client that polls for disappearance after DeleteRouteTable loops forever, and cleanup/idempotency logic keyed on the not-found refusal never fires.
Fix: the DeleteRouteTable handler's state removal and the DescribeRouteTables not-found path (a deleted id must raise InvalidRouteTableID.NotFound, not resolve).
Traces: ec2-t12-post-delete-not-found-probes#task#12. Related open issue 20261103T123451Z-ec2-dependency-and-not-found-refusals-are-never.md does not carry this replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting).
Cause 1 — ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#10, DescribeRouteTables, expected 200 / actual 200: $.DescribeRouteTablesResponse.routeTableSet.item.routeSet.item[1].state value_mismatch (expected 'blackhole', got 'active'); same assertion at ec2-pst-traj-route-filters-match-per-table-not-per-route#task#11 and #task#14 on routeSet.item[2].state.
Cause 2 — ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#12, DescribeInternetGateways, 200/200: internetGatewaySet.item.attachmentSet type_mismatch (expected '', got {'item': {'state': 'available', 'vpcId': 'vpc-00000000000000026'}}).
Cause 3 — ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#11, DescribeRouteTables, 200/200: $.DescribeRouteTablesResponse.routeTableSet type_mismatch (expected the route table with its routeSet, associationSet and tagSet, got '').
Cause 4 — ec2-route-table-association-swap#task#7, DescribeRouteTables, 200/200: routeTableSet.item.associationSet type_mismatch (expected the swapped association with associationState.state 'associated', got '').
Cause 5 — ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3#task#7, DescribeRouteTables, 200/200: routeTableSet.item.routeSet.item type_mismatch (expected [local route, {gatewayId: <vpce id>, destinationPrefixListId: pl-..., state 'active', origin 'CreateRoute'}], got the local route only); 4 steps affected.
Cause 6 — ec2-t12-post-delete-not-found-probes#task#12, DescribeRouteTables, expected 400 / actual 200: expected $.Response.Errors.Error.Code 'InvalidRouteTableID.NotFound' with Message "The routeTable ID 'rtb-...' does not exist", got a 200 DescribeRouteTablesResponse; poll gave up after 12 attempts.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Inspect these steps: ec2-pst-traj-detached-igw-leaves-blackhole-default-route task#10, task#11, task#12; ec2-pst-traj-route-filters-match-per-table-not-per-route task#11 and task#14; ec2-route-table-association-swap task#7; ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3 task#7; ec2-t12-post-delete-not-found-probes task#12.
4. Each step compares the emulator's DescribeRouteTables / DescribeInternetGateways response against the recorded AWS response; the diffs listed under Evidence reproduce deterministically.
