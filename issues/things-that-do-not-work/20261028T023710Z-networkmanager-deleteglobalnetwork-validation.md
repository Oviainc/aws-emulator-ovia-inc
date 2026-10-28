Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# DeleteGlobalNetwork ignores the documented "deregister all transit gateways" precondition, orphaning registrations that can never be removed

_Filed: 2026-10-28T02:37:10Z_
_Operation: DeleteGlobalNetwork | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's own operation documentation names four preconditions for deleting a global network: delete the global network objects (devices, links, sites), DEREGISTER ALL TRANSIT GATEWAYS, and delete any core networks. The handler builds its dependency refusal from only three of them (devices, sites, core networks) and never looks at `store.transit_gateway_registrations`, although the crate already has the counting helper it would need (`transit_gateway_registration_count_of`, used by RegisterTransitGateway for its quota).

Consequence is worse than a missing 400: the registration record is keyed by `<GlobalNetworkId>/<TransitGatewayArn>` and every operation that could reach it refuses first on the now-absent global network — `GetTransitGatewayRegistrations` (get_transit_gateway_registrations.rs:160-170) and `DeregisterTransitGateway` (deregister_transit_gateway.rs:185-195) both answer 404 for an unknown global network. So the orphaned registration is unreadable and undeletable, while `RegisterTransitGateway`'s "you cannot register a transit gateway in more than one global network" check scans ALL stored registrations (register_transit_gateway.rs:219-238) and therefore refuses that transit gateway with 409 forever, for every global network, for the lifetime of the store.

## Evidence

Smithy (smithy_query query="doc" shape="DeleteGlobalNetwork"): "Deletes an existing global network. You must first delete all global network objects (devices, links, and sites), deregister all transit gateways, and delete any core networks."

The crate quotes that same sentence as the rule its deletes refuse by — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:510-513: "Network Manager documents the emptying order each delete requires (\"You must first delete all global network objects (devices, links, and sites), deregister all transit gateways, and delete any core networks\", `smithy_query query=\"doc\" shape=\"DeleteGlobalNetwork\"` ...) and refuses the delete until it holds."

The handler drops the transit-gateway clause — src/ops/delete_global_network.rs:155-170:
```
let sites = store.site_ids_of(&input.global_network_id);
let core_networks = store.core_network_ids_of(&input.global_network_id);
let devices = store.device_count_of(&input.global_network_id);
// Listed in the order the operation's own documentation names them: "delete all
// global network objects (devices, links, and sites) … and delete any core
// networks" (a link hangs off a site, so a held link blocks through its site).
if let Some(refusal) = crate::state::dependents_block_delete(
    crate::state::GLOBAL_NETWORK_KIND,
    &[(DEVICE_KIND, devices), (SITE_KIND, sites.len()), (CORE_NETWORK_KIND, core_networks.len())],
) {
```
The helper it does not call already exists — src/state.rs:4040-4047: `pub fn transit_gateway_registration_count_of(&self, global_network_id: &str) -> usize`.

No test pins the allowance: tests/all/delete_dependency_refusal.rs:29 tables only `const DEPENDENT_LABELS: [&str; 4] = ["device", "link", "site", "core network"];` and tests/all/transit_gateway_registration_lifecycle.rs never deletes the global network while a registration stands.

## Steps to Reproduce

1. CreateGlobalNetwork
2. RegisterTransitGateway on that global network with arn:aws:ec2:us-west-2:<account>:transit-gateway/tgw-0123456789abcdef0 → 200
3. DeleteGlobalNetwork on that global network → 200 (expected: ValidationException 400 naming the transit gateway registration that still blocks it)
4. CreateGlobalNetwork again and RegisterTransitGateway the SAME gateway ARN → 409 ConflictException "transit gateway ... is already registered with global network <deleted id>": the orphaned registration is unreadable (GetTransitGatewayRegistrations 404s on the deleted global network) and undeletable (DeregisterTransitGateway 404s on it too), so that gateway can never be registered again
