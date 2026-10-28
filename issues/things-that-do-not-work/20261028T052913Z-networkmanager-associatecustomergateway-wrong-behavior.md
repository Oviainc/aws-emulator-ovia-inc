Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# An empty LinkId is refused with 404 by AssociateCustomerGateway and treated as "no link" by its two sibling associates

_Filed: 2026-10-28T05:29:13Z_
_Operation: AssociateCustomerGateway | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`LinkId` is optional on all three associate operations of this service and all three target the same modelled shape (`len 0..50`, `pattern ^[\s\S]*$`), so the empty string reaches the handler. `AssociateConnectPeer` and `AssociateTransitGatewayConnectPeer` both normalise it away — an empty `LinkId` names no link and is held as none — and the crate's own tests enumerate that spelling as a supported case. `AssociateCustomerGateway` skips that normalisation and feeds the raw member into the store lookup, so `store.links.get("")` misses and the request is refused with `ResourceNotFoundException` (404) for a link whose id is the empty string.

The result is that the same request body shape — `LinkId: ""` — succeeds on two of the three associates of this crate and 404s on the third, and the stored customer gateway association can never be made in the "no link" form through that spelling.

## Evidence

The normalisation the two siblings apply — src/ops/associate_connect_peer.rs:212-214:
```
        // The link is optional, and the modelled pattern admits the empty string, which
        // names no link and is held as none.
        let link_id = input.link_id.clone().filter(|link| !link.is_empty());
```
and src/ops/associate_transit_gateway_connect_peer.rs:217-219:
```
        // The link is optional, and the modelled pattern admits the empty string, which
        // names no link and is held as none, as `AssociateConnectPeer` holds it.
        let link_id = input.link_id.clone().filter(|link| !link.is_empty());
```

The path that skips it — src/ops/associate_customer_gateway.rs:209-219:
```
        if let Some(link_id) = input.link_id.as_deref() {
            let link_resolves = store
                .links
                .get(link_id)
                .is_some_and(|link| link.global_network_id == input.global_network_id);
            if !link_resolves {
                return Err(AssociateCustomerGatewayErrorModeled::ResourceNotFoundException(
                    crate::state::resource_not_found(crate::state::LINK_KIND, link_id),
                )
```
and the record is stored with the raw member at associate_customer_gateway.rs:314 (`link_id: input.link_id,`).

The member is identically modelled on all three (OPERATIONS.md:36, :43, :57 — `LinkId` (string, len 0..50, pattern ^[\s\S]*$)), and the crate's tests pin the empty spelling as a supported case for the siblings — tests/all/connect_peer_association_lifecycle.rs:137-147:
```
/// How a row spells `AssociateConnectPeerRequest$LinkId`: the modelled member is optional
/// and its pattern `^[\s\S]*$` admits the empty string, which names no link
enum LinkMember { Associated, Absent, Empty }
```
while tests/all/customer_gateway_association_lifecycle.rs carries only a `with_link: bool` (lines 173-216), so nothing pins the 404.

## Steps to Reproduce

1. CreateGlobalNetwork; CreateSite; CreateDevice at that site
2. POST /global-networks/{id}/customer-gateway-associations with {"CustomerGatewayArn": "arn:aws:ec2:us-west-2:<account>:customer-gateway/cgw-0123456789abcdef0", "DeviceId": "<device>", "LinkId": ""}
3. Observe 404 ResourceNotFoundException naming a link with the empty id
4. POST /global-networks/{id}/connect-peer-associations with the same empty "LinkId" spelling (and a Connect peer of the global network) -> 200, the association stored with no link
5. The same optional member, identically modelled, is answered two different ways by two handlers of the same crate
