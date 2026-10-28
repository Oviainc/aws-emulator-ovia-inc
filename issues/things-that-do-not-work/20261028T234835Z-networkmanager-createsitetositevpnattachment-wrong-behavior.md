Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 32369025393869168b42ebb7bfdecf3a05b47d7e

# A site-to-site VPN attachment records its VPN connection but never registers it as a telemetry-bearing connection, so `vpn-connection` — one of the three types GetNetworkTelemetry accepts — can never be reported over the wire

_Filed: 2026-10-28T23:48:35Z_
_Operation: CreateSiteToSiteVpnAttachment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `32369025393869168b42ebb7bfdecf3a05b47d7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The state module states that a global network holds THREE kinds of telemetry-bearing connection, and `GetNetworkTelemetry` refuses any `ResourceType` outside exactly those three, so a client is told `vpn-connection` is a reportable kind. But only two handlers ever create such a registration — `AssociateConnectPeer` and `AssociateTransitGatewayConnectPeer`, both through the shared helper `State::register_telemetry_resource` — and neither of them can produce a `vpn-connection` entry: each hard-codes its own peer type.

`CreateSiteToSiteVpnAttachment` is the ONE handler of this crate that is given a `vpn-connection` ARN (`VpnConnectionArn`, which it stores as the attachment's `resource_arn`). It registers nothing with the global network, although the two associates register the session they tie in exactly the situation this handler is in: the crate's helper exists, is called from two siblings, and the attachment's own edge location and ARN are the two members a `TelemetryResourceRecord` needs.

The consequences inside this crate:
* `GetNetworkTelemetry?resourceType=vpn-connection` is accepted (the value passes `is_telemetry_resource_type`) and can only ever answer an empty page, which reads to a client as "this global network has no VPN connections" even after a site-to-site VPN attachment was created on it.
* `TelemetryResourceRecord::connection_type`'s `VPN_CONNECTION_TYPE => CONNECTION_TYPE_IPSEC` branch — the only producer of the modelled `IPSEC` value of `ConnectionHealth$Type` — is unreachable through any request, so `IPSEC` is a value this emulator can never report.
* `state.rs` itself admits the gap in prose ("nothing registers one yet"), while the module doc above it asserts the opposite as a property of the store.

The only client-visible way to get such an entry is to reach into `crate::state` from inside the process, which is what the test suite has to do.

## Evidence

The rule the crate states — src/state.rs:38-40:
```
//! Beside that tree a global network holds the telemetry-bearing connections registered
//! with it (a Connect peer of either kind, a VPN connection), whose session health is what
//! `GetNetworkTelemetry` reports.
```
The three types the reader accepts and refuses outside of — src/state.rs:680-684 and src/ops/get_network_telemetry.rs:186-199:
```
pub const TELEMETRY_RESOURCE_TYPES: [&str; 3] = [
    CONNECT_PEER_TYPE,
    TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
    VPN_CONNECTION_TYPE,
];
```
```
        if let Some(resource_type) = input.resource_type.as_deref() {
            if !crate::state::is_telemetry_resource_type(resource_type) {
                return Err(GetNetworkTelemetryErrorModeled::ValidationException(
```
The branch only a VPN-connection registration can reach — src/state.rs:2397-2401:
```
    pub fn connection_type(&self) -> &'static str {
        match self.resource_type.as_str() {
            VPN_CONNECTION_TYPE => CONNECTION_TYPE_IPSEC,
            // `connect-peer` and `transit-gateway-connect-peer`: both peer over BGP.
            _ => CONNECTION_TYPE_BGP,
```
The only two writers, both Connect-peer associates — src/ops/associate_connect_peer.rs:292-294 and src/ops/associate_transit_gateway_connect_peer.rs:368-371:
```
        store.register_telemetry_resource(crate::state::TelemetryResourceRecord {
            global_network_id: input.global_network_id,
            resource_type: crate::state::CONNECT_PEER_TYPE.to_string(),
```
```
        store.register_telemetry_resource(crate::state::TelemetryResourceRecord {
            global_network_id: input.global_network_id,
            resource_type: crate::state::TRANSIT_GATEWAY_CONNECT_PEER_TYPE.to_string(),
```
The handler that holds a VPN connection and registers nothing — src/ops/create_site_to_site_vpn_attachment.rs:331-333 and :352-360 (the record is inserted and the answer returned with no `register_telemetry_resource` anywhere in the file):
```
            // The resource this attachment attaches is the VPN connection itself, which is
            // also the `VpnConnectionArn` the shape reports.
            resource_arn: input.vpn_connection_arn,
```
The comment that admits the gap — src/state.rs:4483-4486:
```
    /// contributes nothing, not a placeholder. A connection registered with a global
    /// network only for its telemetry lives in a different store
    /// ([`Self::telemetry_resources`]) that [`Self::network_resource_count`] does not count
    /// either, and nothing registers one yet, so the two agree on that zero as well.
```
Test coverage: tests/all/network_telemetry.rs:16-23 has to inject the records by hand because no request can make them — "No request of this suite's own registers such a connection: these suites SEED the store through `crate::state` and then read it back over the wire. ... The registrations a request DOES make are covered by `crate::connect_peer_association_lifecycle` ... and by `crate::transit_gateway_connect_peer_association_lifecycle`" — and its table seeds two `vpn-connection` rows (tests/all/network_telemetry.rs:211 and :230) that no sequence of HTTP calls can reproduce. No test states a reason why a VPN connection cannot be registered by a request.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document configuring an edge in us-west-2
2. CreateSiteToSiteVpnAttachment with CoreNetworkId = that core network and VpnConnectionArn = arn:aws:ec2:us-west-2:111122223333:vpn-connection/vpn-0123456789abcdef0 → 200, the attachment is stored with that ARN as its resource (src/ops/create_site_to_site_vpn_attachment.rs:333)
3. GET /global-networks/{globalNetworkId}/network-telemetry → 200 with an EMPTY NetworkTelemetry list: nothing registered the VPN connection, although src/state.rs:38-40 says the global network holds it
4. GET /global-networks/{globalNetworkId}/network-telemetry?resourceType=vpn-connection → 200 with an empty list (the value is accepted by src/ops/get_network_telemetry.rs:187, so the answer reads as 'none registered')
5. Compare: AssociateConnectPeer on the same global network → a later GetNetworkTelemetry DOES report that peering (src/ops/associate_connect_peer.rs:292), so the same global network reports two of the three documented kinds and can never report the third — and ConnectionHealth$Type = IPSEC (src/state.rs:2399) is unreachable
