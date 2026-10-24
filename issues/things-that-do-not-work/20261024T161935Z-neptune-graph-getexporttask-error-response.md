Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# Export-task ResourceNotFoundException message says "task id" instead of AWS's "export task id"

_Filed: 2026-10-24T16:19:35Z_
_Operation: GetExportTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An unknown but well-formed `taskIdentifier` is correctly refused with ResourceNotFoundException (404), but the message template diverges from the recorded AWS wording.

Expected (recorded AWS response for GetExportTask on a nonexistent task):
  `No export task found for customer [<account>] with export task id [<taskId>]`

Actual (emulator, src/state.rs:1873-1880, `export_task_not_found`):
  `No export task found for customer [<account>] with task id [<taskId>]`

AWS spells the resource word out in the identifier clause for every resource of this service (the graph template the emulator already matches is `No graph found for customer [...] with graph id [...]`), so the export-task clause must read `with export task id [...]`.

## Evidence

Spec/recording: GetExportTask models ResourceNotFoundException @httpError 404 (smithy_query(query="operation",operation="GetExportTask")) and `taskIdentifier` is pattern ^t-[a-z0-9]{10}$, so an unknown well-formed id reaches resource lookup. The recorded AWS message is quoted verbatim in this repo's open issue 20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong.md (cause 4, trace `neptune-graph.import-export-task-nonexistent-not-found`): "'No export task found for customer [<account>] with export task id [<taskId>]'". That issue's gap was the 501 scaffold; GetExportTask is now implemented (src/ops/get_export_task.rs:176-181) and answers the right code, leaving only the wording divergent.

Handler: src/state.rs:1873-1880 formats "No export task found for customer [{}] with task id [{task_identifier}]".

Test coverage: no test asserts this message (grep for "with task id" matches only src/state.rs); tests/all/export_task_lifecycle.rs:704 asserts the status/code only.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. Call GetExportTask(taskIdentifier='t-0000000000').
3. Observe 404 ResourceNotFoundException with message 'No export task found for customer [<account>] with task id [t-0000000000]'; AWS returns '... with export task id [t-0000000000]'.
