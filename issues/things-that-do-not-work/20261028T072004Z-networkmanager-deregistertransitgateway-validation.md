Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# DeregisterTransitGateway removes the registration a stored transit gateway Connect peer association was required to stand on

_Filed: 2026-10-28T07:20:04Z_
_Operation: DeregisterTransitGateway | Class: validation_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateTransitGatewayConnectPeer` refuses a request unless the addressed global network holds a transit gateway registration that could have carried the peer, quoting the operation's own documentation for the rule, and stores the association plus a telemetry registration for the peering.

`DeregisterTransitGateway` removes exactly that registration with no check for the associations that stand on it: the handler never consults `store.transit_gateway_connect_peer_associations` (grep for `connect_peer` in the file finds nothing). So a caller can register a gateway, associate one of its Connect peers with a device, then deregister the gateway: the association survives — `GetTransitGatewayConnectPeerAssociations` keeps reporting it and `GetNetworkTelemetry` keeps reporting the peering's session health — while the global network holds no registration for any gateway that peer could have been created on, which is precisely the state the associate refuses to create.

Every other removal in this crate refuses to leave a tie standing on a precondition it no longer satisfies: `DisassociateLink` refuses while a customer gateway association names the pair, the deletes route the same refusal through `crate::state::dependents_block_delete`, and `UpdateDevice` refuses a site move for the same reason. This one handler reasoned explicitly about which associations the doc sentence "This action removes any customer gateway associations" refers to and concluded none are touched, but it never considers the Connect peer associations the crate itself ties to the registration.

## Evidence

The precondition the associate enforces — src/ops/associate_transit_gateway_connect_peer.rs:257-279:
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
and the telemetry entry it then registers — associate_transit_gateway_connect_peer.rs:367-384 (`store.register_telemetry_resource(... resource_type: TRANSIT_GATEWAY_CONNECT_PEER_TYPE ...)`).

The removal that ignores it — src/ops/deregister_transit_gateway.rs:197-227:
```
        let key = crate::state::transit_gateway_registration_key(
            &input.global_network_id,
            &input.transit_gateway_arn,
        );
        let removed = store
            .transit_gateway_registrations
            .remove(&key)
            .ok_or_else(|| { ... })?;

        // "This action removes any customer gateway associations" (the operation's own
        // documentation): ... So no stored association is removed here:
```
Nothing between the two statements reads `transit_gateway_connect_peer_associations`.

The rule the crate applies to the same situation — src/ops/disassociate_link.rs:183-213:
```
        // "You must first disassociate any customer gateways that are associated with the
        // link." ... so the device/link pair stays associated until that
        // association is removed. Like every other dependency refusal of this service this
        // is a VALIDATION failure of the request against stored state
```
and src/ops/update_device.rs:263-273: "would leave that tie standing on a precondition it no longer satisfies ... leaving the tie inconsistent is exactly the state the modelled `ConflictException` (409) reports, so the move is refused and nothing is written".

No test pins the current allowance: tests/all/transit_gateway_registration_lifecycle.rs:272-328 deregisters only registrations with no association over them, and tests/all/transit_gateway_connect_peer_association_lifecycle.rs never deregisters the gateway it registered.

## Steps to Reproduce

1. CreateGlobalNetwork; CreateSite; CreateDevice at that site
2. RegisterTransitGateway(TransitGatewayArn=arn:aws:ec2:us-west-2:<account>:transit-gateway/tgw-0123456789abcdef0) -> 200
3. AssociateTransitGatewayConnectPeer(TransitGatewayConnectPeerArn=arn:aws:ec2:us-west-2:<account>:transit-gateway-connect-peer/tgw-connect-peer-0123456789abcdef0, DeviceId=<device>) -> 200 (refused with 400 without step 2)
4. DELETE /global-networks/{id}/transit-gateway-registrations/<escaped tgw arn> -> 200 (expected: the dependency refusal this crate gives for a tie standing on the removed precondition, naming the Connect peer association)
5. GET /global-networks/{id}/transit-gateway-connect-peer-associations -> still reports the association, and GET /global-networks/{id}/network-telemetry still reports the peering's session, although no registered gateway could carry that peer any more
6. Re-running step 3 now answers 400, so the service reports a state it refuses to create
