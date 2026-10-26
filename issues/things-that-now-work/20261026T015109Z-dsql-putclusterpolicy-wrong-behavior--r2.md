Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# PutClusterPolicy skips the accepts_mutation() status gate every sibling mutation of a cluster applies

_Filed: 2026-10-26T01:51:09Z_
_Operation: PutClusterPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has one rule for mutating a stored cluster: a cluster the clock still owes a transition (CREATING, UPDATING) or one already on its way out (DELETING) does not accept a mutation, expressed once as `Cluster::accepts_mutation()` and called by `UpdateCluster`, `DeleteCluster` and `CreateStream`. `PutClusterPolicy` writes the same cluster record (`cluster.policy = Some(...)`, and `cluster.next_policy_version()` bumps `policy_versions_issued` on it) but does not call that helper: it only compares the status against `STATUS_DELETING`. So a `PutClusterPolicy` issued against a cluster that is still `CREATING` — or one an `UpdateCluster` just put into `UPDATING` — answers 200 and mutates the cluster, at the very instant the same cluster answers 409 `ConflictException` to `UpdateCluster`, `DeleteCluster` and `CreateStream`. One operation of the crate disagrees with its three siblings and with the helper's own documented rule about what a cluster in a transient status accepts.

## Evidence

The rule and the helper, `src/state.rs:201-207`:
```
impl Cluster {
    /// Whether the cluster is in a status that accepts a mutation. A cluster the
    /// clock still owes a transition (`CREATING`, `UPDATING`) or one already on
    /// its way out (`DELETING`) is not.
    pub fn accepts_mutation(&self) -> bool {
        self.status == STATUS_ACTIVE || self.status == STATUS_IDLE
    }
```
Siblings that call it — `src/ops/update_cluster.rs:284-296`:
```
        // A cluster the clock still owes a transition, or one on its way out, cannot
        // take a configuration change.
        if !cluster.accepts_mutation() {
            return Err(UpdateClusterErrorModeled::ConflictException(ConflictException {
```
`src/ops/create_stream.rs:306-318`:
```
        // A cluster the clock still owes a transition, or one on its way out,
        // cannot take a new stream.
        if !cluster.accepts_mutation() {
            return Err(CreateStreamErrorModeled::ConflictException(ConflictException {
```
`src/ops/delete_cluster.rs:259-261`:
```
        // A cluster the clock still owes a transition cannot start deleting; one that
        // is already deleting answers with the delete in flight.
        if cluster.status != crate::state::STATUS_DELETING && !cluster.accepts_mutation() {
```
The path that skips it — `src/ops/put_cluster_policy.rs:279-290`:
```
        // A cluster on its way out takes no new policy.
        if cluster.status == crate::state::STATUS_DELETING {
            return Err(PutClusterPolicyErrorModeled::ConflictException(ConflictException {
```
and then it mutates the cluster regardless of a `CREATING`/`UPDATING` status, `src/ops/put_cluster_policy.rs:313-317`:
```
        let version = cluster.next_policy_version();
        cluster.policy = Some(crate::state::ClusterPolicy {
            document: input.policy.clone(),
            version: version.clone(),
        });
```
Test coverage of the rule names only the three siblings, `tests/all/cluster_lifecycle.rs:1022-1036`:
```
/// RULE: a cluster the clock still owes a transition does not accept a mutation.

#[tokio::test]
async fn a_cluster_still_creating_refuses_a_mutation() {
...
        ("UpdateCluster", dsql.update(&identifier, json!({"deletionProtectionEnabled": true})).await),
        ("DeleteCluster", dsql.delete(&identifier).await),
```
and `tests/all/stream_lifecycle.rs:705-706`:
```
/// and creating a stream on it is one — so it is refused with 409
/// `ConflictException` until the cluster is `ACTIVE`.
```
No test row exercises `PutClusterPolicy` against a cluster in a transient status, so nothing pins the current answer.

## Steps to Reproduce

1. POST /cluster with {"deletionProtectionEnabled": false} and note the answer's status is CREATING (the clock owes ACTIVE for SETTLE_AFTER = 750ms, src/state.rs:115)
2. Immediately (inside that window) POST /cluster/{identifier}/policy with {"policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"} — it answers 200 and attaches the policy, mutating the CREATING cluster
3. Immediately POST /cluster/{identifier} with {"deletionProtectionEnabled": true} — it answers 409 ConflictException ("cluster ... cannot be updated while its status is CREATING") for the same cluster in the same status
4. Same contradiction with UPDATING: once ACTIVE, POST /cluster/{identifier} to put the cluster into UPDATING, then POST /cluster/{identifier}/policy inside the settle window — 200, while a second UpdateCluster is 409
