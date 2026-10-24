Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# networkSettings.interfaceMappings[].networkId is never resolved against the account's networks

_Filed: 2026-10-24T13:29:38Z_
_Operation: CreateCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `InterfaceMapping$NetworkId` is "The ID of the network that you want to connect to the specified logicalInterfaceName". A cluster is created against networks that already exist (CreateNetwork first, then CreateCluster referencing those network ids), so a mapping naming a network the account does not hold is refused by the real service with a client error and no cluster is stored.

Actual: the handler bounds the length of `interfaceMappings` and then projects the whole `networkSettings` onto the response shape verbatim; `insert_cluster` refuses only a duplicate name or a full store. A cluster is therefore created with mappings pointing at nonexistent network ids, DescribeCluster/ListClusters report them, and the derived network-side view silently disagrees: a network's `AssociatedClusterIds` is computed by scanning clusters whose mappings name it, so the dangling reference is reported by nothing and resolvable by nobody.

## Evidence

Spec: `InterfaceMapping$NetworkId` member documentation (above) and CreateCluster's modelled BadRequestException (400) / UnprocessableEntityException (422); the MediaLive Anywhere order of operations (create Network, then Cluster referencing it) is well-established service behavior. Handler: src/ops/create_cluster.rs:205-220 (counts mappings only) and 248-263 (`network_settings: input.network_settings.as_ref().map(crate::state::described_cluster_network_settings)`). State: src/state/clusters.rs:335-347 `insert_cluster` — only NameAlreadyInUse and StoreFull; `self.networks` is never consulted (the id is merely copied at clusters.rs:477). Tests: tests/all/cluster_listing.rs and tests/all/networks.rs cover the happy paths; no test asserts a cluster create with an unknown networkId.

## Steps to Reproduce

1. POST /prod/clusters with {name: "c1", clusterType: "ON_PREMISES", networkSettings: {interfaceMappings: [{logicalInterfaceName: "video", networkId: "9999999"}]}} without creating any network first.
2. Expected: a client error (400/422) naming the networkId member; no cluster stored.
3. Actual: 201 with the mapping echoed from the stored record; GET /prod/clusters/<id> keeps reporting networkId 9999999, and GET /prod/networks lists no network that claims the cluster.
