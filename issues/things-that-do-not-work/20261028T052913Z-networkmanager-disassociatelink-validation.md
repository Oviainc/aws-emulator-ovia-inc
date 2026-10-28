Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# DisassociateLink breaks the device/link association a stored Connect peer association was required to stand on, although the same handler refuses it for a customer gateway association

_Filed: 2026-10-28T05:29:13Z_
_Operation: DisassociateLink | Class: validation_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateConnectPeer` and `AssociateTransitGatewayConnectPeer` both refuse a request naming a link unless the device/link pair already holds a stored link association, and they store that `LinkId` on the association record. `DisassociateLink` removes exactly that pair's association, and it already implements the dependency refusal for ONE of the three records that can depend on it — a customer gateway association naming the same device and link — but counts neither of the two Connect peer association kinds.

So a caller can associate a link, hang a Connect peer association on that device/link pair, then disassociate the link: the peer association survives, naming a link of a device the service no longer considers associated — a state the service refuses to CREATE. Worse, with the link association gone `DeleteLink` no longer blocks either (it counts only link associations and customer gateway associations), so the link itself can then be deleted and `GetConnectPeerAssociations` / `GetTransitGatewayConnectPeerAssociations` keep reporting a `LinkId` naming a link no read of the service can resolve.

## Evidence

The precondition the associate enforces — src/ops/associate_connect_peer.rs:226-245:
```
            // "If you specify a link, it must be associated with the specified device."
            // (`smithy_query query="doc" shape="AssociateConnectPeer"`) — the association
            // that has to hold is the stored device/link one `AssociateLink` makes.
            let pair = crate::state::link_association_key(&input.device_id, link);
            if !store.link_associations.contains_key(&pair) {
                return Err(AssociateConnectPeerErrorModeled::ValidationException(...
```
and the identical check in src/ops/associate_transit_gateway_connect_peer.rs:233-254. Both then store the link on the record (associate_connect_peer.rs:278 `link_id,`).

The half of the refusal DisassociateLink does implement — src/ops/disassociate_link.rs:183-213:
```
        // "You must first disassociate any customer gateways that are associated with the
        // link." (`smithy_query query="doc" shape="DisassociateLink"`) — a customer gateway
        // association names the device and the link this call would untie
        // (`AssociateCustomerGateway`), so the device/link pair stays associated until that
        // association is removed.
        let blocking: Option<String> = store
            .customer_gateway_associations
            .values()
            .find(|association| { ... })
```
Nothing in the file consults `store.connect_peer_associations` or `store.transit_gateway_connect_peer_associations`; the very next statement removes the pair (disassociate_link.rs:215): `let removed = store.link_associations.remove(&key)...`.

The crate states elsewhere that leaving such a tie standing on a precondition it no longer satisfies is a refusal, not a success — src/ops/update_device.rs:263-273: "A link association exists only while its device and its link sit at the SAME site ... so moving a device that is still associated with a link — or unsiting it — would leave that tie standing on a precondition it no longer satisfies ... leaving the tie inconsistent is exactly the state the modelled `ConflictException` (409) reports, so the move is refused and nothing is written".

The downstream delete that then stops blocking — src/ops/delete_link.rs:175-203 counts only `link_associations` and `customer_gateway_associations`.

No test pins the current allowance: tests/all/link_association_lifecycle.rs and tests/all/customer_gateway_association_lifecycle.rs:838-863 (which asserts the 400 for the customer gateway case) never create a Connect peer association over the pair they disassociate.

## Steps to Reproduce

1. CreateGlobalNetwork; CreateSite; CreateDevice at that site; CreateLink at that site
2. AssociateLink(DeviceId=device, LinkId=link) -> 200
3. CreateCoreNetwork with a policy configuring an edge; CreateVpcAttachment; CreateConnectAttachment; CreateConnectPeer -> peer
4. AssociateConnectPeer(ConnectPeerId=peer, DeviceId=device, LinkId=link) -> 200 (refused without step 2)
5. DELETE /global-networks/{id}/link-associations?deviceId=device&linkId=link -> 200 (expected: the dependency ValidationException (400) naming the Connect peer association that still stands on the pair, as the same handler answers for a customer gateway association)
6. GetConnectPeerAssociations still reports the association with LinkId=link while GetLinkAssociations reports no device/link pair
7. DeleteLink(link) -> 200: the Connect peer association now names a link no read of the service can resolve
