Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# ListCoreNetworkRoutingInformation treats an UPDATING core network as having no policy in force, refusing every segment with 400

_Filed: 2026-10-28T07:20:04Z_
_Operation: ListCoreNetworkRoutingInformation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler gates the core network's derived topology on `state_at(now) == AVAILABLE`: while the core network reports any other state it substitutes `PolicyTopology::default()`, so `has_segment`/`has_edge_location` both fail and the request is refused with ValidationException (400) "no such segment" / "no such edge" for a segment and an edge the LIVE policy does configure.

The only non-AVAILABLE state a stored core network of this crate ever reports is `UPDATING`, which is exactly what `ExecuteCoreNetworkChangeSet` and `UpdateCoreNetwork` set for one SETTLE_DELAY (500 ms). So for 500 ms after a change set is executed — the very window in which a client polls the new routing — every routing-information read of every segment of the just-executed policy answers 400, and for 500 ms after a plain description-only `UpdateCoreNetwork` as well.

The gate contradicts the crate's own stated rule twice over: (1) `CoreNetworkRecord::to_shape_with_state` states that `UPDATING` does NOT take the topology away, so `GetCoreNetwork` keeps reporting those very segments and edges in the same instant this handler says they do not exist; (2) the handler's justifying comment appeals to a `CREATING -> AVAILABLE` transition that `CoreNetworkRecord::state_at` documents as a state "no core network of this service passes through". No other handler gates the topology on the core network's state — every create resolves the edge straight off `core_network.topology` — so a `CreateVpcAttachment` at an edge succeeds in the same instant this listing calls that edge nonexistent.

## Evidence

The gate — src/ops/list_core_network_routing_information.rs:301-327:
```
        // A segment and an edge location are the two coordinates of the route table this
        // listing reads, and both are a function of the policy IN FORCE on the core network:
        // ... A core network's policy is in force once the `CREATING` ->
        // `AVAILABLE` transition the clock owes has fallen due, which is exactly when a read
        // of the core network reports its segments and edges, so until then it declares no
        // segment here either.
        let nothing_in_force = crate::state::PolicyTopology::default();
        let topology = if core_network.state_at(now) == crate::state::STATE_AVAILABLE {
            &core_network.topology
        } else {
            &nothing_in_force
        };
        if !topology.has_segment(&input.segment_name) {
            return Err(
                ListCoreNetworkRoutingInformationErrorModeled::ValidationException(
                    crate::state::no_such_segment(...
```

The rule it contradicts — src/state.rs:2069-2079:
```
    /// A core network's segments and edges are the ones the policy in force configures, so
    /// a record being REMOVED — the one state a stored core network never reports itself in
    /// — carries no topology, and every other state carries the one derived from the stored
    /// policy document. A policy being deployed over it (`UPDATING`) does not take the
    /// topology away: a policy IS in force there, the one that execution put in force.
    pub fn to_shape_with_state(&self, state: &str) -> CoreNetwork {
```

The state the gate actually excludes, and that the crate says is the only one — src/state.rs:2046-2058:
```
    /// network is `UPDATING` while a reconfiguration of it is still outstanding — a policy
    /// version being put in force, or an update of its own members — and `AVAILABLE` once
    /// none is, which is the state it is created in. `CreateCoreNetwork` answers the core
    /// network ALREADY `AVAILABLE`, so nothing is owed on its creation and the modelled
    /// `CREATING` ([`STATE_CREATING`]) is a state no core network of this service passes
    /// through.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
```

The writers that put it into UPDATING — src/state.rs:2039-2041 (`execute_policy_version`): `self.topology = topology;` / `self.updating_until = Some(now + SETTLE_DELAY);` — and src/ops/update_core_network.rs:170-177:
```
        // An update is not applied instantly: AWS answers the reconfigured core network in
        // the non-terminal `UPDATING` state ... and the policy in force stays in force throughout
        // (`crate::state::CoreNetworkRecord::to_shape_with_state`).
        record.updating_until = Some(now + crate::state::SETTLE_DELAY);
```

The sibling that applies no such gate — src/ops/create_vpc_attachment.rs:190: `if !core_network.topology.has_edge_location(&edge_location) {`.

No test pins the current behaviour: tests/all/core_network_routing_information.rs:238-250 and :341-344 sleep `PAST_SETTLE` after `ExecuteCoreNetworkChangeSet` before every listing, so the UPDATING window is never read.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document declaring segment "shared" and an edge in us-west-2
2. GET /core-networks/{id}/core-network-routing-information?segmentName=shared&edgeLocation=us-west-2 -> 200
3. PutCoreNetworkPolicy with a second document, wait for the change set to be READY_TO_EXECUTE, then POST .../core-network-change-sets/{version}/execute -> 200
4. IMMEDIATELY (within 500 ms) GET /core-networks/{id}/core-network-routing-information?segmentName=shared&edgeLocation=us-west-2 -> 400 ValidationException naming SegmentName as a segment the core network does not declare (expected: the routes of the policy just put in force)
5. In the same window GET /core-networks/{id} -> 200 reporting Segments[].Name = "shared" and Edges[].EdgeLocation = us-west-2, contradicting the 400
6. The same 400 follows any UpdateCoreNetwork (e.g. a Description change) for 500 ms, although that call changes no policy at all
