Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f21d2937dde7ed50f2d73e4c6f2a7c8bdd006019

# An UpdateCluster that drops a peer leaves the removed peer still naming this cluster, so the dismantled peering is never unwound on the other side

_Filed: 2026-10-29T00:25:17Z_
_Operation: UpdateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f21d2937dde7ed50f2d73e4c6f2a7c8bdd006019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCluster` replaces a cluster's whole `multiRegionProperties` with the one the request carries and never touches the clusters the new peer list OMITS. The model words those omitted entries as peers the update REMOVES (it requires `dsql:RemovePeerCluster` "for each current peer cluster that your list omits", "in its own Region"), and this crate states for itself that a peering is a relationship BOTH clusters carry, which is why `DeleteCluster` calls `state::unwind_peerings_with` before a cluster goes away.

The update path implements only the OPPOSITE direction of that symmetry: after writing the member it calls `state.owe_active_to_clusters_done_waiting(now)`, which completes the wait of a cluster whose peering this call ESTABLISHED, but nothing takes this cluster's half out of the configuration of a peer this call REMOVED. So after A is updated to drop peer B:

* `GetCluster` on B keeps reporting `multiRegionProperties.clusters` naming A, although A is no longer peered with B — the end state the same removal reached through `DeleteCluster` is explicitly cleaned up and B's status recomputed;
* B's dangling half makes `state::multi_region_setup_complete(B)` false, so the NEXT call that recomputes B's status — any later `UpdateCluster` on B, even one that only flips `deletionProtectionEnabled` — settles B into `PENDING_SETUP`, waiting for a peering nobody asked for. That is exactly the harm `unwind_peerings_with`'s own doc names ("the next call that recomputes its status reads that reference as a peering still waiting to be established"), reached through the update instead of the deletion.

## Evidence

The model says the update removes the peers the list omits — `smithy_query(query="doc", shape="UpdateCluster")`:
"dsql:RemovePeerCluster — Permission to remove peer clusters. When you list peer clusters in `multiRegionProperties.clusters`, you need this permission for each current peer cluster that your list omits. Resources: Each removed peer cluster: exact ARN of each removed peer cluster, in its own Region" and "You need `dsql:RemovePeerCluster` only for the peer clusters that the update removes."

The rule this crate states for itself — `services/service-com-amazonaws-dsql-dsql/src/state.rs:963-981` (doc of `unwind_peerings_with`):
"The tear-down counterpart of [`Self::owe_active_to_clusters_done_waiting`]: a peering is a relationship BOTH clusters carry, so a cluster leaving this service has to stop being named by the clusters that stay — otherwise a survivor reports a peer that no longer exists, and the next call that recomputes its status reads that reference as a peering still waiting to be established."

The sibling that calls it — `src/ops/delete_cluster.rs:302-312`:
"// A peering is a relationship BOTH clusters carry, so the call that starts
        // this cluster's removal takes its half out of every OTHER cluster's
        // configuration ...
        if starting {
            state.unwind_peerings_with(&deleted.arn, now);
        }"

The update path that skips it — `src/ops/update_cluster.rs:345-347`:
"        if let Some(multi_region_properties) = input.multi_region_properties {
            cluster.multi_region_properties = Some(multi_region_properties);
        }"
and the only cross-cluster propagation it does, `src/ops/update_cluster.rs:362-364`:
"        // Configuring one side of a peering can complete the OTHER side's wait, so
        // the clock now owes `ACTIVE` to every cluster this call finished peering.
        state.owe_active_to_clusters_done_waiting(now);"
— the ADD direction only; no call takes this cluster out of a dropped peer's `multiRegionProperties`.

What a dangling half does to a later call — `src/state.rs:2262-2290`: `multi_region_setup_complete` is true only when "every peer it names is a cluster this service holds whose own configuration names it back", and `settled_cluster_status` returns `STATUS_PENDING_SETUP` when `cluster.is_multi_region() && !multi_region_setup_complete(..)`.

Test coverage: `tests/all/peering_unwound_by_deletion.rs:5-29` pins the unwinding for the DELETION only — and its own header quotes the same model sentence ("you need dsql:RemovePeerCluster only for the peer clusters that the update removes") as the reason a peer entry names a cluster that is expected to be there. `tests/all/multi_region_cluster_status.rs` covers creating/completing a peering (`a_peering_reaches_active_only_once_both_clusters_name_each_other`) but no case updates a cluster to DROP a peer, so nothing pins the behaviour described here.

## Steps to Reproduce

1. Build three views of one store signed for different Regions, as `tests/all/peering_unwound_by_deletion.rs` does with `harness::Dsql::sharing(&engine, FIRST_REGION)` / `SECOND_REGION` / `THIRD_REGION`.
2. `CreateCluster` A (first Region), B (second Region) and C (third Region); then `UpdateCluster` A with `multiRegionProperties = {witnessRegion: <a fourth Region>, clusters: [B.arn, C.arn]}`, and `UpdateCluster` B and C each naming A back. Wait for the clock: all three report ACTIVE.
3. `UpdateCluster` A with `multiRegionProperties = {witnessRegion: <same>, clusters: [C.arn]}` — the list omits B, the peer the model calls "removed".
4. `GetCluster` B: `multiRegionProperties.clusters` still contains A's ARN, although A no longer names B. (Reach the same end state by DELETING A instead and B's peer list is cleaned up, per `tests/all/peering_unwound_by_deletion.rs`.)
5. `UpdateCluster` B with nothing but `deletionProtectionEnabled: false`, then wait for the clock: B settles into `PENDING_SETUP` instead of `ACTIVE`, because `settled_cluster_status` reads B's stale reference to A as a peering still waiting to be established.
