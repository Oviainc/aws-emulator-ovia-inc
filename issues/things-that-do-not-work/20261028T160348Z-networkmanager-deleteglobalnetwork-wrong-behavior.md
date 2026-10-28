Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# DeleteGlobalNetwork leaves the route analyses the global network holds behind, unreadable and unremovable, defeating the bound state.rs claims for them

_Filed: 2026-10-28T16:03:48Z_
_Operation: DeleteGlobalNetwork | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteGlobalNetwork` removes only the `global_networks` entry. It counts four kinds of child as blocking the delete (devices, sites, transit gateway registrations, core networks) and removes nothing else, so the `route_analyses` entries the global network holds survive the delete.

A route analysis is a record held BY one global network (`RouteAnalysisRecord::global_network_id`) with no id space of its own outside that parent, and this service models NO operation that removes one — the 95-operation surface has `StartRouteAnalysis` and `GetRouteAnalysis` and nothing else. After the parent is deleted:

* the record is unreadable: `GetRouteAnalysis` refuses on the global network before it ever looks the analysis up, so every read answers 404;
* the record is unremovable: no operation of this service deletes a route analysis, and `DeleteGlobalNetwork` is the one call that could have taken it with the parent;
* the per-global-network bound `state.rs` states for the map is defeated: the surviving entries are keyed to a global network nobody can address, so `route_analysis_count_of` never counts them again and a create-analyse-delete loop grows `route_analyses` without end.

This is the exact orphan this crate refuses everywhere else. `DeleteGlobalNetwork` itself spells out the rule for the transit gateway registration, and `DeleteCoreNetwork` spells out the complementary half — a tie held BY the record goes WITH the record — and acts on it for prefix list associations. Route analyses get neither treatment.

## Evidence

The bound the crate claims for the map — src/state.rs:3819-3825:
```
    /// Route analyses by `RouteAnalysisId`, each carrying the global network it was
    /// started on and the paths the analysis found there ([`RouteAnalysisRecord`]). A
    /// `BTreeMap`, so the store has one stable order like every other map here.
    ///
    /// Bounded by [`MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK`] per global network, so
    /// repeated `StartRouteAnalysis` calls cannot grow it without end.
    pub route_analyses: BTreeMap<String, RouteAnalysisRecord>,
```

The delete, which touches no route analysis — src/ops/delete_global_network.rs:156-185 (the complete dependency list) and 187-197 (the only removal):
```
        let sites = store.site_ids_of(&input.global_network_id);
        let core_networks = store.core_network_ids_of(&input.global_network_id);
        let devices = store.device_count_of(&input.global_network_id);
...
        if let Some(refusal) = crate::state::dependents_block_delete(
            crate::state::GLOBAL_NETWORK_KIND,
            &[
                (crate::state::DEVICE_KIND, devices),
                (crate::state::SITE_KIND, sites.len()),
                (
                    crate::state::TRANSIT_GATEWAY_REGISTRATION_KIND,
                    registrations,
                ),
                (crate::state::CORE_NETWORK_KIND, core_networks.len()),
            ],
        ) {
```
```
        let removed = store
            .global_networks
            .remove(&input.global_network_id)
```

The rule this same handler states, two lines above that list — src/ops/delete_global_network.rs:159-165:
```
        // A transit gateway registration is a tie this global network holds rather than a
        // resource with an id of its own, and the caller removes it with
        // DeregisterTransitGateway — so it blocks the delete exactly like a child resource
        // does, counted the same way the per-global-network quota counts it. Were it not
        // counted, deleting the global network would leave a registration keyed by a global
        // network nobody can address any more: unreadable, undeletable, and still holding
        // its transit gateway against every other global network.
```

The complementary half of the rule, applied by the sibling delete — src/ops/delete_core_network.rs:195-201:
```
        // A prefix list association is not a resource of its own: it is a tie held BY this
        // core network (`state::PrefixListAssociationRecord`), addressed only through it, so
        // the ties go with the record the same way its policy versions do, rather than
        // outliving the core network they were made on.
        store
            .prefix_list_associations
            .retain(|_, association| association.core_network_id != input.core_network_id);
```

Why the survivor is unreadable — src/ops/get_route_analysis.rs:152-160:
```
        if !store.global_networks.contains_key(&input.global_network_id) {
            return Err(GetRouteAnalysisErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(
                    crate::state::GLOBAL_NETWORK_KIND,
                    &input.global_network_id,
                ),
            )
            .into());
        }
```

And the writer that stores it under the parent — src/ops/start_route_analysis.rs:202-209 (`store.start_route_analysis(&input.global_network_id, ...)`), whose own comment at 175-178 repeats the bound: "A quota bounds how many analyses one global network can be made to hold: at the cap the start is refused and nothing is recorded, so repeated calls cannot grow the store without end."

Test coverage: tests/all/route_analysis.rs covers start/read; tests/all/delete_dependency_refusal.rs and tests/all/delete_global_network_transit_gateway_precondition.rs cover the delete's dependency refusals — neither exercises a global network that holds a route analysis.

## Steps to Reproduce

1. POST /global-networks (CreateGlobalNetwork) -> GlobalNetworkId gn
2. POST /global-networks/{gn}/route-analyses with a Source and Destination (StartRouteAnalysis) -> RouteAnalysisId ra; the record is now in state.route_analyses under ra
3. GET /global-networks/{gn}/route-analyses/{ra} (GetRouteAnalysis) -> 200, the analysis is readable
4. DELETE /global-networks/{gn} (DeleteGlobalNetwork) -> 200: the dependency list in delete_global_network.rs:172-182 does not count route analyses, so nothing blocks, and nothing removes the analysis either
5. GET /global-networks/{gn}/route-analyses/{ra} -> 404 ResourceNotFoundException on the GLOBAL NETWORK: the stored record can no longer be read, and no operation of this service can remove it
6. Repeat steps 1-4: state.route_analyses gains one permanently unreachable entry per analysis, although src/state.rs:3823-3824 states the map is "Bounded by MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK per global network, so repeated StartRouteAnalysis calls cannot grow it without end"
