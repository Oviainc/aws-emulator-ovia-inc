Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# StartSuiteRun never consults suiteRunConfiguration.parallelRun when refusing a concurrent run

_Filed: 2026-10-25T14:37:21Z_
_Operation: StartSuiteRun | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate implements a hard "one run at a time per suite definition" rule in StartSuiteRun: any run of the named definition that is not in a terminal status makes the next StartSuiteRun fail with ConflictException. The model's own `SuiteRunConfiguration$parallelRun` member is the input that governs exactly this — the model states "TRUE if multiple test suites run in parallel" — and the handler never reads it. `parallel_run` exists on the deserialized configuration struct and is persisted/echoed verbatim by GetSuiteRun, but no code path in `src/` ever looks at its value, so a caller that asks for a parallel run gets the same unconditional ConflictException as one that does not. The field is therefore decorative: it round-trips through reads but has no effect on the one behaviour it is documented to control.

Consolidated symptoms of this one class: (a) StartSuiteRun with `parallelRun: true` is refused with ConflictException while an earlier run of the same definition is PENDING/RUNNING/STOPPING; (b) `parallelRun: false` and an absent `parallelRun` behave identically to `parallelRun: true`, so the member can never be observed to do anything.

## Evidence

Smithy model (`smithy_query(query="shape",shape="SuiteRunConfiguration")`): member `parallelRun` -> `{"target": "ParallelRun", "type": "boolean", "doc": "<p>TRUE if multiple test suites run in parallel.</p>"}`.

Handler, services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/start_suite_run.rs:211-225:
```
        // One run at a time per definition: any run of it that is not finished — PENDING,
        // RUNNING, or STOPPING until the stop completes — refuses the next start.
        let in_progress = store
            .runs_of_mut(&input.suite_definition_id, now)
            .find(|run| run.is_in_progress())
            .map(|run| run.id.clone());
        if let Some(in_progress) = in_progress {
            return Err(StartSuiteRunErrorModeled::ConflictException(ConflictException {
```
The refusal is computed from the store alone; `input.suite_run_configuration` is only moved into the stored record at start_suite_run.rs:246 (`configuration: input.suite_run_configuration,`).

The member is stored but never consulted anywhere in the crate: `grep -n 'parallel_run|parallelRun'` over the crate returns exactly two non-model hits — `src/ops/types/shared_types.rs:57  pub parallel_run: Option<bool>,` (the struct field) and `tests/all/suite_lifecycle.rs:297  configuration["parallelRun"] = json!(parallel);` (a test helper that only varies the body for a verbatim round-trip assertion). No `src/` file other than the type definition mentions it.

Test coverage: tests/all/suite_lifecycle.rs:834-846 builds the row "several selected tests, parallel" with `Some(true)` for `parallelRun`, and the same loop asserts at tests/all/suite_lifecycle.rs:933-948 `"a second run while one is in progress must conflict"` — i.e. the parallel case is exercised and pinned to the same ConflictException, with no stated reason tied to the model or to `parallelRun`.

## Steps to Reproduce

1. CreateSuiteDefinition with a valid configuration (suiteDefinitionName, rootGroup "g", devicePermissionRoleArn <role ARN>); keep suiteDefinitionId.
2. POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns with body {"suiteRunConfiguration": {"primaryDevice": {"thingArn": <thing ARN>}, "parallelRun": true}} -> HTTP 200, a PENDING run is minted.
3. Immediately POST the same path with the same body (still `parallelRun: true`).
4. Observed: HTTP 400 ConflictException ("Suite run <id> is still in progress for suite definition <id>"), identical to the answer for `parallelRun: false` or an absent `parallelRun` — the member the model describes as making "multiple test suites run in parallel" changed nothing.
5. GET /suiteDefinitions/{suiteDefinitionId}/suiteRuns/{suiteRunId} shows `suiteRunConfiguration.parallelRun: true` round-tripped, confirming the value is stored yet never consulted.
