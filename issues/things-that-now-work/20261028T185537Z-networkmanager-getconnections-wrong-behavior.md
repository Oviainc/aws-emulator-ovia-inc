Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 293c15726352cf981422bfe1d7cee5f6a5389f12

# GetConnections invents a 10-id maximum for ConnectionIds, a bound the member's documentation does not publish and the crate's own rule says must not be invented

_Filed: 2026-10-28T18:55:37Z_
_Operation: GetConnections | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `293c15726352cf981422bfe1d7cee5f6a5389f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetConnections` runs `crate::state::check_filter_ids("ConnectionIds", ...)`, so a request carrying 11 or more `connectionIds` query values is refused with HTTP 400 / `ValidationException` and no page at all, instead of being served the listing.

The handler's own comment justifies that refusal by asserting that the member documents the bound. The Smithy model says otherwise: `GetConnectionsRequest$ConnectionIds` is documented as "One or more connection IDs." and publishes no maximum at all — unlike `GetSitesRequest$SiteIds`, `GetLinksRequest$LinkIds`, `GetDevicesRequest$DeviceIds`, `DescribeGlobalNetworksRequest$GlobalNetworkIds`, `GetCustomerGatewayAssociationsRequest$CustomerGatewayArns` and `GetTransitGatewayRegistrationsRequest$TransitGatewayArns`, each of which does carry the "The maximum is 10." sentence.

This crate states the rule for exactly this situation in two sibling listings: a filter whose documentation publishes no maximum gets no such refusal invented for it. `GetConnections` is the one handler that breaks that rule, and `state.rs`'s own enumeration of the members that publish the bound leaves `ConnectionIds` out while its opening sentence ("Every id-list filter this service models documents the same bound") claims the opposite — the crate contradicts itself here, and the model settles it.

The existing test that pins the refusal does so on a stated reason — membership in the class of "a list-valued filter member whose documentation publishes a maximum number of values" — that `ConnectionIds` does not satisfy, so the test row for `GetConnections` is itself grounded in the false claim.

## Evidence

Smithy model (kind A), via `smithy_query query="doc" shape="GetConnectionsRequest" member="ConnectionIds"`:
  {"shape": "GetConnectionsRequest", "member": "ConnectionIds", "documentation": "<p>One or more connection IDs.</p>"}
Compare `smithy_query query="doc" shape="GetCustomerGatewayAssociationsRequest" member="CustomerGatewayArns"`:
  "One or more customer gateway Amazon Resource Names (ARNs). The maximum is 10."

The handler that asserts the opposite and refuses:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_connections.rs:165-167
  "// `ConnectionIds` documents a maximum of 10 ids that the modelled
   // `ConnectionIdList` does not express as `@length`, so a longer filter is refused —
   // before anything is listed — rather than silently honoured."
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_connections.rs:182-183
  "crate::state::check_filter_ids(\"ConnectionIds\", &requested)
       .map_err(GetConnectionsErrorModeled::ValidationException)?;"

The crate's own rule, stated in two sibling listings (kind B):
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_connect_peer_associations.rs:189-193
  "`ConnectPeerIds` is \"The IDs of the Connect peers.\"
   (`smithy_query query=\"doc\" shape=\"GetConnectPeerAssociationsRequest\"
   member=\"ConnectPeerIds\"`), which documents none of the \"The maximum is 10.\"
   bound `crate::state::check_filter_ids` guards for the filters that do, so no
   such refusal is invented here."
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_transit_gateway_connect_peer_associations.rs:190-194 states the same rule verbatim for `TransitGatewayConnectPeerArns`.

The bound's own doc, whose enumeration omits ConnectionIds while its first sentence claims universality:
services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5438-5444
  "Every id-list filter this service models documents the same bound: \"The IDs of one or
   more global networks. The maximum is 10.\" on
   `DescribeGlobalNetworksRequest$GlobalNetworkIds`, and the same sentence on
   `GetSitesRequest$SiteIds` ... `GetLinksRequest$LinkIds`, `GetDevicesRequest$DeviceIds` and
   `GetCustomerGatewayAssociationsRequest$CustomerGatewayArns`"

Test coverage, and the stated reason kind A contradicts:
services/service-com-amazonaws-networkmanager-networkmanager/tests/all/filter_maximum_table.rs:10-17
  "The rule: a list-valued filter member whose documentation publishes a maximum number of
   values is refused ... Every such member of this service publishes the same number (\"One or more site IDs. The
   maximum is 10.\" ... and the same sentence on `GetLinksRequest$LinkIds`, `GetDevicesRequest$DeviceIds`,
   `DescribeGlobalNetworksRequest$GlobalNetworkIds` and
   `GetTransitGatewayRegistrationsRequest$TransitGatewayArns`)"  — ConnectionIds is not among them.
services/service-com-amazonaws-networkmanager-networkmanager/tests/all/filter_id_list_maximum.rs:147-155 nevertheless adds a `GetConnections` / `ConnectionIds` row to that table, and tests/all/filter_id_list_maximum.rs:180-185 asserts the 400 for every row past `MAX_FILTER_VALUES`.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateDevice twice in it (a connection ties two devices).
2. CreateConnection 11 times against that device pair, collecting the 11 ConnectionIds.
3. Call GET /global-networks/{GlobalNetworkId}/connections?connectionIds=<id1>&...&connectionIds=<id11>.
4. Observed: HTTP 400 ValidationException naming ConnectionIds ("ConnectionIds may name at most 10 ids, but 11 were given"), and no Connections list.
5. Expected per the model and the crate's own rule for a filter that publishes no maximum (as applied to ConnectPeerIds and TransitGatewayConnectPeerArns): HTTP 200 with the 11 connections, since GetConnectionsRequest$ConnectionIds documents no bound.
6. Contrast: GET /global-networks/{GlobalNetworkId}/connect-peer-associations?connectPeerIds=<11 ids> is served 200, because that handler deliberately invents no bound.
