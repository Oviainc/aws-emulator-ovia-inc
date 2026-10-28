Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8d66e3013bd632dac86bbefa8efd5ff8af0fdc07

# A deleted multi-Region cluster is never removed from its peers' multiRegionProperties, so the surviving peer keeps a dangling peer ARN and its next update demotes it to PENDING_SETUP

_Filed: 2026-10-28T15:11:35Z_
_Operation: DeleteCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8d66e3013bd632dac86bbefa8efd5ff8af0fdc07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteCluster claims to unwind a multi-Region cluster's peering before the removal starts, but the only thing the handler and the clock do is move the cluster's own status (PENDING_DELETE -> DELETING -> DELETED) and then drop it from the store. Nothing removes the deleted cluster's ARN from the `multiRegionProperties.clusters` list of the OTHER clusters that were peered with it, and no tear-down counterpart of `ComAmazonawsDsqlDsqlState::owe_active_to_completed_peerings` (which the establish direction has, and `UpdateCluster` calls) exists.

Two observable consequences on the SURVIVING peer B after its peer A is deleted and gone:

1. `GetCluster` on B keeps reporting `multiRegionProperties.clusters` naming the ARN of a cluster this service no longer holds — a dangling reference in persisted state.

2. `state::multi_region_setup_complete` resolves a peer ARN only to a LIVE cluster that names it back, so B's peering now reads as incomplete. B is still ACTIVE because nothing re-evaluates a settled cluster, but the next accepted `UpdateCluster` on B — even one carrying only `deletionProtectionEnabled` — recomputes `settled_cluster_status(B)` and the clock settles B into PENDING_SETUP instead of back to ACTIVE. A cluster waiting in PENDING_SETUP refuses every mutation that does not carry `multiRegionProperties` (409) and cannot be deleted at all, so an unrelated update silently freezes a healthy cluster: the only way out is for the caller to notice and rewrite the peer list itself.

The delete therefore leaves the store in a state the crate's own peering rules treat as broken, which is exactly what the handler's doc says the PENDING_DELETE leg exists to prevent.

## Evidence

The crate states the rule it does not implement:

- `src/ops/delete_cluster.rs:5-7`: "Retires the persisted cluster named by the `@httpLabel` `identifier`: it reports `DELETING` — or `PENDING_DELETE` for a multi-Region cluster, whose peering is unwound before the removal starts"
- `src/state.rs:79-81`: "/// See [`STATUS_CREATING`]: the status of a multi-Region cluster whose deletion is /// unwinding its peering, before the removal itself starts."
- `src/state.rs:284-285` (`Cluster::deletion_in_flight` doc): "the peering tear-down of a multi-Region cluster /// ([`STATUS_PENDING_DELETE`])"

What the code actually does — status only, no peer list touched:

- `src/ops/delete_cluster.rs:280-295`: `let unwinding_peering = cluster.is_multi_region(); cluster.status = if unwinding_peering { crate::state::STATUS_PENDING_DELETE } else { crate::state::STATUS_DELETING }.to_string(); ... cluster.pending = Some(crate::state::PendingTransition { due_at: now + crate::state::SETTLE_AFTER, next_status: if unwinding_peering { crate::state::STATUS_DELETING } else { crate::state::STATUS_DELETED } });`
- `src/state.rs:824-834` (`settle_once`, the DELETED leg): `if pending.next_status == STATUS_DELETED { if let Some(retired) = self.clusters.remove(&identifier) { for resource in retired.removed_identifiers() { self.forget_tokens_for(resource); } self.retire(identifier, retired); } continue; }` — tokens and the tombstone only; no other cluster's `multi_region_properties` is read or written.

The establish direction HAS the sibling the tear-down lacks:

- `src/ops/update_cluster.rs:357-359`: "// Configuring one side of a peering can complete the OTHER side's wait, so // the clock now owes `ACTIVE` to every cluster this call finished peering." followed by `state.owe_active_to_completed_peerings(now);`
- `src/state.rs:890` `pub fn owe_active_to_completed_peerings(&mut self, now: SystemTime)` — "Called by the operations that WRITE a cluster's multi-Region configuration". No `forget`/`unpeer` counterpart exists anywhere in the crate.

Why the surviving peer is demoted:

- `src/state.rs:2149-2153` (`multi_region_setup_complete`): `let peers = cluster.multi_region_peers(); !peers.is_empty() && peers.iter().all(|peer_arn| peer_cluster(clusters, peer_arn).is_some_and(|peer| peer.names_peer(&cluster.arn)))`
- `src/state.rs:2164-2173` (`settled_cluster_status`): `if cluster.is_multi_region() && !multi_region_setup_complete(cluster, clusters) { STATUS_PENDING_SETUP } else { STATUS_ACTIVE }`
- `src/ops/update_cluster.rs:348-353`: every accepted update runs `cluster.status = STATUS_UPDATING...; let settles_to = crate::state::settled_cluster_status(&cluster, &state.clusters);`
- The freeze is the crate's own pinned rule: `tests/all/multi_region_cluster_status.rs:341-343`: "RULE: while a cluster waits in `PENDING_SETUP` it accepts the configuration change that completes its peering — the call the wait is for — and refuses every other mutation with the status gate's 409."

Test coverage: `tests/all/multi_region_cluster_status.rs:544` `a_peered_cluster_reports_pending_delete_before_its_removal` builds a peered pair with `peered_pair` (line 239) but asserts only the DELETED cluster's own statuses (`client.get(&identifier)` at lines 581-604); the surviving peer is bound as `_second, _two_id` (line 563) and never read, so nothing pins the current behaviour of the survivor.

## Steps to Reproduce

1. Create two clusters in one store, signed for different Regions, each with `multiRegionProperties.clusters` naming the other's ARN (the `peered_pair` fixture in tests/all/multi_region_cluster_status.rs:239), and wait for the clock until both report ACTIVE.
2. Call DeleteCluster on the first cluster A, then wait for the clock until GetCluster A answers 404.
3. Call GetCluster on the surviving cluster B: it still reports `multiRegionProperties.clusters` = [the ARN of the deleted A], a cluster this service no longer holds.
4. Call UpdateCluster on B with `{"deletionProtectionEnabled": false}` — accepted, 200 UPDATING — and wait for the clock.
5. Call GetCluster on B: it reports PENDING_SETUP instead of ACTIVE. Every further UpdateCluster on B that does not carry `multiRegionProperties` now answers 409, and DeleteCluster B answers 409, so B is frozen until the caller rewrites its peer list.
