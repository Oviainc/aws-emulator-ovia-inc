Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# StartImportTask's graph-status refusal is hand-written instead of the shared state-gate builder that crate::state says words it

_Filed: 2026-10-28T07:20:53Z_
_Operation: StartImportTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartImportTask` refuses the two halves of one documented precondition — "The graph needs to be empty and in the AVAILABLE state" — through two different wordings: the EMPTINESS half goes through the service's shared builder `crate::state::graph_not_empty_conflict()` ("Unable to start the import task because the graph is not empty."), while the STATUS half, ten lines above it in the same handler, is a hand-written sentence in a different template that also fronts the graph id ("graph g-xxxxxxxxxx is in status CREATING and cannot accept an import task").

That is not merely a stylistic split: `crate::state`'s own documentation for the emptiness builder asserts that the status half is worded by `graph_state_conflict`, and the documentation of `graph_state_conflict` states that it is "Written once here and reached by every one of those state gates, so no two of them word the same refusal differently" and that "the graph was addressed by an identifier the caller sent, so the refusal does not front it". The handler reaches neither the builder nor its template, and does front the identifier. So the crate states a rule for this refusal in one place and breaks it in the other: one documented precondition sentence, two refusal grammars, and a state-gate message that the crate's own rule says should not carry the graph id.

The same hand-written template appears on three more graph-status gates (`create_graph_snapshot.rs:283-294`, `start_export_task.rs:272-283`, `create_private_graph_endpoint.rs:255-267`); this issue is filed against `StartImportTask` because it is the one whose refusal `crate::state` explicitly claims the shared builder words, and the one that words the two halves of a single documented sentence two different ways inside one handler.

## Evidence

The rule, stated in crate::state — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:2805-2816 (doc of `graph_not_empty_conflict`):

    /// The modelled `ConflictException` for an import into a graph that still HOLDS DATA.
    ///
    /// `StartImportTask` states two preconditions on the graph it loads into — "The graph
    /// needs to be empty and in the AVAILABLE state"
    /// (`smithy_query(query="doc",shape="StartImportTask")`) — and this is the refusal of the
    /// emptiness half, the sibling of the status half [`graph_state_conflict`] words

and the builder it names — src/state.rs:2778-2803:

    /// `action` is the verb of the operation asking for the transition ("start", "stop",
    /// "update", "delete", "reset"), and `status` is the status that held the request back.
    /// The service words this refusal as what it could not do and the state that stopped it
    /// — and nothing else: the graph was addressed by an identifier the caller sent, so the
    /// refusal does not front it ... Written once here and reached by every one of those
    /// state gates, so no two of them word the same refusal differently.
    pub fn graph_state_conflict(action: &str, status: GraphStatus) -> ConflictException {
        ConflictException {
            message: format!(
                "Unable to {action} the graph because it is in the {} state.",
                status.as_wire()
            ),

The code that breaks it — src/ops/start_import_task.rs:302-317:

        // "The graph needs to be empty and in the AVAILABLE state" — the STATUS half of
        // that sentence: ...
        if graph_status != crate::state::GraphStatus::Available {
            return Err(StartImportTaskErrorModeled::ConflictException(
                ConflictException {
                    message: format!(
                        "graph {graph_id} is in status {} and cannot accept an import task",
                        graph_status.as_wire()
                    ),
                    reason: None,
                },
            )
            .into());
        }

and, immediately below it, the OTHER half of the same sentence going through the shared builder — src/ops/start_import_task.rs:327-332:

        if !graph_is_empty {
            return Err(StartImportTaskErrorModeled::ConflictException(
                crate::state::graph_not_empty_conflict(),
            )
            .into());
        }

The five handlers that do reach the shared builder: delete_graph.rs:213-218, reset_graph.rs:205-210, update_graph.rs:194-199, start_graph.rs:192-197, stop_graph.rs:191-196 (each `crate::state::graph_state_conflict("<verb>", status)`).

Test coverage: tests/all/import_requires_an_empty_graph.rs:66 pins the emptiness half (`const NOT_EMPTY_MESSAGE: &str = "Unable to start the import task because the graph is not empty."`); tests/all/graph_state_gate_messages.rs:99-101 pins the shared template for the five graph-transitioning operations. NO test pins the hand-written status sentence of start_import_task.rs:309-312, so nothing in the crate justifies the second grammar.

## Steps to Reproduce

1. Start the emulator and `POST /graphs` with `{"graphName": "import-gate", "provisionedMemory": 16}`; note the returned `id` (the graph reports CREATING for crate::state::PROVISIONING_SPAN).
2. Immediately `POST /importtasks` with `{"graphIdentifier": "<id>", "source": "s3://bucket/prefix/", "roleArn": "arn:aws:iam::000000000000:role/NeptuneLoadFromS3"}`.
3. Observe the 409 message: "graph <id> is in status CREATING and cannot accept an import task" — a template no other refusal of this service uses, fronting the graph id, where crate::state::graph_state_conflict (which state.rs:2810 says words this half) renders "Unable to start the import task because it is in the CREATING state."-style sentences without the identifier.
4. For contrast, `POST /graphs/<id>/start` on the same graph answers the shared template ("Unable to start the graph because it is in the CREATING state."), and the emptiness half of the very same StartImportTask precondition answers "Unable to start the import task because the graph is not empty."
