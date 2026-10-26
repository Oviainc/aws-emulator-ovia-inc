Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# A multi-Region cluster never reports PENDING_SETUP: every cluster settles straight to ACTIVE regardless of multiRegionProperties

_Filed: 2026-10-23T23:29:20Z_
_Operation: CreateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ClusterStatus` models PENDING_SETUP and PENDING_DELETE alongside CREATING/ACTIVE/DELETING/DELETED, and Aurora DSQL uses PENDING_SETUP for a cluster created with `multiRegionProperties` (a witness Region and/or peer clusters) that is still awaiting its peering to complete: the cluster is not usable and shows PENDING_SETUP until the multi-Region configuration is finished, only then ACTIVE.

Expected: CreateCluster carrying `multiRegionProperties` reports CREATING and then PENDING_SETUP (becoming ACTIVE once the peer relationship is established), so a client that polls GetCluster for a usable cluster distinguishes the single-Region and multi-Region paths.

Actual: the handler unconditionally queues a single transition to ACTIVE (`create_cluster.rs` lines 285-288) and the state machine has no PENDING_SETUP/PENDING_DELETE values at all (`state.rs` defines only CREATING/ACTIVE/IDLE/UPDATING/DELETING/DELETED). A multi-Region cluster is therefore indistinguishable from a single-Region one, and two modeled statuses are unreachable on every code path.

## Evidence

Spec: `ClusterStatus` enum (ACTIVE | CREATING | DELETED | DELETING | FAILED | IDLE | INACTIVE | PENDING_DELETE | PENDING_SETUP | UPDATING) is the output `status` type of CreateCluster/GetCluster/UpdateCluster/DeleteCluster; the CreateCluster docs describe multi-Region creation with witness Region and peer clusters as a distinct, permission-gated flow. Handler: `src/ops/create_cluster.rs` lines 268-292 (one pending transition, next_status = ACTIVE, independent of `multi_region_properties`); `src/state.rs` status constants lines 45-55 contain no PENDING_SETUP/PENDING_DELETE. Tests: `tests/all/cluster_lifecycle.rs` line 148 creates a witness+peer cluster and asserts only that the properties round-trip; `the_clock_settles_a_created_cluster_into_active` asserts ACTIVE for all clusters, so no test pins the multi-Region status.

## Steps to Reproduce

1. POST /cluster with {"deletionProtectionEnabled": false, "multiRegionProperties": {"witnessRegion": "us-west-2", "clusters": ["arn:aws:dsql:us-west-2:<account>:cluster/<peer-id>"]}}.
2. GET /cluster/{id} repeatedly.
3. Emulator: CREATING then ACTIVE. Expected: the cluster reports PENDING_SETUP while its multi-Region configuration is outstanding, never jumping straight to ACTIVE.
4. Observe that no request to any operation can ever produce status PENDING_SETUP or PENDING_DELETE.
