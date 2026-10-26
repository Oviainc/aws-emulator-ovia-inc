Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# Graph state-gate ConflictException messages are emulator-authored, not AWS's "Unable to start the graph because it is in the <STATUS> state."

_Filed: 2026-10-24T16:19:35Z_
_Operation: StartGraph | Class: error-response_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartGraph now correctly refuses a graph that is not STOPPED with ConflictException (409), but the message is the emulator's own sentence rather than the recorded AWS wording.

Expected (recorded AWS response for StartGraph on an AVAILABLE graph):
  `Unable to start the graph because it is in the AVAILABLE state.`

Actual (src/ops/start_graph.rs:192-202):
  `graph g-xxxxxxxxxx is in status AVAILABLE and cannot be started`

The same emulator-authored template is used for every sibling state gate on a graph, so the fix should cover the family that shares the shape of this refusal:
* StopGraph (src/ops/stop_graph.rs:191-201) — `graph <id> is in status <S> and cannot be stopped`, where AWS's mirrored sentence is `Unable to stop the graph because it is in the <S> state.`
* UpdateGraph (src/ops/update_graph.rs:194-204) — `graph <id> is in status <S> and cannot be updated`.

A client or operator matching the documented/recorded refusal text does not recognise any of these, and the emulator sentence additionally fronts the graph id in a position AWS does not use.

## Evidence

Spec: StartGraph/StopGraph/UpdateGraph each model ConflictException @httpError 409 (smithy_query(query="operation",operation="UpdateGraph") shows ConflictException 409 among the errors). The recorded AWS message for the StartGraph state gate is quoted verbatim in this repo's open issue 20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong.md, cause 3 (trace `neptune-graph.neptune-graph-start-graph-wrong-status-rejected`): "recorded message 'Unable to start the graph because it is in the AVAILABLE state.'". That cause's blocking symptom was the 501 scaffold, which has since been implemented to return the correct 409 — the message rule is what still diverges.

Handler: src/ops/start_graph.rs:192-202 builds `format!("graph {} is in status {} and cannot be started", graph.id, graph.status.as_wire())`; stop_graph.rs:191-201 and update_graph.rs:194-204 build the mirrored sentences.

Test coverage: tests/all/graph_run_state.rs:362 (`stop_and_start_refuse_a_graph_whose_status_the_transition_cannot_start_from`) asserts the 409 status and the ConflictException code only — grep for "Unable to start" in the crate returns nothing, so no test pins the message.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName='start-gate-probe', provisionedMemory=16) and poll GetGraph until status AVAILABLE.
3. Call StartGraph(graphIdentifier=<id>).
4. Observe 409 ConflictException with message 'graph <id> is in status AVAILABLE and cannot be started'; AWS returns 'Unable to start the graph because it is in the AVAILABLE state.'
5. Call StopGraph twice (the second while the graph is STOPPED) and UpdateGraph on the STOPPED graph to see the same emulator-authored template on the sibling gates.
