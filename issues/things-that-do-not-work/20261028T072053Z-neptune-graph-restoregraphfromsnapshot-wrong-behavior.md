Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# RestoreGraphFromSnapshot ignores the modelled "Default =1" for an omitted replicaCount and reports the snapshot's captured value instead

_Filed: 2026-10-28T07:20:53Z_
_Operation: RestoreGraphFromSnapshot | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RestoreGraphFromSnapshotInput$replicaCount` is the one configuration member of this operation whose model documentation states a DEFAULT for an omitted value: "The number of replicas in other AZs. Min =0, Max = 2, Default =1". The handler never applies that default: when the request omits `replicaCount` it stores whatever replica count the SNAPSHOT captured from the source graph, so a restore of a snapshot taken from a graph with `replicaCount` 0 or 2 mints (and every later GetGraph/ListGraphs reports) 0 or 2 where the model documents 1.

This is also inconsistent inside the crate: the two sibling handlers that mint a graph read the SAME documented sentence and encode it as a constant. `create_graph.rs` and `create_graph_using_import_task.rs` each define `DEFAULT_REPLICA_COUNT: i32 = 1`, quoting the member doc as their source, and apply it on an absent member. Only the restore path substitutes a different value for the same absent member.

Note the scope: the restore handler's "an omitted member is restored as the snapshot captured it" rule is sound for the members whose docs state NO default (`provisionedMemory` — "Min = 16" only, `publicConnectivity`, `deletionProtection`) and for `vectorSearchConfiguration`, which the request cannot carry at all. `replicaCount` is the single member where that rule overrides a default the model spells out.

## Evidence

The model sentence, from the member itself — `smithy_query(query="doc",shape="RestoreGraphFromSnapshotInput",member="replicaCount")`:

    "<p>The number of replicas in other AZs. Min =0, Max = 2, Default =1</p> <important> <p> Additional charges equivalent to the m-NCUs selected for the graph apply for each replica. </p> </important>"

The handler, which never reaches a default at all — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/restore_graph_from_snapshot.rs:359-361:

            replica_count: input
                .replica_count
                .unwrap_or(captured_configuration.replica_count),

The same sentence, honoured by the sibling that mints a graph — src/ops/create_graph.rs:306 and src/ops/create_graph.rs:333-337:

            replica_count: input.replica_count.unwrap_or(DEFAULT_REPLICA_COUNT),
    ...
    /// The number of replicas a graph gets when `replicaCount` is not supplied.
    ///
    /// From the member's own documentation: "The number of replicas in other AZs. Min =0,
    /// Max = 2, Default = 1."
    const DEFAULT_REPLICA_COUNT: i32 = 1;

and by the third graph-minting handler — src/ops/create_graph_using_import_task.rs:424 and :507-511:

            replica_count: input.replica_count.unwrap_or(DEFAULT_REPLICA_COUNT),
    ...
    /// From the member's own documentation: "The number of replicas in other AZs to
    /// provision on the new graph after import. Default = 1, Min = 0, Max = 2."
    const DEFAULT_REPLICA_COUNT: i32 = 1;

Test coverage: tests/all/graph_restore_from_snapshot.rs pins the divergent behaviour, and states its reason, at :368-380 — "The rule itself: an overridden member reports the request's value, an omitted one reports what the snapshot captured." — over `OVERRIDABLE_MEMBERS` (:65-70), which includes "replicaCount". The case at :265-273 ("only the memory overridden, to the published upper bound, an encrypted source") creates the source graph with `"replicaCount": MAX_REPLICA_COUNT` (= 2, :53) and restores it overriding only `provisionedMemory`, so the assertion requires the restored graph to report 2 for the omitted member. No test exercises the documented default on this path. The model sentence quoted above contradicts that stated reason for this one member.

## Steps to Reproduce

1. Start the emulator and `POST /graphs` with `{"graphName": "replica-source", "provisionedMemory": 16, "replicaCount": 2}`; wait for GetGraph to report AVAILABLE.
2. `POST /snapshots` with `{"graphIdentifier": "<id>", "snapshotName": "replica-snapshot"}` and wait for the snapshot to report AVAILABLE.
3. `POST /snapshots/<snapshotId>/restore` with a body carrying only `{"graphName": "replica-restored"}` — i.e. omit `replicaCount`.
4. Read the response (and a later `GET /graphs/<newId>`): `replicaCount` is 2, the value the snapshot captured, where the member's own documentation states "Default =1".
5. Repeat with a source graph created with `"replicaCount": 0` — the restore reports 0 for the omitted member.
