Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# CreateGraphUsingImportTask's duplicate-graphName conflict uses an emulator-authored message that also discloses the colliding graph's id

_Filed: 2026-10-24T15:33:12Z_
_Operation: CreateGraphUsingImportTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateGraphUsingImportTask enforces the same per-account/per-Region `graphName` uniqueness rule CreateGraph enforces, and AWS reports the collision with the same request-derived ConflictException message, built from the requested name alone: 'Graph with name <graphName> already exists!'.

The handler instead formats its own sentence and appends the id of the graph that already holds the name: "a graph named '<graphName>' already exists with id <graphId>". The HTTP status and error code (ConflictException/409) are right; only the message rule diverges. Two consequences: a client or operator matching the documented AWS text does not recognise the refusal, and the response discloses an existing resource's identifier that the caller was not told about and may not be entitled to see.

Expected: ConflictException/409 whose message is derived from the requested `graphName` only. Actual: an emulator-authored sentence carrying the colliding graph id.

## Evidence

Spec: smithy_query(operation="CreateGraphUsingImportTask") declares `graphName` (len 1..63, pattern ^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$) as required and ConflictException (@httpError 409) among its errors; the name-uniqueness rule is the same one CreateGraph applies (both handlers call crate::state::graph_id_named(name, region)).
Handler: services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/create_graph_using_import_task.rs lines ~309-320 — `message: format!("a graph named '{}' already exists with id {existing}", input.graph_name)`.
Backlog: the open issue '20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong' records the AWS message rule for this collision (its cause 2: expected 'Graph with name <name> already exists!', got the same emulator sentence with the graph id), but it is scoped to src/ops/create_graph.rs and the CreateGraph trace only; this second copy of the same message, in the CreateGraphUsingImportTask handler, is a separate code path that the fix there does not reach.
Test coverage: tests/all/create_graph_using_import_task.rs asserts that a duplicate name is refused with ConflictException/409 but does not assert the message text, so nothing pins the AWS wording.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName=dup-probe, provisionedMemory=16).
3. CreateGraphUsingImportTask(graphName='dup-probe', source='s3://my-bucket/data/', roleArn='arn:aws:iam::000000000000:role/import-role', format='CSV').
4. Observe ConflictException/409 with message "a graph named 'dup-probe' already exists with id g-xxxxxxxxxx"; expected 'Graph with name dup-probe already exists!' with no reference to the existing graph's id.
