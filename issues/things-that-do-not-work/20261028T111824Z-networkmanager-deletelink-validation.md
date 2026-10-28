Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: fa255a5b72d32034e1d300bb9323da879d85edac

# A link still named by a stored connection is deleted, leaving the connection pointing at a link nobody can read

_Filed: 2026-10-28T11:18:24Z_
_Operation: DeleteLink | Class: validation_

> _Note: this issue was identified against the code base at commit `fa255a5b72d32034e1d300bb9323da879d85edac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnection` stores `LinkId` and `ConnectedLinkId` as references into `store.links`, resolved within the global network, and `DeleteConnection` explains that a connection needs no dependency refusal of its own precisely BECAUSE removing the connection is what makes the resources it names deletable in turn.

`DeleteLink` does not implement that half for links. It enumerates exactly two dependent kinds — link associations, and customer gateway associations whose optional `link_id` names this link — and then removes the record. A stored connection whose `link_id` / `connected_link_id` names the link blocks nothing, so the link is removed while the connection keeps reporting it: `GetConnections` still answers the connection with that `LinkId`, `GetLinks` answers 404 for it, and `GetNetworkResourceRelationships` silently stops reporting the connection -> link tie it used to report.

The omission is internally inconsistent in two ways. (1) The handler already walks ONE collection of optional link references (the customer gateway association's `link_id`) and refuses on it, while skipping the identical optional link reference a connection holds. (2) The crate's own relationship builder treats a connection's link as a live reference of that link, deriving a `Relationship` from it. Note also that the customer-gateway check it does perform is reachable only through a device/link link association (which already blocks the delete), whereas the connection reference needs no association at all — so the connection path is the one that actually escapes.

## Evidence

The harm the handler says it is guarding against — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/delete_link.rs:164-174: "'You must first disassociate the link from any devices and customer gateways.' ... so a link that is still associated with either is refused rather than deleted in a way that leaves the association pointing at a link nobody can read ... Neither association is a resource with an id of its own, so what still ties one to this link is a count of the stored records naming it".

The two kinds it counts, and nothing else — delete_link.rs:175-205:
```
let link_associations = store.link_associations.values().filter(|association| { association.global_network_id == input.global_network_id && association.link_id == input.link_id }).count();
let gateway_associations = store.customer_gateway_associations.values().filter(|association| { association.global_network_id == input.global_network_id && association.link_id.as_deref() == Some(input.link_id.as_str()) }).count();
// Listed in the order the operation's own documentation names them.
if let Some(refusal) = crate::state::dependents_block_delete(
    crate::state::LINK_KIND,
    &[(crate::state::LINK_ASSOCIATION_KIND, link_associations),
      (crate::state::CUSTOMER_GATEWAY_ASSOCIATION_KIND, gateway_associations)],
) { return Err(...); }
let removed = store.links.remove(&input.link_id).ok_or_else(|| { ... })?;
```

The invariant the sibling delete states — src/ops/delete_connection.rs:187-190: "Nothing hangs off a connection: the two devices and the optional links it names are its REFERENCES, which outlive it, so no dependency refusal applies here and the record goes away on its own. Removing it is what makes those devices, and the global network above them, deletable in turn."

The reference that is left dangling — src/ops/create_connection.rs:248-268 resolves each supplied link against `store.links` within the same global network ("One the request does name is a reference like any other, resolved within the same global network."), and create_connection.rs:306-307 stores it: `link_id,` / `connected_link_id,`. No link association is required for a connection to name a link.

The crate's own treatment of that reference as live — src/state.rs:4586-4593:
```
for link in [record.link_id.as_deref(), record.connected_link_id.as_deref()].into_iter().flatten() {
    if let Some(arn) = link_arn(link) { tie(&mut pairs, &record.arn, &arn); }
}
```

Test coverage: tests/all/delete_link.rs pins only the two association kinds — its module doc (lines 9-12) names "an association of either kind" and `association_cases()` (line 322) / `a_link_still_associated_with_a_device_or_a_gateway_is_refused_until_it_is_disassociated()` (line 384) build only link associations and customer gateway associations. No test creates a connection on the link it deletes, so nothing pins the allowance.

## Steps to Reproduce

1. CreateGlobalNetwork -> CreateSite -> CreateDevice twice in that site -> CreateLink in that site
2. CreateConnection(GlobalNetworkId, DeviceId=device1, ConnectedDeviceId=device2, LinkId=link) -> 200
3. DeleteLink(GlobalNetworkId, LinkId=link) -> 200 (expected: the dependency ValidationException 400 that delete_link.rs raises through `dependents_block_delete`, naming the connection that still references the link)
4. GetConnections on the global network still reports the connection carrying LinkId=link, while GetLinks no longer reports that link
5. GetNetworkResourceRelationships on the global network no longer reports the connection -> link tie it reported in step 2
