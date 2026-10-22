Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the Direct Connect (OvertureService) emulator

_Filed: 2026-10-21T21:58:08Z_
_Operation: service-com-amazonaws-directconnect-overtureservice | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 73 operations in `services/service-com-amazonaws-directconnect-overtureservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. The service holds no state, so no client workflow can run.

Implement a FIRST, SMALL stateful slice: the connection + private virtual interface lifecycle, which is Direct Connect's core resource loop. Wire protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: OvertureService.<OperationName>` (see README/OPERATIONS.md).

Happy path, in order (6 operations):

1. **CreateConnection** — input `location` (required), `bandwidth` (required), `connectionName` (required), optional `lagId`, `tags`, `providerName`, `requestMACSec`, `billingMode`. Must mint a fresh `connectionId` (`dxcon-xxxxxxxx`), persist the record, and return the stored record: `connectionId`, `connectionName`, `connectionState` = `requested`, `location`, `bandwidth`, `region`, `ownerAccount`, `vlan` (required, 0 for a port), `tags`, `providerName`, `billingMode`, `jumboFrameCapable`, `macSecCapable`, `hasLogicalRedundancy`, `awsDevice`/`awsDeviceV2`/`awsLogicalDeviceId`.
2. **DescribeConnections** — input optional `connectionId` (plus `maxResults`/`nextToken`). With no `connectionId` return ALL stored connections; with one, return exactly that connection; for an unknown id return `DirectConnectClientException` (HTTP 400), not an empty list. Output `connections` must be rendered from the store, field-for-field equal to what CreateConnection returned.
3. **CreatePrivateVirtualInterface** — input `connectionId` (required) + `newPrivateVirtualInterface` (required: `virtualInterfaceName`, `vlan` required; optional `asn`/`asnLong`, `mtu`, `authKey`, `amazonAddress`, `customerAddress`, `addressFamily`, `virtualGatewayId`, `directConnectGatewayId`, `tags`, `enableSiteLink`). Must reject an unknown `connectionId` with `DirectConnectClientException`, mint `virtualInterfaceId` (`dxvif-xxxxxxxx`), store the VIF attached to that connection, and return it with `virtualInterfaceState` = `pending`, `virtualInterfaceType` = `private`, and the vlan/asn/addresses as supplied (not zeroed).
4. **DescribeVirtualInterfaces** — input optional `connectionId` and/or `virtualInterfaceId`. Must FILTER from the store: by `connectionId` return only that connection's VIFs; by `virtualInterfaceId` return only that one; with neither, all of them. Unknown `virtualInterfaceId` => `DirectConnectClientException`.
5. **DeleteVirtualInterface** — input `virtualInterfaceId` (required). Must transition the stored VIF to `deleted` (or remove it) and return `virtualInterfaceState`; a second call / unknown id => `DirectConnectClientException`. A following DescribeVirtualInterfaces must no longer show it as available.
6. **DeleteConnection** — input `connectionId` (required). Must set the stored connection's `connectionState` to `deleted` and return the full connection record from state (same field set as CreateConnection). A following DescribeConnections for that id must reflect `deleted`, and a second DeleteConnection => `DirectConnectClientException`.

Rules for this slice:
- NEVER STUBS: every response must be computed from persisted state. No canned bodies, no echoing the input back without storing it, no random id without a stored record.
- Ids are server-minted and stable across calls; a create followed by a describe must return exactly what was created.
- Modeled errors only: bad/unknown identifiers => `DirectConnectClientException` (HTTP 400) in the awsJson1_1 error shape, never 501 and never a hollow 200.
- Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line in each of the six files so the census reflects progress.

## State Model

In `src/state.rs` (`ComAmazonawsDirectconnectOvertureserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `connections: HashMap<String /* connectionId */, Connection>` where `Connection { connection_id, connection_name, connection_state: ConnectionState, location, bandwidth, region, owner_account, vlan: i32, lag_id: Option<String>, provider_name: Option<String>, billing_mode: Option<BillingMode>, mac_sec_capable: bool, jumbo_frame_capable: bool, has_logical_redundancy, aws_device, aws_device_v2, aws_logical_device_id, loa_issue_time: Option<Timestamp>, tags: Vec<Tag>, virtual_interface_ids: Vec<String> }`
- `virtual_interfaces: HashMap<String /* virtualInterfaceId */, VirtualInterface>` where `VirtualInterface { virtual_interface_id, connection_id, owner_account, virtual_interface_name, virtual_interface_type: "private", virtual_interface_state: VirtualInterfaceState, vlan: i32, asn: i32, asn_long: Option<i64>, amazon_side_asn: Option<i64>, auth_key, amazon_address, customer_address, address_family, mtu, jumbo_frame_capable, virtual_gateway_id, direct_connect_gateway_id, route_filter_prefixes: Vec<RouteFilterPrefix>, bgp_peers: Vec<BgpPeer>, location, region, aws_device_v2, aws_logical_device_id, site_link_enabled, tags: Vec<Tag> }`
- `id_counter: u64` — monotonic source for `dxcon-`/`dxvif-` suffixes so ids are unique and reproducible within a run.

Done when a client script (CreateConnection -> DescribeConnections -> CreatePrivateVirtualInterface -> DescribeVirtualInterfaces -> DeleteVirtualInterface -> DeleteConnection) succeeds end to end with correct, stateful bodies at every step. All other operations (LAGs, interconnects, gateways, hosted connections, tagging, MACsec, resiliency groups, BGP failover tests, ...) stay at 501 and will be filed as individual gap issues later.

## Evidence

services/service-com-amazonaws-directconnect-overtureservice/src/ops/create_connection.rs lines 4 and 240 show the scaffold marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateConnection"))`; the same pattern holds for all 73 files in src/ops/. OPERATIONS.md (generated from smithy-model/com-amazonaws-directconnect-overtureservice.json) documents the protocol (awsJson1_1, target prefix `OvertureService.`) and the exact contracts used above: CreateConnection (line 130), DeleteConnection (196), DescribeConnections (251), CreatePrivateVirtualInterface (166), DescribeVirtualInterfaces (349), DeleteVirtualInterface (238), each declaring `DirectConnectClientException (HTTP 400)` / `DirectConnectServerException (HTTP 500)`. `NewPrivateVirtualInterface` member list taken from the Smithy model (virtualInterfaceName + vlan required). No handler holds state today, so every call answers 501.

## Steps to Reproduce

1. POST / with X-Amz-Target: OvertureService.CreateConnection and body {"location":"EqDC2","bandwidth":"1Gbps","connectionName":"demo-conn"} -> expect 200 with a minted connectionId and connectionState "requested"; actual: HTTP 501 NotImplemented
2. POST / with X-Amz-Target: OvertureService.DescribeConnections and body {} -> expect the connection created above, identical field values
3. POST / with X-Amz-Target: OvertureService.CreatePrivateVirtualInterface and body {"connectionId":"<id>","newPrivateVirtualInterface":{"virtualInterfaceName":"demo-vif","vlan":101,"asn":65000,"virtualGatewayId":"vgw-abc123"}} -> expect 200 with a minted virtualInterfaceId, vlan 101, state "pending"
4. POST / with X-Amz-Target: OvertureService.DescribeVirtualInterfaces and body {"connectionId":"<id>"} -> expect exactly the one VIF just created
5. POST / with X-Amz-Target: OvertureService.DeleteVirtualInterface and body {"virtualInterfaceId":"<vifId>"} -> expect virtualInterfaceState "deleted"; re-describing must not list it as available, and repeating the delete must return DirectConnectClientException (400)
6. POST / with X-Amz-Target: OvertureService.DeleteConnection and body {"connectionId":"<id>"} -> expect the stored connection with connectionState "deleted"; DescribeConnections for that id reflects it, and a second DeleteConnection returns DirectConnectClientException (400)
