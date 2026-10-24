Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# Import-task ResourceNotFoundException message says "task id" instead of AWS's "import task id"

_Filed: 2026-10-24T16:19:35Z_
_Operation: GetImportTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An unknown but well-formed `taskIdentifier` is correctly refused with ResourceNotFoundException (404), but the message template diverges from the recorded AWS wording by one word.

Expected (recorded AWS response for GetImportTask / CancelImportTask on a nonexistent task):
  `No import task found for customer [<account>] with import task id [<taskId>]`

Actual (emulator, src/state.rs:1842-1849, `import_task_not_found`):
  `No import task found for customer [<account>] with task id [<taskId>]`

The resource word is spelled out in AWS's identifier clause for every resource of this service — the graph template the emulator already matches is `No graph found for customer [...] with graph id [...]` — so the import-task clause must read `with import task id [...]`, not `with task id [...]`.

The same helper is the refusal path of BOTH GetImportTask and CancelImportTask (ops/get_import_task.rs:195-199 and ops/cancel_import_task.rs:186-190 both call `crate::state::import_task_not_found`), so one template fix covers both operations.

## Evidence

Spec/recording: GetImportTask and CancelImportTask both model ResourceNotFoundException @httpError 404 (smithy_query(query="operation",operation="GetImportTask")), and `taskIdentifier` is pattern ^t-[a-z0-9]{10}$ so `t-0000000000` reaches resource lookup. The recorded AWS message is quoted verbatim in this repo's own open issue 20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong.md (cause 4, trace `neptune-graph.import-export-task-nonexistent-not-found`): "'No import task found for customer [<account>] with import task id [<taskId>]'". That issue's gap was the 501 scaffold; the handler is now implemented and answers the right code, so only the wording remains divergent.

Handler: src/state.rs:1842-1849 formats "No import task found for customer [{}] with task id [{task_identifier}]". Compare src/state.rs graph_not_found, which does spell the resource word ("with graph id") and is asserted in tests/all/error_message_rendering.rs:347.

Test coverage: no test asserts the import-task not-found message (grep for "with task id" matches only src/state.rs); tests/all/import_task_lifecycle.rs asserts the 404 status/code only, so the wording is uncovered.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. Call GetImportTask(taskIdentifier='t-0000000000').
3. Observe 404 ResourceNotFoundException with message 'No import task found for customer [<account>] with task id [t-0000000000]'; AWS returns '... with import task id [t-0000000000]'.
4. Repeat with CancelImportTask(taskIdentifier='t-0000000000') — same message from the same helper.
