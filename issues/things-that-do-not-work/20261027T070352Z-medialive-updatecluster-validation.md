Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# UpdateCluster stores networkSettings.interfaceMappings[].networkId without resolving it against the account's networks

_Filed: 2026-10-27T07:03:52Z_
_Operation: UpdateCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCluster` accepts a `networkSettings.interfaceMappings` list and writes each entry's `networkId` onto the stored cluster after checking only how MANY entries the list has. No entry is resolved against `state.networks`, so an update can point a live cluster's logical interface at a network id this account never created (or at one whose delete has been accepted), and `DescribeCluster`/`ListClusters` then keep reporting that dangling id.

The crate resolves exactly this kind of reference everywhere else a write stores one: `UpdateNode` resolves each `sdiSourceMappings[].sdiSource` and refuses an unknown one through the shared `unknown_reference_message("sdiSourceMappings", "SdiSource", ...)`; `CreateInput`/`UpdateInput` resolve `inputSecurityGroups`, `sdiSources` and `inputDevices` through `input_configuration_error`; `CreateChannelPlacementGroup`/`UpdateChannelPlacementGroup` resolve `Nodes` against the cluster's nodes. The cluster's network reference is the one reference list in the Anywhere family that is stored unresolved, and because a network's `AssociatedClusterIds` is DERIVED by scanning clusters whose mappings name it, the dangling reference is reported by nothing and resolvable by nobody.

(The create side of the same member is already filed as 20261024T132938Z-medialive-createcluster-validation; this is the update handler, which has its own path and its own refusal set.)

## Evidence

The member's own documentation (`smithy_query(query="doc", shape="InterfaceMappingUpdateRequest", member="NetworkId")`):

    "The ID of the network that you want to connect to the specified logicalInterfaceName. You can use the ListNetworks operation to discover all the IDs."

The update path, `services/service-com-amazonaws-medialive-medialive/src/state/clusters.rs:441-451` — the only check on the list is its length:

        if let Some(mappings) = changes
            .network_settings
            .as_ref()
            .and_then(|settings| settings.interface_mappings.as_ref())
        {
            if mappings.len() > MAX_CLUSTER_INTERFACE_MAPPINGS {
                return Err(ClusterUpdateRefusal::TooManyInterfaceMappings {
                    count: mappings.len(),
                });

and `src/state/clusters.rs:607-617` copies the id verbatim:

        if let Some(mappings) = changes.interface_mappings {
            stored.interface_mappings = Some(
                mappings
                    .into_iter()
                    .map(|mapping| InterfaceMapping {
                        logical_interface_name: mapping.logical_interface_name,
                        network_id: mapping.network_id,
                    })

The refusal set the handler can answer with has no reference refusal at all (`src/ops/update_cluster.rs:272-295`): `NoSuchCluster`, `NameTaken`, `TooManyInterfaceMappings`.

The sibling that DOES resolve its reference list, `src/ops/update_node.rs:264-266`:

        Refusal::UnknownSdiSource(sdi_source) => bad_request(
            crate::state::unknown_reference_message("sdiSourceMappings", "SdiSource", &sdi_source),
        ),

and the shared helper's own doc, `src/state.rs:4753-4757`: "Composed from the REQUEST, so it names what the caller must change. One helper for every reference list an input configuration carries (`InputSecurityGroups`, `SdiSources`, `InputDevices`), so the refusals cannot be worded three ways."

Tests: `tests/all/cluster_update.rs:467-514` covers the name bound and the mapping-count bound only; `tests/all/networks.rs` covers the network lifecycle. No test asserts an update with an unknown `networkId`.

## Steps to Reproduce

1. POST /prod/clusters to create a cluster (no networks created).
2. PUT /prod/clusters/<clusterId> with {"networkSettings": {"interfaceMappings": [{"logicalInterfaceName": "video", "networkId": "9999999"}]}}.
3. Expected: a client error naming the networkId member (the refusal class UpdateNode gives an unknown sdiSource), and the stored cluster left as it was.
4. Actual: 200, and GET /prod/clusters/<clusterId> keeps reporting networkId 9999999 while GET /prod/networks lists no network claiming that cluster.
