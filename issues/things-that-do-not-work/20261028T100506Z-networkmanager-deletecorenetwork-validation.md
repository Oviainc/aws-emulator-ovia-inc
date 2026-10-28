Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# DeleteCoreNetwork neither refuses nor removes the peerings held on the core network, leaving peerings that point at a core network the store no longer holds

_Filed: 2026-10-28T10:05:06Z_
_Operation: DeleteCoreNetwork | Class: validation_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteCoreNetwork` computes its dependency refusal from attachments only, and it cascades the one tie that is not a resource of its own (the prefix list associations). Peerings are neither counted nor cascaded, although a peering hangs off exactly one core network (`PeeringRecord.core_network_id` / `core_network_arn`) and the crate already has the dependent count for that relation, `State::peering_count_of_core_network`, which `CreateTransitGatewayPeering` uses for its quota.

So `CreateTransitGatewayPeering` followed by `DeleteCoreNetwork` answers 200 and leaves a stored peering whose `CoreNetworkId`/`CoreNetworkArn` name a core network no read can resolve. The store then contradicts itself about whether that peering exists: `GetTransitGatewayPeering`, `DeletePeering` and `ListTagsForResource` on the peering ARN still serve it, while the global network's inventory silently drops it, because `peerings_of_global_network` resolves a peering through its core network — so `GetNetworkResources`/`GetNetworkResourceCounts` for resourceType=peering report one fewer peering than the store holds.

This contradicts the rule the crate states for every other delete, and the crate's own code shows the orphan state is known and reachable: `CreateTransitGatewayRouteTableAttachment` carries a dedicated refusal for "a peering whose core network is no longer stored", a state that can only arise because this delete allows it.

## Evidence

The delete, counting attachments only and cascading just the prefix lists — src/ops/delete_core_network.rs:156-184:
```
        if store.core_networks.contains_key(&input.core_network_id) {
            let attachments = store.attachment_count_of_core_network(&input.core_network_id);
            if let Some(refusal) = crate::state::dependents_block_delete(
                crate::state::CORE_NETWORK_KIND,
                &[(crate::state::ATTACHMENT_KIND, attachments)],
            ) {
                return Err(DeleteCoreNetworkErrorModeled::ValidationException(refusal).into());
            }
        }
        let removed = store.core_networks.remove(&input.core_network_id)...
        // A prefix list association is not a resource of its own ... so the ties go with the record
        store.prefix_list_associations.retain(|_, association| association.core_network_id != input.core_network_id);
```
The rule it skips, as the sibling delete states it — src/ops/delete_peering.rs:156-169:
```
        // An attachment created over this peering takes its core network and its edge
        // location from it, so removing the peering would leave that attachment reaching a
        // route table through nothing. Like every other delete of this service, such
        // dependents block the delete until the caller removes them.
```
The dependent count the crate already has — src/state.rs:4043-4049:
```
    /// How many peerings `core_network_id` holds, for the per-core-network quota.
    pub fn peering_count_of_core_network(&self, core_network_id: &str) -> usize {
```
The crate's own acknowledgement that the orphan is reachable — src/ops/create_transit_gateway_route_table_attachment.rs:235-250:
```
        // The attachment hangs off that core network, so a peering whose core network is no
        // longer stored can carry none: an attachment minted there would hold a dangling
        // back-reference and a policy decision no policy made.
        let core_network = store.core_networks.get(&core_network_id).ok_or_else(|| { ... "peering {} peers core network {core_network_id}, which is no longer stored" ...
```
The inventory that then disagrees with the peering readers — src/state.rs:4213-4225:
```
    /// The peerings the global network `global_network_id` holds, in `PeeringId` order.
    fn peerings_of_global_network<'a>(...) -> impl Iterator<Item = &'a PeeringRecord> {
        self.peerings.values().filter(move |peering| {
            self.core_networks.get(&peering.core_network_id).is_some_and(...)
```
No test pins the allowance: tests/all/delete_dependency_refusal.rs contains no peering case at all, and tests/all/transit_gateway_peering.rs never deletes the core network under a live peering.

## Steps to Reproduce

1. CreateGlobalNetwork; CreateCoreNetwork with a policy document configuring an edge location
2. CreateTransitGatewayPeering(CoreNetworkId=cn, TransitGatewayArn=...) -> 200, peering-xxxx
3. GET /global-networks/{gn}/network-resource-count?resourceType=peering -> Count 1
4. DELETE /core-networks/{cn} -> 200 (expected: the dependency ValidationException (400) dependents_block_delete builds, naming the peering still held on the core network, as DeletePeering answers for its own dependents)
5. GET /transit-gateway-peerings/{peering} -> 200, reporting CoreNetworkId/CoreNetworkArn of the deleted core network
6. GET /global-networks/{gn}/network-resource-count?resourceType=peering -> Count 0, so the two readers disagree about whether the peering exists
