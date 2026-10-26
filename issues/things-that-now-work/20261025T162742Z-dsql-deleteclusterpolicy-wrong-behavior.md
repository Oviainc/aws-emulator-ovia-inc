Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b3c7a1d3b5e40780898598751dac20435d474802

# DeleteClusterPolicy removes the policy of a DELETING cluster that PutClusterPolicy refuses to touch

_Filed: 2026-10-25T16:27:42Z_
_Operation: DeleteClusterPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b3c7a1d3b5e40780898598751dac20435d474802`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutClusterPolicy` gates its write on the cluster's status: a cluster whose status is `DELETING` is refused with `ConflictException`, with the comment "A cluster on its way out takes no new policy." `DeleteClusterPolicy` — the sibling mutation on the very same field of the very same record — applies no status gate at all: it looks the cluster up, checks `expectedPolicyVersion`, and writes `cluster.policy = None` whatever status the cluster is in. So inside the settle span after `DeleteCluster` (status `DELETING`), `PutClusterPolicy` answers 409 while `DeleteClusterPolicy` answers 200 and mutates the dying cluster's stored policy, and `GetClusterPolicy` then reports 404 for a cluster that was never supposed to accept the change. The two policy handlers disagree about whether a cluster on its way out accepts a policy mutation.

## Evidence

The gate the sibling applies, `services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs:279-290`:
```
        // A cluster on its way out takes no new policy.
        if cluster.status == crate::state::STATUS_DELETING {
            return Err(PutClusterPolicyErrorModeled::ConflictException(ConflictException {
                message: format!(
                    "cluster {} cannot take a policy while its status is {}",
                    cluster.identifier, cluster.status
                ),
```

The same crate's rule about which statuses accept a mutation, `services/service-com-amazonaws-dsql-dsql/src/state.rs:202-207`:
```
    /// Whether the cluster is in a status that accepts a mutation. A cluster the
    /// clock still owes a transition (`CREATING`, `UPDATING`) or one already on
    /// its way out (`DELETING`) is not.
    pub fn accepts_mutation(&self) -> bool {
        self.status == STATUS_ACTIVE || self.status == STATUS_IDLE
    }
```

What `DeleteClusterPolicy` does instead — no status check between the lookup and the write, `services/service-com-amazonaws-dsql-dsql/src/ops/delete_cluster_policy.rs:220-252`:
```
        let Some(cluster) = state.clusters.get_mut(&input.identifier) else {
            ...
        };

        // Nothing attached: there is no policy, and no version, to delete.
        let Some(attached) = cluster.policy.as_ref().map(|policy| policy.version.clone()) else {
        ...
        cluster.policy = None;
```
(the only other callers of a status gate are `update_cluster.rs:286` and `create_stream.rs:308`, both `if !cluster.accepts_mutation()`).

Test coverage: no test pins a policy mutation on a `DELETING` cluster — `grep DELETING services/service-com-amazonaws-dsql-dsql/tests` matches only `delete_cluster_idempotency.rs`, `cluster_lifecycle.rs`, `stream_listing.rs` and `stream_lifecycle.rs` status assertions, and `tests/all/cluster_lifecycle.rs:1030-1037` covers the mutation refusal for `UpdateCluster` and `DeleteCluster` only.

## Steps to Reproduce

1. CreateCluster with {"deletionProtectionEnabled": false}; wait out the settle span so it is ACTIVE.
2. PutClusterPolicy on it with a benign policy document — 200, note the policyVersion.
3. DeleteCluster the cluster — 200, status DELETING.
4. Immediately (inside the 750 ms settle span) call PutClusterPolicy on the same identifier — 409 ConflictException, "cluster ... cannot take a policy while its status is DELETING".
5. Immediately call DeleteClusterPolicy on the same identifier — 200 and the stored policy is removed, with no status refusal, so the same cluster state accepts one policy mutation and refuses the other.
