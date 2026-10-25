Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1547d46e2415f486cf41cf952181485c961e01df

# A virtual cluster's teardown does not reach the endpoints and job runs it holds: they stay live inside a TERMINATED cluster

_Filed: 2026-10-25T18:40:48Z_
_Operation: DeleteVirtualCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1547d46e2415f486cf41cf952181485c961e01df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteVirtualCluster` moves only the cluster record itself into `TERMINATING` and then `TERMINATED`. Nothing in the handler (or in `state::move_virtual_cluster`) touches the managed endpoints and job runs the cluster holds, so after the teardown settles the store contains a cluster that reports `state: TERMINATED` while:

* every managed endpoint it hosts still reports `ACTIVE` (or `CREATING`) from `DescribeManagedEndpoint` and is still listed by `ListManagedEndpoints`, and
* every queued job run it holds is still ADMITTED TO `RUNNING` by `state::settle`, which gates the `PENDING -> RUNNING` step only on the cluster's `maxConcurrentJobRuns` and never on the cluster's own state, so `DescribeVirtualCluster` keeps reporting a non-zero `schedulerStatus.currentConcurrentJobRuns` for a cluster that has been torn down.

This contradicts the premise the crate's two sibling creates are written on: both refuse new work in a cluster that is no longer `RUNNING` precisely because such a cluster "cannot accept work" / "cannot host an endpoint". The crate also already owns the machinery to carry the children down with the parent — `ENDPOINT_TERMINATING`/`ENDPOINT_TERMINATED` constants, `PendingTransition`, `ManagedEndpointRecord::settle`, `move_job_run` — and the state module itself states that an endpoint must not be "left naming a cluster that is gone". Because the teardown never ends the cluster's non-terminal runs, the cluster also never satisfies the retired predicate `reserve_virtual_cluster` uses, so its record is never reclaimed either.

## Evidence

The teardown touches only the cluster, src/ops/delete_virtual_cluster.rs:177-186:
```
        state.move_virtual_cluster(
            &region,
            &input.id,
            crate::state::CLUSTER_TERMINATING,
            crate::state::CLUSTER_TERMINATED,
        );

        Ok(DeleteVirtualClusterOutput {
            id: Some(input.id),
```
and `state::move_virtual_cluster` (src/state.rs:1569-1585) only rewrites `record.state`/`record.pending` — no endpoint and no job run is visited.

The premise the siblings state, src/ops/create_managed_endpoint.rs:188-194: "A managed endpoint is a gateway INTO a virtual cluster, so the cluster is resolved out of state first, in the region this request is signed for: one that was never created here, or one that is no longer RUNNING because it was deleted, cannot host an endpoint." and src/ops/start_job_run.rs:181-184: "A job run runs IN a virtual cluster, so the cluster is resolved out of state first ... a cluster that was never created here, or one that is no longer RUNNING because it was deleted, cannot accept work."

Yet `settle` keeps starting that cluster's work, src/state.rs:1128-1139:
```
            let waiting_for_a_slot = self
                .job_runs
                .get(&id)
                .and_then(|record| {
                    let transition = record.pending.as_ref()?;
                    Some(
                        now >= transition.due_at
                            && transition.next_state == JOB_RUNNING
                            && !self.cluster_has_free_running_slot(&record.virtual_cluster_id),
                    )
                })
```
(the cluster's own `state` is never consulted).

The state module's own rule for the parent/child relation, src/state.rs:1279-1280: "A managed endpoint lives UNDER a virtual cluster, so the cluster's endpoints are reclaimed with it rather than left naming a cluster that is gone." — which only happens on RECLAMATION, never on the delete the caller asked for. And the endpoint terminal states the crate defines for exactly this transition are written by nothing: src/state.rs:97-103 `pub const ENDPOINT_TERMINATING: &str = "TERMINATING"; pub const ENDPOINT_TERMINATED: &str = "TERMINATED"; ... pub const ENDPOINT_TERMINAL_STATES: &[&str] = &[ENDPOINT_TERMINATED, ENDPOINT_TERMINATED_WITH_ERRORS];`, so the retired predicate in `reserve_managed_endpoint` (src/state.rs:1392 `.filter(|record| endpoint_state_is_terminal(&record.state))`) can never match.

Same for the cluster record, src/state.rs:1252-1257:
```
                .filter(|record| {
                    record.state == CLUSTER_TERMINATED
                        && self
                            .cluster_job_runs(&record.id)
                            .all(|run| job_state_is_terminal(&run.state))
```

Test coverage: tests/all/managed_endpoint_create_admission.rs:83-158 deliberately keeps an endpoint ("held") alive through its cluster's teardown and then asserts only that the cluster "still holds exactly the one endpoint it had" (line 151-158) — no test asserts the state that endpoint reports after the teardown, and no test asserts what happens to a queued run of a deleted cluster, so nothing pins the current outcome.

## Steps to Reproduce

1. POST /virtualclusters with a containerProvider -> 200, note the cluster id V.
2. POST /virtualclusters/V/endpoints (name/type/releaseLabel/executionRoleArn/clientToken) -> 200, note the endpoint id E; wait past the transition span so E settles to ACTIVE.
3. POST /virtualclusters/V/jobruns with executionRoleArn + releaseLabel + jobDriver -> 200, note the run id J.
4. DELETE /virtualclusters/V -> 200; wait past the transition span.
5. GET /virtualclusters/V -> state TERMINATED.
6. GET /virtualclusters/V/endpoints/E -> actual: 200 with "state": "ACTIVE" inside a cluster that `CreateManagedEndpoint` refuses to host an endpoint in; GET /virtualclusters/V/endpoints still lists E.
7. GET /virtualclusters/V/jobruns/J -> actual: the run was admitted to RUNNING after the cluster was torn down, and GET /virtualclusters/V reports schedulerStatus.currentConcurrentJobRuns = 1 for the TERMINATED cluster.
