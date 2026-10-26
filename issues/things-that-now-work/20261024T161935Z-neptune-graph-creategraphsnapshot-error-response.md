Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# Duplicate snapshotName conflict uses an emulator-authored message that also discloses the colliding snapshot's id

_Filed: 2026-10-24T16:19:35Z_
_Operation: CreateGraphSnapshot | Class: error-response_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateGraphSnapshot with a `snapshotName` the account already holds in the Region is correctly refused with ConflictException (409), but the message is the emulator's own sentence and it leaks the id of the snapshot the caller was not told about.

Actual (src/ops/create_graph_snapshot.rs:233-244):
  `a snapshot named 'taken-name' already exists with id gs-xxxxxxxxxx`

Expected: a refusal derived only from the request's `snapshotName`, in the shape AWS uses for this service's duplicate-name conflicts — the recorded CreateGraph analogue is `Graph with name <graphName> already exists!`, i.e. request-derived and with no disclosure of the colliding resource's id. The snapshot refusal should follow the same rule (`Snapshot with name <snapshotName> already exists!`) rather than an emulator sentence.

This is the third site of the same defect: the CreateGraph instance is open as cause 2 of 20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong.md and the CreateGraphUsingImportTask instance is open as 20261024T153312Z-neptune-graph-creategraphusingimporttask-error-response.md; the snapshot surface has the same emulator-authored template and the same id disclosure, and is not covered by either.

## Evidence

Spec: CreateGraphSnapshot models ConflictException @httpError 409 (smithy_query(query="operation",operation="CreateGraphSnapshot")), and `snapshotName` is the request member the uniqueness check is keyed on (pattern ^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$). The AWS message rule for a duplicate-name conflict in this service is recorded for CreateGraph as 'Graph with name <graphName> already exists!' (quoted in the open 7-cause issue, trace `neptune-graph.neptune-graph-create-graph-duplicate-name-conflict-rejected`), which is request-derived and discloses no colliding id.

Handler: src/ops/create_graph_snapshot.rs:233-244 formats "a snapshot named '{}' already exists with id {existing}" from `store.snapshot_id_named(...)`, so the response carries a resource identifier the caller never asked for.

Test coverage: tests/all/graph_snapshot_lifecycle.rs:358-402 (`a_snapshot_name_already_in_the_account_conflicts`) asserts only the 409 status and the ConflictException code — the message text is unasserted.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName='snapshot-conflict-src', provisionedMemory=16); poll GetGraph until AVAILABLE.
3. CreateGraphSnapshot(graphIdentifier=<id>, snapshotName='taken-name') -> 201.
4. Wait for the graph to return to AVAILABLE, then CreateGraphSnapshot(graphIdentifier=<id>, snapshotName='taken-name') again.
5. Observe 409 ConflictException whose message is "a snapshot named 'taken-name' already exists with id gs-..." — an emulator-authored sentence that also discloses the existing snapshot's id, instead of a request-derived 'Snapshot with name taken-name already exists!'.
