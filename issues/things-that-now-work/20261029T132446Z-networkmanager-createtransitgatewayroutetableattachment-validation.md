Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a1e71609e4dd8609a7a875072f755c7dbeedcd71

# CreateTransitGatewayRouteTableAttachment never checks that the policy in force configures an edge at the peering's edge location, the check its four sibling creates apply

_Filed: 2026-10-29T13:24:45Z_
_Operation: CreateTransitGatewayRouteTableAttachment | Class: validation_

> _Note: this issue was identified against the code base at commit `a1e71609e4dd8609a7a875072f755c7dbeedcd71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other operation of this crate that puts a resource on an edge of a core network re-checks, at the moment of the call, that the policy IN FORCE on that core network configures an edge at the location the new record will carry (`core_network.topology().has_edge_location(..)` -> `crate::state::no_such_edge`). `CreateTransitGatewayRouteTableAttachment` is the one create that does not: it copies `edge_location` out of the stored peering and stores it unchecked.

The peering's edge location was validated only against the policy that was in force when `CreateTransitGatewayPeering` ran. Nothing in the crate removes or refuses a peering when a later `PutCoreNetworkPolicy` + `ExecuteCoreNetworkChangeSet` puts a document in force that configures no edge at that location (`CoreNetworkRecord::execute_policy_version` only swaps the live version and starts the settle span; `DeleteCoreNetwork` is the only thing that touches peerings). So after such a policy change the handler happily mints an attachment sitting at an edge the same core network simultaneously denies having: `GetCoreNetwork` reports no such entry in `Edges`, `ListCoreNetworkRoutingInformation` refuses that very `EdgeLocation` with 400 `no_such_edge`, and `CreateVpcAttachment` / `CreateSiteToSiteVpnAttachment` for a resource in that Region are refused — yet the stored transit gateway route table attachment claims it, and `attachment_claim` is run with an `edge_location` the policy configures nothing for, so the segment the attachment reports is decided against an edge that does not exist.

## Evidence

The handler takes the edge from the peering and never judges it against the policy in force — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_transit_gateway_route_table_attachment.rs:189-191:
```
        let core_network_id = peering.core_network_id.clone();
        let core_network_arn = peering.core_network_arn.clone();
        let edge_location = peering.edge_location.clone();
```
and the only comparison it makes with that location is against the route table's own ARN Region — same file:212-226 (`"peering {} peers a transit gateway in {edge_location}, which holds no route table in {route_table_region}"`). The core network IS resolved a few lines later (same file:239-250) and its topology IS used, but only for the claim — same file:251-259:
```
        let claim = core_network
            .topology()
            .attachment_claim(&crate::state::AttachmentCandidate {
                attachment_type: crate::state::ATTACHMENT_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE,
                edge_location: &edge_location,
```
A grep of this handler for the siblings' guard returns nothing: `has_edge_location|no_such_edge|check_edge_location_list` matches `create_vpc_attachment.rs`, `create_site_to_site_vpn_attachment.rs`, `create_direct_connect_gateway_attachment.rs`, `update_direct_connect_gateway_attachment.rs`, `create_transit_gateway_peering.rs` and `list_core_network_routing_information.rs` — and not `create_transit_gateway_route_table_attachment.rs`.

The sibling creates that DO apply it (kind B, other side):
- services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_vpc_attachment.rs:176-199: "The attachment's edge location is the Region the VPC is in — no request member carries it — and it has to be one the core network's policy configures an edge for, since an attachment attaches to an EDGE of the core network." followed by `if !core_network.topology().has_edge_location(&edge_location) { ... crate::state::no_such_edge(...) }`
- services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_site_to_site_vpn_attachment.rs:198-223: the identical comment and `if !core_network.topology().has_edge_location(&edge_location) { ... no_such_edge ... }`
- services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_direct_connect_gateway_attachment.rs:198-209: "The list is judged by `crate::state::check_edge_location_list` ... then at least one location, every one an edge the core network's policy configures, and no location twice."
- services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_transit_gateway_peering.rs:203-212: `if !core_network.topology().has_edge_location(&edge_location) { ... no_such_edge(FIELD_TRANSIT_GATEWAY_ARN, ...) }`

The same rule, applied by a READ, is what makes the stored attachment self-contradictory — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/list_core_network_routing_information.rs:327-338:
```
        if !topology.has_edge_location(&input.edge_location) {
            return Err( ... crate::state::no_such_edge(
                        crate::state::FIELD_EDGE_LOCATION, &input.core_network_id, &input.edge_location,
```

Nothing clears the peering when the edge goes away — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:2113-2129 (`execute_policy_version`) writes only `executed_at`, `settled_at`, the other versions' `out_of_date`, `live_policy_version` and `updating_until`; the peerings map is untouched.

Test coverage: tests/all/transit_gateway_route_table_attachment.rs and tests/all/transit_gateway_route_table_attachment_lifecycle.rs exercise the create against a peering whose edge the live policy still configures; neither drives a policy change that drops that edge, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a PolicyDocument whose core-network-configuration declares edge-locations [us-east-1]
2. CreateTransitGatewayPeering(CoreNetworkId, TransitGatewayArn=arn:aws:ec2:us-east-1:<account>:transit-gateway/tgw-0123456789abcdef0) -> 200 (the peering's edge us-east-1 is validated here)
3. PutCoreNetworkPolicy on the same core network with a document whose core-network-configuration declares edge-locations [us-west-2] only, then ExecuteCoreNetworkChangeSet for that version and let the settle span fall due
4. GetCoreNetwork -> Edges now lists only us-west-2; ListCoreNetworkRoutingInformation with EdgeLocation=us-east-1 -> 400 ValidationException (no such edge); CreateVpcAttachment for a us-east-1 VPC on this core network -> 400 ValidationException (no such edge)
5. CreateTransitGatewayRouteTableAttachment(PeeringId=<the peering>, TransitGatewayRouteTableArn=arn:aws:ec2:us-east-1:<account>:transit-gateway-route-table/tgw-rtb-0123456789abcdef0) -> 200: the attachment is stored with EdgeLocation us-east-1, an edge this core network reports none of, and its segment was decided by attachment_claim against that non-existent edge
6. Expected: the ValidationException every sibling create answers for an edge location the policy in force configures none for
