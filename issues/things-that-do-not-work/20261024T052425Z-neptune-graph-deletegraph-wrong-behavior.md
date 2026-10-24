Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# DeleteGraph refuses skipSnapshot=false with 501 and makes the graph vanish instead of reporting DELETING

_Filed: 2026-10-24T05:24:25Z_
_Operation: DeleteGraph | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two divergences on the implemented DeleteGraph handler (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/delete_graph.rs):

1. `skipSnapshot=false` is not supported. The modeled input member `skipSnapshot` is @required and its documentation states: "If `true` is specified, no graph snapshot is created. If `false` is specified, a graph snapshot is created before the graph is deleted." Expected (AWS): DeleteGraph(graphIdentifier=g-..., skipSnapshot=false) succeeds with HTTP 200, the response reports the graph with status DELETING, and a final graph snapshot of the graph is created (visible to GetGraphSnapshot/ListGraphSnapshots). Actual: the handler short-circuits with `crate::scaffolded::Scaffolded::not_implemented("DeleteGraph with skipSnapshot=false (a final snapshot)")` (delete_graph.rs:217-221), i.e. HTTP 501 `NotImplemented` — an unmodeled status/code for this operation — and the graph is left intact. Every `aws neptune-graph delete-graph --no-skip-snapshot` call therefore fails. The handler's own comment justifies this by saying snapshots "are their own (still scaffolded) slice", but CreateGraphSnapshot and the snapshot store in `crate::state` are now implemented, so the final snapshot can be minted from the same code path (mint a `gs-` id, name it after the graph, store it with status CREATING settling to AVAILABLE).

2. The deletion is instantaneous rather than a transition. Expected (AWS): the accepted delete moves the graph to DELETING and the graph remains observable for the duration of the deletion — GetGraph answers 200 with status DELETING, ListGraphs still lists it, and its graphName stays reserved (a CreateGraph re-using the name while the old graph is DELETING conflicts) until deletion finishes, after which GetGraph answers ResourceNotFoundException. Actual: the handler calls `store.remove_graph(...)` inside the same request (delete_graph.rs:223-235) and only stamps `status = DELETING` on the detached copy it returns, so the very next GetGraph answers 404, ListGraphs omits the graph immediately, and the name is immediately re-usable. This is inconsistent with the emulator's own `PendingTransition`/`PROVISIONING_SPAN` machinery, which already models CREATING/UPDATING/STARTING/STOPPING as clock-owed transitions — DELETING is the one transition that is skipped.

## Evidence

Spec: Smithy model `com.amazonaws.neptunegraph#DeleteGraph` — `DELETE /graphs/{graphIdentifier} -> 200`, input `skipSnapshot` (boolean, @required, @httpQuery(skipSnapshot)) documented as "If `false` is specified, a graph snapshot is created before the graph is deleted"; output carries `status` (GraphStatus, which includes DELETING). 501/NotImplemented is not among DeleteGraph's modeled errors (ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400).

Handler: src/ops/delete_graph.rs:211-221 returns `Scaffolded::not_implemented` whenever `!input.skip_snapshot`; lines 223-235 remove the record from the store within the request and set DELETING only on the returned copy.

Test coverage: tests/all/graph_lifecycle.rs:804-823 currently ASSERTS the divergent behavior (`assert_eq!(status, 501, "case skipSnapshot=false: ...")`) and lines 769-802 assert that GetGraph 404s, ListGraphs omits the graph and the name is immediately free after the delete — so these tests must be updated along with the handler. No test asserts the AWS behavior (200 + DELETING + final snapshot).

Not covered by the existing open issue `20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong.md`, whose DeleteGraph cause is only about the deletion-protection refusal using ConflictException instead of ValidationException (all of its DeleteGraph reproduction steps pass skipSnapshot=True).

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName='final-snap-graph', provisionedMemory=16); poll GetGraph until status is AVAILABLE.
3. Call DeleteGraph(graphIdentifier=<id>, skipSnapshot=False) — i.e. DELETE /graphs/<id>?skipSnapshot=false. Observe HTTP 501 with code 'NotImplemented'; expected HTTP 200 with status DELETING plus a final snapshot retrievable via GetGraphSnapshot/ListGraphSnapshots.
4. Call DeleteGraph(graphIdentifier=<id>, skipSnapshot=True). Observe HTTP 200 with status DELETING.
5. Immediately call GetGraph(graphIdentifier=<id>): observe 404 ResourceNotFoundException; expected 200 with status DELETING while the deletion is in flight (and 404 only after it completes).
6. Immediately call ListGraphs and CreateGraph(graphName='final-snap-graph', provisionedMemory=16): observe the graph already absent from the listing and the name immediately re-usable (201); expected the graph still listed in DELETING and the name still reserved (ConflictException) until the deletion finishes.
