Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 08a55bcc278a7e8a745e318c09a635676933476e

# AssociateCustomerGateway never consults the documented "registered in your global network" precondition, so a global network with no registered transit gateway and no core network accepts any customer gateway

_Filed: 2026-10-29T03:58:26Z_
_Operation: AssociateCustomerGateway | Class: validation_

> _Note: this issue was identified against the code base at commit `08a55bcc278a7e8a745e318c09a635676933476e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own modelled documentation states a precondition on WHICH customer gateways may be associated: "You can only associate customer gateways that are connected to a VPN attachment on a transit gateway or core network registered in your global network." The handler never consults that precondition at all. Its complete set of checks is: the `CustomerGatewayArn` parses as a customer gateway ARN, the path's global network is stored, the `DeviceId` names a device of that global network, and — only when a `LinkId` is supplied — the link is of that global network and the device/link tie exists. Nothing reads `transit_gateway_registrations` and nothing reads `core_networks`.

So a brand-new global network that has never had `RegisterTransitGateway` called on it and owns no core network at all still stores a customer gateway association for any well-formed `arn:aws:ec2:<region>:<account>:customer-gateway/cgw-<hex>`, and `GetCustomerGatewayAssociations` then reports it as `PENDING`/`AVAILABLE`. There is no reading of the quoted sentence under which a global network holding ZERO registered transit gateways and ZERO core networks is a global network such a gateway can be "connected to a VPN attachment on".

This is also an inconsistency inside the crate. The sibling associate that carries the parallel documented precondition — `AssociateTransitGatewayConnectPeer`, whose doc reads "You can only associate transit gateway Connect peers that have been created on a transit gateway that's registered in your global network." — DOES enforce it, through a helper the crate already owns (`State::carries_transit_gateway_in`), refusing with the modelled `ValidationException` when the global network carries no registration the ARN could belong to. `AssociateCustomerGateway` models the same `ValidationException` and skips the equivalent gate, so two documented "must be registered in your global network" preconditions are enforced in one handler and ignored in its sibling.

## Evidence

Model text that states the rule (`smithy_query query="doc" shape="AssociateCustomerGateway"`), reproduced verbatim in the handler's own header at services/service-com-amazonaws-networkmanager-networkmanager/src/ops/associate_customer_gateway.rs:43-50:
```
//! Associates a customer gateway with a device and optionally, with a link. If you specify
//! a link, it must be associated with the specified device. You can only associate customer
//! gateways that are connected to a VPN attachment on a transit gateway or core network
//! registered in your global network. When you register a transit gateway or core network,
//! customer gateways that are connected to the transit gateway are automatically included
//! in the global network.
```

The handler's ENTIRE validation, services/service-com-amazonaws-networkmanager-networkmanager/src/ops/associate_customer_gateway.rs:164-246 — ARN form, global network, device, then only the link clause:
```
        if crate::state::parse_customer_gateway_arn(&input.customer_gateway_arn).is_none() {
...
        if !store.global_networks.contains_key(&input.global_network_id) {
...
        let device_resolves = store
            .devices
            .get(&input.device_id)
            .is_some_and(|device| device.global_network_id == input.global_network_id);
...
        let link_id = crate::state::optional_reference(input.link_id);
        if let Some(link_id) = link_id.as_deref() {
...
            let pair = crate::state::link_association_key(&input.device_id, link_id);
            if !store.link_associations.contains_key(&pair) {
```
and then straight to the idempotent-repeat branch, the quota and the insert at lines 248-327. A grep of the whole file for the precondition's own vocabulary finds the doc line and nothing else:
```
$ grep -n 'registration|carries_transit|core_network|vpn' src/ops/associate_customer_gateway.rs
45: //! gateways that are connected to a VPN attachment on a transit gateway or core network
```

The sibling that DOES enforce its parallel sentence, services/service-com-amazonaws-networkmanager-networkmanager/src/ops/associate_transit_gateway_connect_peer.rs:258-280:
```
        // "You can only associate transit gateway Connect peers that have been created on a
        // transit gateway that's registered in your global network." (same doc) — see
        // `carries_transit_gateway_in` for what this global network's registrations can
        // establish about the peer's ARN.
        if !store.carries_transit_gateway_in(
            &input.global_network_id,
            &peer_region,
            peer_account.as_deref(),
        ) {
            return Err(
                AssociateTransitGatewayConnectPeerErrorModeled::ValidationException(
```

The helper already exists in this crate, services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:4262-4281:
```
    /// Whether `global_network_id` holds a transit gateway registration for a gateway that
    /// could carry a Connect peer living in `region` and owned by `account`.
...
    pub fn carries_transit_gateway_in(
        &self,
        global_network_id: &str,
        region: &str,
        account: Option<&str>,
    ) -> bool {
```

Test coverage: tests/all/customer_gateway_association_lifecycle.rs associates customer gateways on a global network that only ever got a device and a link — it never calls `RegisterTransitGateway` or `CreateCoreNetwork` (grep for `transit-gateway-registrations|registered` in that file returns no matches), and its header states only the ARN-shape rule it covers ("The rule holds for every well-formed customer gateway ARN, not only the one shape a recording happens to carry", customer_gateway_association_lifecycle.rs:13-14) — not the registration precondition quoted above. tests/all/associate_customer_gateway_scoping.rs likewise exercises only parent scoping.

## Steps to Reproduce

1. Read the operation's documented precondition: `smithy_query(query="doc", shape="AssociateCustomerGateway")` — "You can only associate customer gateways that are connected to a VPN attachment on a transit gateway or core network registered in your global network."
2. Open services/service-com-amazonaws-networkmanager-networkmanager/src/ops/associate_customer_gateway.rs and read the whole handler body (lines 150-329): the only stored state it consults is `global_networks`, `devices`, `links`, `link_associations`, `customer_gateway_associations`. Neither `transit_gateway_registrations` nor `core_networks` is read.
3. Confirm with `grep -n 'registration|carries_transit|core_network' src/ops/associate_customer_gateway.rs` — the only hit is the doc line at 45.
4. POST /global-networks (CreateGlobalNetwork), then POST /global-networks/{id}/devices (CreateDevice). Register no transit gateway and create no core network.
5. POST /global-networks/{id}/customer-gateway-associations with {"CustomerGatewayArn":"arn:aws:ec2:us-west-2:123456789012:customer-gateway/cgw-0123456789abcdef0","DeviceId":"<device>"}. It answers 200 and stores the association, although the global network holds no registered transit gateway and no core network the gateway could be connected through.
6. Compare with the sibling: src/ops/associate_transit_gateway_connect_peer.rs:258-280 refuses the same class of request with the modelled `ValidationException` via `store.carries_transit_gateway_in(...)` (src/state.rs:4274).
