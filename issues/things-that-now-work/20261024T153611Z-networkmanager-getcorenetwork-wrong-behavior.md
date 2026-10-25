Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# GetCoreNetwork never reports Segments[].SharedSegments or NetworkFunctionGroups[].Segments from the live policy

_Filed: 2026-10-24T15:36:11Z_
_Operation: GetCoreNetwork | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The core network topology GetCoreNetwork reports is derived from the stored policy document, but the derivation ignores the document's `segment-actions` entirely. As a result two members of the reported topology are always absent, whatever the live policy says:

* `CoreNetwork$Segments[].SharedSegments` — the segments a segment shares its routes with, which a Cloud WAN policy declares with a `segment-actions` entry of action `share` / `share-with`.
* `CoreNetwork$NetworkFunctionGroups[].Segments` — the segments attached to a network function group, which the same `segment-actions` list declares via `send-via` / `send-to`.

Expected: after a policy that shares segment A with segment B is in force, GetCoreNetwork reports segment A with `SharedSegments` naming B (and an empty list when the policy shares nothing), and reports each network function group's `Segments` from the group's send-via/send-to actions.

Actual: `PolicyTopology::to_segments` always emits `shared_segments: None` and `to_network_function_groups` always emits `segments: None`, so both members are missing from every GetCoreNetwork (and CreateCoreNetwork) response regardless of the document. A client that reads the core network to discover which segments can route to each other sees no sharing at all, even though the policy it just put in force declares it.

## Evidence

Smithy model: `CoreNetworkSegment` = {Name, EdgeLocations, SharedSegments} and `CoreNetworkNetworkFunctionGroup` = {Name, EdgeLocations, Segments} (smithy_query shape=CoreNetwork,CoreNetworkSegment); GetCoreNetwork's output is `CoreNetwork{Segments, NetworkFunctionGroups, Edges, ...}` — so both members are part of the modeled answer.

Handler/state: src/ops/get_core_network.rs builds the answer through `CoreNetworkRecord::to_shape` -> `PolicyTopology::to_segments` / `to_network_function_groups` (src/state.rs lines 3496-3536), which hardcode `shared_segments: None` and `segments: None` with the comment "Which segments a segment shares routes with is decided by the document's segment actions, which no operation of this slice acts on, so the member is absent rather than guessed." `policy_topology` (src/state.rs lines 3559+) parses only `core-network-configuration`, `segments`, `network-function-groups` and `attachment-policies`; grepping state.rs for `segment-actions` / `share-with` returns no match, so the information is never derived even though the document that carries it is stored verbatim on the record.

Well-established Cloud WAN semantics: segment sharing is expressed only in the policy's `segment-actions` block, and a core network's reported segments are the materialization of that policy — which is also why the emulator already derives Edges, Segments and NetworkFunctionGroups from the same document.

Test coverage: no test in tests/all/ mentions SharedSegments (grep for `SharedSegments` finds nothing), so the gap is unguarded. No filed issue (open or fixed) covers it.

## Steps to Reproduce

1. POST /global-networks to create a global network
2. POST /core-networks with a PolicyDocument whose core-network-configuration declares one edge location, whose `segments` declares two segments (e.g. "prod" and "shared"), and whose `segment-actions` carries {"action":"share","mode":"attachment-route","segment":"shared","share-with":["prod"]}
3. Wait for the core network to settle to AVAILABLE, then GET /core-networks/{CoreNetworkId}
4. Observe CoreNetwork.Segments lists both segments with Name and EdgeLocations but no SharedSegments member at all, instead of reporting "prod" as a shared segment of "shared"
5. Repeat with a policy declaring a network-functions-group plus a send-via segment-action and observe NetworkFunctionGroups[].Segments is likewise absent
