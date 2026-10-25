Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1547d46e2415f486cf41cf952181485c961e01df

# A retired cluster drops its own clientToken records but not the CreateStream tokens of the streams it takes with it

_Filed: 2026-10-25T18:42:58Z_
_Operation: DeleteCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1547d46e2415f486cf41cf952181485c961e01df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the clock settles a cluster's deletion, `ComAmazonawsDsqlDsqlState::settle` removes the cluster (and, with it, every stream stored on that cluster record) and calls `forget_tokens_for(&identifier)` with the CLUSTER identifier. `TokenLedger::forget` drops entries whose `RecordedCall::resource` equals that string, so the ledgers whose records are keyed by a cluster identifier (CreateCluster, UpdateCluster, PutClusterPolicy, DeleteClusterPolicy) are cleaned up correctly. `CreateStream`, however, records its token against the STREAM identifier (`RecordedCall::new(&stream_identifier)`), and `forget` is never called with a stream identifier anywhere in the crate, so a `CreateStream` clientToken survives the stream it names forever (until ledger eviction).

That breaks the rule this crate states for itself in two places: `forget_tokens_for`'s doc ("so such a token never outlives the resource it names") and `TokenScope::outlives_its_resource`'s doc ("Every other scope records a call ON a live resource, so the token goes when the resource goes"), which exempts only `TokenScope::DeleteCluster`. `TokenScope::CreateStream` is one of those "every other" scopes, yet its records are unreachable for the only cleanup call site.

Observable consequence: after the cluster that held a stream is deleted, reusing that `CreateStream` clientToken for a new stream is answered 409 `ConflictException` (`TokenVerdict::Mismatch`, because the fingerprint's cluster-identifier part now differs) instead of being served as a fresh request — while the identical reuse pattern on `CreateCluster` IS served, precisely because that ledger was forgotten. Two creates in the same crate therefore disagree about what a clientToken means once the resource it named is gone.

## Evidence

The rule the crate states, `services/service-com-amazonaws-dsql-dsql/src/state.rs:636-647`:
```
    /// Drop every idempotency token recorded against `resource` in the ledgers
    /// whose record is about a resource that is still there, so such a token
    /// never outlives the resource it names. The ledger of the call that removes
    /// the resource keeps its record ([`TokenScope::outlives_its_resource`]).
    fn forget_tokens_for(&mut self, resource: &str) {
        for scope in TokenScope::ALL {
            if scope.outlives_its_resource() {
                continue;
            }
            self.tokens(scope).forget(resource);
        }
    }
```
and the only scope it exempts, `state.rs:307-317`:
```
    /// Whether a token of this scope stays in its ledger after the resource it
    /// names is gone.
    ///
    /// Every other scope records a call ON a live resource, so the token goes
    /// when the resource goes; ...
    fn outlives_its_resource(self) -> bool {
        matches!(self, TokenScope::DeleteCluster)
    }
```

The ONE call site, which passes the cluster identifier, `state.rs:609-616`:
```
            // A settled deletion retires the cluster: `GetCluster` answers
            // `ResourceNotFoundException` and `ListClusters` stops listing it.
            if pending.next_status == STATUS_DELETED {
                if let Some(retired) = self.clusters.remove(&identifier) {
                    self.forget_tokens_for(&identifier);
                    self.retire(identifier, retired);
                }
            }
```
and what `forget` matches on, `state.rs:450-452`:
```
    pub fn forget(&mut self, resource: &str) {
        self.entries.retain(|_, entry| entry.call.resource != resource);
```

The side that is keyed differently — `services/service-com-amazonaws-dsql-dsql/src/ops/create_stream.rs:359-368`:
```
        if let Some(token) = input.client_token {
            state
                .tokens(crate::state::TokenScope::CreateStream)
                .record(
                    token,
                    fingerprint,
                    crate::state::RecordedCall::new(&stream_identifier)
                        .answering(crate::state::STATUS_MEMBER, &created.status),
                );
```
versus the sibling writers, which are keyed by the cluster and therefore ARE forgotten: `create_cluster.rs:348` `crate::state::RecordedCall::new(&identifier)`, `update_cluster.rs:332`, `put_cluster_policy.rs:323`, and `delete_cluster_policy.rs:255-264` whose comment states the premise explicitly: "Recorded against the CLUSTER the policy belonged to ... the token is dropped with that cluster". `grep -n "forget" src/` shows `forget`/`forget_tokens_for` are called from nowhere else, so no path ever passes a stream identifier.

The streams do go with the cluster — `state.rs:14-17`: "a cluster's streams are stored ON the cluster record rather than in a second map beside it: the cluster's deletion takes its streams with it".

Test coverage: no test reuses a `CreateStream` clientToken after the cluster is gone. `tests/all/stream_lifecycle.rs:407-464` pins the replay and `:468-549` the mismatch conflict, both against a LIVE cluster; `tests/all/cluster_lifecycle.rs:842-890` pins the `DeleteCluster` retry; `tests/all/delete_cluster_idempotency.rs:351` pins per-operation token scoping. None of them deletes the cluster and then reuses a stream token.

## Steps to Reproduce

1. CreateCluster with {"deletionProtectionEnabled": false}; wait out the 750 ms settle span (state::SETTLE_AFTER) so it is ACTIVE. Call it cluster C.
2. CreateStream on C with clientToken "t-1" and a valid Kinesis targetDefinition — 200, note streamIdentifier S.
3. DeleteCluster C (no deletion protection) — 200, status DELETING; wait out the settle span so settle() retires C and takes stream S with it.
4. CreateCluster a second cluster D and wait for ACTIVE.
5. CreateStream on D with the SAME clientToken "t-1" — answered 409 ConflictException (client-token mismatch), because the CreateStream ledger still holds "t-1" recorded against the long-gone stream S.
6. Contrast: repeat the experiment with CreateCluster instead — create cluster with clientToken "t-2", delete it, let it retire, then CreateCluster again with "t-2" and different parameters — 200, because forget_tokens_for dropped that cluster-keyed token.
