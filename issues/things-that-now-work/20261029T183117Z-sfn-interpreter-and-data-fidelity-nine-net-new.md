Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.jsonata-output-and-states-input-transform stepfunctions.choice-jsonata-condition-reads-assigned-variables stepfunctions.deep-sfn-2-sfn-jsonata-map-items-expression-and-item-context stepfunctions.deep-sfn-2-sfn-jsonata-mixed-query-language-state-override stepfunctions.deep-sfn-1-sfn-sync-child-fail-surfaces-as-task-failed-in-parent stepfunctions.stop-execution-aborted-event-details-with-and-without-cause stepfunctions.stop-execution-oversized-error-rejected-execution-keeps-running stepfunctions.reject-update-state-machine-on-version-arn stepfunctions.list-executions-missing-required-filter-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.jsonata-output-and-states-input-transform|run#5|AssertionError|assert big["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.choice-jsonata-condition-reads-assigned-variables|run#6|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-map-items-expression-and-item-context|run#6|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-mixed-query-language-state-override|run#5|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-child-fail-surfaces-as-task-failed-in-parent|run#19|JSONDecodeError|inner = json.loads(d["cause"])
Issue-Key: stepfunctions.stop-execution-aborted-event-details-with-and-without-cause|_wait_entered#6|Timeout|time.sleep(1)
Issue-Key: stepfunctions.stop-execution-oversized-error-rejected-execution-keeps-running|_wait_entered#5|Timeout|time.sleep(1)
Issue-Key: stepfunctions.reject-update-state-machine-on-version-arn|run#18|AssertionError|assert code == "StateMachineDoesNotExist", (
Issue-Key: stepfunctions.list-executions-missing-required-filter-rejected|run#11|AssertionError|assert message == "Must provide a StateMachine ARN or MapRun ARN", f"unexpected ValidationException message {message!r}"
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sfn interpreter and data fidelity: the nine checks that now pass — JSONata evaluation, StopExecution's observable contracts, the run-a-job failure cause, and the two control-plane classifications (RESOLVED)

> _This report is the RESOLVED half of the nine-cause bundle originally filed as
> `20261029T183117Z-sfn-interpreter-and-data-fidelity-nine-net-new.md` against commit
> `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. The causes of that bundle that are still
> open are recorded on their own, with their own checks, in
> `issues/things-that-do-not-work/20261029T183117Z-sfn-interpreter-and-data-fidelity-five-causes-still-open.md`
> — nothing was closed that still diverges._

## Operation
StartExecution

## Description
Four distinct defects of the original bundle are resolved; the nine checks above
replay clean.

Resolved 1 — JSONata definitions are evaluated. A state machine whose
`QueryLanguage` is `JSONata`, whole-machine or as a per-state override, is run: the
`Arguments`, `Output`, `Assign`, `Condition` and `Items` expressions are evaluated
through the service crate's bounded seam onto the maintained JSONata interpreter
(`services/service-com-amazonaws-sfn-awsstepfunctions/src/state/jsonata.rs` over
`services/common`'s `jsonata`), and `$states.input`, `$states.context`,
`$states.result` and the assigned variables are bound where the language binds them.
  Checks: `.jsonata-output-and-states-input-transform`,
  `.choice-jsonata-condition-reads-assigned-variables`,
  `.deep-sfn-2-sfn-jsonata-map-items-expression-and-item-context`,
  `.deep-sfn-2-sfn-jsonata-mixed-query-language-state-override`.

Resolved 2 — StopExecution's observable contracts converge. A successful stop writes
an `ExecutionAborted` history event whose `executionAbortedEventDetails` carry the
submitted `error` and `cause` (and omit `cause` when it was not given), and a value
past the modelled `@length` on either member is refused with `ValidationException`
before the execution is touched, so the execution keeps running.
  Checks: `.stop-execution-aborted-event-details-with-and-without-cause`,
  `.stop-execution-oversized-error-rejected-execution-keeps-running`.

Resolved 3 — a run-a-job (`.sync`) task that FAILED reports the invoked service's own
description of the job as its `cause`. The task's `cause` was the single member the
emulator picked out of the reporting call's answer (a nested execution's own `cause`
text, Athena's `StateChangeReason`, Glue's `JobRun.ErrorMessage`), which a client
cannot decode as the JSON document AWS puts there. It is now the JSON text of the
whole report — the same document a job that DID its work reports as the task's
result, in the integration's own PascalCase convention — so a `Catch` reads the
job's status, identity, error and reason out of one document
(`src/state/integration.rs`, `RunAJob::outcome_of`).
  Check: `.deep-sfn-1-sfn-sync-child-fail-surfaces-as-task-failed-in-parent`.

Resolved 4 — the two control-plane classifications. `UpdateStateMachine` addresses
the state machine RECORD, which only an unqualified ARN names: an ARN qualified with
a published version or an alias names a different resource, so the request is now
refused with the modelled `StateMachineDoesNotExist` and this service's one
resource-refusal sentence (`State Machine Does Not Exist: '<arn>'`) rather than a
`ValidationException` of the emulator's own wording — the Distributed-Map LABEL
qualifier keeps the `ValidationException` its own documentation states.
`ListExecutions` naming neither selector now carries the service's fixed sentence
`Must provide a StateMachine ARN or MapRun ARN`.
  Checks: `.reject-update-state-machine-on-version-arn`,
  `.list-executions-missing-required-filter-rejected`.

## Evidence
All nine checks above replay clean. The rules are pinned by
`services/service-com-amazonaws-sfn-awsstepfunctions/tests/all/control_plane_refusal_classes.rs`
(the two classifications, over tables spanning the qualifier class, whether the
qualified resource exists, the member an update names, and every other member a
listing may carry), by `tests/all/sync_service_integration.rs` and
`core/router/tests/all/sfn_glue_run_a_job_sync.rs` (the failure cause, read back
against what the invoked service itself reports for that job), by
`tests/all/jsonata_states.rs` and by the StopExecution suites.
