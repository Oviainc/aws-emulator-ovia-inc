Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f6d3b7b16cb7e1ce86179d792dd5264644759e0e

# A policy version differing only in `segment-actions` gets an EMPTY change set, although executing it visibly moves the core network's reported topology

_Filed: 2026-10-28T21:00:26Z_
_Operation: GetCoreNetworkChangeSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f6d3b7b16cb7e1ce86179d792dd5264644759e0e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`policy_changes` — the one function that builds a version's change set — compares only three things: each edge of `core-network-configuration`, the (edge location, name) placements of `segments` / `network-function-groups`, and the `attachment-policies` block as a whole. It never looks at `PolicyTopology::shares`, `PolicyTopology::service_insertions` or `PolicyTopology::static_routes`, which `policy_topology` DOES derive from the document's `segment-actions`.

So a `PutCoreNetworkPolicy` (or `RestoreCoreNetworkPolicyVersion`) whose document keeps the same edges, segments and attachment policies but changes `segment-actions` is stored with `changes: []`. `GetCoreNetworkChangeSet` then answers an empty `CoreNetworkChanges` list and `GetCoreNetworkChangeEvents` an empty event list — i.e. "this version changes nothing" — while `ExecuteCoreNetworkChangeSet` on that same version replaces `CoreNetworkRecord::topology` and the core network's reported `Segments[].SharedSegments` and `NetworkFunctionGroups[].Segments` change, and `ListCoreNetworkRoutingInformation` starts (or stops) reporting the `create-route` static routes.

The crate's own `CHANGE_TYPE` doc admits the gap ("are not yet reported as a change") and justifies it by claiming "the derived topology keeps only ... an edge's location, ASN and inside CIDR blocks, and a segment's or group's name and edge locations" — which `PolicyTopology`'s own fields and `to_segments` / `to_network_function_groups` / `static_routes_of` contradict: the topology keeps the shares, the service insertions and the static routes, and three read operations report them.

Because `CoreNetworkPolicyRecord::changes` is computed ONCE when the version is stored, the empty change set is permanent for that version; nothing later recomputes it.

## Evidence

The comment that admits the gap and the claim it rests on — src/state.rs:1028-1036:
```
/// The members this list leaves out are the ones no difference between two STORED documents
/// can be seen in here. `ATTACHMENT_MAPPING`, the two attachment-route kinds and the
/// routing-policy kinds describe objects that are not part of a document at all. The
/// remaining configuration kinds (`CORE_NETWORK_CONFIGURATION`, `SEGMENTS_CONFIGURATION`,
/// `SEGMENT_ACTIONS_CONFIGURATION`) name blocks a document does carry, but the derived
/// topology keeps only what the operations of this slice act on — an edge's location, ASN
/// and inside CIDR blocks, and a segment's or group's name and edge locations — so two
/// documents differing ONLY in a part of those blocks the topology does not model (segment
/// actions, a segment's other attributes) are not yet reported as a change.
```
The topology it says does not model them — src/state.rs:6917-6928:
```
    /// One entry per `segment-actions[]` entry of action `share` the document carries, in
    /// the order it lists them: which segment shares its routes, and which segments they
    /// are shared with.
    pub shares: Vec<SegmentShare>,
    /// One entry per `segment-actions[]` entry of action `send-via` / `send-to` the
    /// document carries ...
    pub service_insertions: Vec<ServiceInsertion>,
    /// One entry per `segment-actions[]` entry of action `create-route` the document
    /// carries ...
    pub static_routes: Vec<SegmentStaticRoute>,
```
The diff that skips all three — src/state.rs:7602-7679 (`policy_changes`): the only comparisons are `edges_by_location` (:7605-7607), `in_force.placements(&in_force.segments)` / `.network_function_groups` (:7628-7638) and `if in_force.attachment_policies != submitted.attachment_policies` (:7659). `shares`, `service_insertions` and `static_routes` appear nowhere in the function.

The reads that DO report them — src/state.rs:7302 `shared_segments: Some(self.shared_segments_of(&group.name, &declared)),` and src/state.rs:7327-7330 `segments: Some(ServiceInsertionSegments { send_via: ..., send_to: ... })`, both reached from `CoreNetworkRecord::to_shape_with_state` (src/state.rs:2122-2123), plus `topology.static_routes_of(&input.segment_name)` in src/ops/list_core_network_routing_information.rs:425.

The change set is frozen at store time — src/state.rs:1387-1391: "Computed ONCE, where the version is stored, because that is when the change set is generated" with src/state.rs:2017 `let changes = policy_changes(&in_force, &policy_topology(&document));`, and the read is a plain lookup — src/ops/get_core_network_change_set.rs:190 `let changes = version.change_set();`.

Test coverage: tests/all/core_network_segment_actions.rs pins that these very fields ARE part of the reported core network — e.g. :96 `segments: json!([segment("prod", &[]), segment("shared", &["prod"])]),` and :240 `network_function_groups: json!([group("inspection", &["prod"], &[])]),` — and tests/all/core_network_change_set.rs exercises only edge/segment/attachment-policy diffs (:65-:84, :221-:246), so no test covers a segment-action-only diff.

## Steps to Reproduce

1. CreateCoreNetwork on a fresh global network with a policy document declaring edge `us-west-2`, segments `prod` and `shared`, and `"segment-actions": []` → 200, version 1 executed
2. GET /core-networks/{id} → `Segments` reports `shared` with `SharedSegments: []`
3. PutCoreNetworkPolicy with the SAME edges/segments but `"segment-actions": [{"action":"share","mode":"attachment-route","segment":"shared","share-with":["prod"]}]` → 200, version 2
4. GET /core-networks/{id}/core-network-change-sets/2 → `CoreNetworkChanges: []` (and GET .../core-network-change-events/2 → `CoreNetworkChangeEvents: []`), i.e. the change set says version 2 changes nothing
5. POST /core-networks/{id}/core-network-change-sets/2/execute → 200
6. GET /core-networks/{id} again (past the settle span) → `Segments` now reports `shared` with `SharedSegments: ["prod"]` — a change the change set never listed
7. Same with `"action":"create-route"`: the change set is empty, yet ListCoreNetworkRoutingInformation reports new static routes after the execution
