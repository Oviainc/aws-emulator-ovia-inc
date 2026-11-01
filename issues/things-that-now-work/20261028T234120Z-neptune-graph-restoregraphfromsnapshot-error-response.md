Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# RestoreGraphFromSnapshot gates on the source snapshot's status BEFORE the graphName conflict, inverting the precedence its two sibling minting handlers state and justify

_Filed: 2026-10-28T23:41:20Z_
_Operation: RestoreGraphFromSnapshot | Class: error-response_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers in this crate mint a resource whose request carries (a) a name that must be free in the Region and (b) a precondition on the status of the SOURCE resource it is minted from. Two of them — `CreateGraphSnapshot` and `CreatePrivateGraphEndpoint` — deliberately check the duplicate FIRST and say so in a comment, with the stated reason that a duplicate must be reported as the conflict it is "whatever the graph is doing". `RestoreGraphFromSnapshot` does the opposite: it gates on the source snapshot's status first and only then looks the `graphName` up in the Region's name index.

So a restore that is wrong in BOTH ways — a `graphName` the Region already holds, named off a snapshot whose creation has not yet settled — is answered with the snapshot-state conflict ("…cannot restore the graph snapshot because it is in the CREATING state") rather than the graph-name conflict every other graph-minting operation answers for that name (`CreateGraph`, `CreateGraphUsingImportTask`, and this same handler once the snapshot settles). The caller fixes the snapshot wait, retries, and is then told about the name — two round trips for a request whose name was never acceptable, and a refusal that reports the condition the crate's own siblings say is the lower-priority one.

The handler's own comment on the status gate makes the inconsistency explicit: it claims the gate is "the same precondition `CreateGraphSnapshot` states about the graph it snapshots" — but `CreateGraphSnapshot` states that precondition AFTER the name conflict, on purpose.

## Evidence

The rule, stated and justified by sibling #1 — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/create_graph_snapshot.rs:240-256`:
```
        // `snapshotName` is unique within the account PER REGION: a second snapshot by
        // the same name conflicts with a stored snapshot of the same Region, while the
        // same name in another Region is another snapshot. Checked before the source
        // graph's status so a duplicate name is reported as the name conflict it is,
        // whatever the graph is doing. ...
        if store
            .snapshot_id_named(&input.snapshot_name, &region)
            .is_some()
        {
            return Err(CreateGraphSnapshotErrorModeled::ConflictException(
                crate::state::snapshot_name_conflict(&input.snapshot_name),
```
with its source-status gate only afterwards, `src/ops/create_graph_snapshot.rs:286-294`:
```
        if source_graph_status != crate::state::GraphStatus::Available {
            return Err(CreateGraphSnapshotErrorModeled::ConflictException(
                crate::state::graph_status_precondition_conflict(
                    "create the graph snapshot",
                    source_graph_status,
                ),
```

The same rule, stated by sibling #2 — `src/ops/create_private_graph_endpoint.rs:211-216`:
```
        // One endpoint per VPC per graph: `(graphIdentifier, vpcId)` is the identity
        // `GetPrivateGraphEndpoint` and `DeletePrivateGraphEndpoint` address an endpoint
        // by, so a second endpoint in a VPC that already has one conflicts with the
        // stored record. Checked before the graph's status so a duplicate is reported as
        // the conflict it is, whatever the graph is doing.
        if let Some(existing) = store.settled_private_endpoint(&graph_id, &vpc_id, now) {
```
its graph-status gate following at `src/ops/create_private_graph_endpoint.rs:258-266`.

The handler that breaks it — `src/ops/restore_graph_from_snapshot.rs:273-297`, status gate first, name conflict second:
```
        // Only a complete snapshot can be restored: while its creation is still
        // outstanding the snapshot holds no usable copy of the data, and one whose
        // deletion has been accepted is on its way out. That is the same precondition
        // `CreateGraphSnapshot` states about the graph it snapshots, and it is worded by
        // the one builder every snapshot state gate in this service reaches.
        if snapshot_status != crate::state::SnapshotStatus::Available {
            return Err(RestoreGraphFromSnapshotErrorModeled::ConflictException(
                crate::state::snapshot_state_conflict("restore", snapshot_status),
            )
            .into());
        }

        // `graphName` is unique per account PER REGION, exactly as it is for
        // `CreateGraph`: ...
        if store.graph_id_named(&input.graph_name, &region).is_some() {
            return Err(RestoreGraphFromSnapshotErrorModeled::ConflictException(
                crate::state::graph_name_conflict(&input.graph_name),
            )
            .into());
        }
```

Test coverage: nothing pins the current order. `tests/all/graph_restore_from_snapshot.rs:520-558` (`a_snapshot_whose_creation_is_still_in_flight_cannot_be_restored`) uses a FREE name (`"too-early-restore"`), and `tests/all/duplicate_name_conflicts.rs:252-304` asserts the name conflict for `RestoreGraphFromSnapshot` only against a snapshot its helper has already waited into service (`tests/all/duplicate_name_conflicts.rs:212-213`: `wait_until_available(router, case, &format!("/snapshots/{id}")).await;`). No test exercises the two conditions together, so no assertion changes either way.

## Steps to Reproduce

1. CreateGraph with graphName `restore-target`, provisionedMemory 16; wait out PROVISIONING_SPAN so it reports AVAILABLE.
2. CreateGraphSnapshot on that graph with snapshotName `capture`; wait both the graph and the snapshot back to AVAILABLE, then RestoreGraphFromSnapshot from `capture` with graphName `restore-target` — 409 ConflictException naming the taken graph name (the baseline).
3. CreateGraphSnapshot on the same graph again with snapshotName `capture-two`; the response reports status CREATING.
4. IMMEDIATELY (inside PROVISIONING_SPAN) POST /snapshots/{capture-two-id}/restore with `{"graphName": "restore-target"}` — a name the Region already holds.
5. Observe the 409 message is the snapshot-state refusal (`restore the graph snapshot` / `it is in the CREATING state`) instead of the graph-name conflict of step 2, although both sibling minting handlers state that the duplicate is reported first "whatever the graph is doing".
6. For the contrast, drive the sibling: put a graph into SNAPSHOTTING (CreateGraphSnapshot on it) and immediately CreateGraphSnapshot again with a snapshotName that is already taken — the name conflict is answered, not the source-status conflict.
