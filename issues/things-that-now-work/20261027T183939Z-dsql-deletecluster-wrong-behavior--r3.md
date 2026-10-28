Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 086f796f1e6ea795d98417e1d50d205591b703fd

# A repeated DeleteCluster of a multi-Region cluster in PENDING_DELETE answers 409, although the deletion is already in flight

_Filed: 2026-10-27T18:39:39Z_
_Operation: DeleteCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `086f796f1e6ea795d98417e1d50d205591b703fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteCluster exempts a delete-in-flight from the cluster status gate, but the exemption is keyed on the literal `DELETING` status only. A multi-Region cluster's deletion first reports `PENDING_DELETE` (the handler's own branch), and `Cluster::accepts_mutation` admits only ACTIVE/IDLE, so a second DeleteCluster that arrives during the PENDING_DELETE span is refused 409 ConflictException ("cluster ... cannot be deleted while its status is PENDING_DELETE") instead of answering the delete already in flight. The same second call against a single-Region cluster (which goes straight to `DELETING`) answers 200 with the in-flight delete, and the same multi-Region cluster answers 200 once the clock has carried it on to `DELETING` — so one and the same retry is a success or a conflict depending only on which leg of the delete chain it lands in. This is reachable without any client token: `clientToken` is optional and SDKs mint a fresh one per call, so a retry whose token the ledger has not seen falls through to this gate.

## Evidence

Handler, the exemption that names only one of the two in-flight statuses — `services/service-com-amazonaws-dsql-dsql/src/ops/delete_cluster.rs:261`:
```
        // A cluster the clock still owes a transition cannot start deleting; one that
        // is already deleting answers with the delete in flight.
        if cluster.status != crate::state::STATUS_DELETING {
            if let Some(conflict) = cluster.mutation_conflict("be deleted") {
                return Err(DeleteClusterErrorModeled::ConflictException(conflict).into());
            }
        }
```
The same handler states that PENDING_DELETE *is* this operation's own answer for a started deletion — `src/ops/delete_cluster.rs:5`: "Retires the persisted cluster named by the `@httpLabel` `identifier`: it reports `DELETING` — or `PENDING_DELETE` for a multi-Region cluster, whose peering is unwound before the removal starts" — and that every served branch is equally a success — `src/ops/delete_cluster.rs:296`: "Recorded whichever branch answered: a delete of a cluster already on its way out succeeded just as much as the one that started the deletion, so its retry has the same recorded result to answer from."
The gate it falls into admits neither — `src/state.rs:270`:
```
    pub fn accepts_mutation(&self) -> bool {
        self.status == STATUS_ACTIVE || self.status == STATUS_IDLE
    }
```
Test coverage: `tests/all/multi_region_cluster_status.rs:552` pins that the FIRST delete of a peered cluster answers `STATUS_PENDING_DELETE` (`reports: STATUS_PENDING_DELETE`), and `tests/all/delete_cluster_idempotency.rs:86` only covers "one retry while the cluster is still DELETING" — single-Region. No test exercises a second delete during PENDING_DELETE, so the asymmetry is unpinned.

## Steps to Reproduce

1. Build two views of one engine signed for two Regions (as tests/all/multi_region_cluster_status.rs::peered_pair does) and create a peered multi-Region cluster pair with deletionProtectionEnabled=false, letting both settle.
2. DELETE /cluster/{identifier} for one of the peered clusters → 200 with status PENDING_DELETE.
3. Immediately DELETE /cluster/{identifier} again (no client-token, or a fresh one) → 409 ConflictException "cluster {id} cannot be deleted while its status is PENDING_DELETE".
4. Repeat steps 2-3 against a single-Region cluster: the second delete answers 200 with status DELETING — the in-flight delete — so the same retry is served for one cluster kind and refused for the other.
5. Wait for the clock to carry the multi-Region cluster to DELETING and delete again → 200, confirming the refusal is confined to the PENDING_DELETE leg of the same deletion.
