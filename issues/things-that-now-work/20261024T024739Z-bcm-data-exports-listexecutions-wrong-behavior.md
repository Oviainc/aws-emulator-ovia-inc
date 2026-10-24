Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# An export never accrues an execution, so ListExecutions/GetExecution can never report a delivery

_Filed: 2026-10-24T02:47:39Z_
_Operation: ListExecutions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Nothing in the emulator ever records a `StoredExecution`: the execution store is written only by test code reaching into state directly. Through the public API an export created with any `RefreshCadence` stays with zero executions forever.

Expected: an export that the service has refreshed has at least one execution, so `ListExecutions` on it returns a non-empty `Executions` list of `ExecutionReference` values and `GetExecution` on one of those ids returns 200 with the export snapshot and an `ExecutionStatus` (one of the modelled `ExecutionStatusCode` values), plus the export's `ExportStatus.LastRefreshedAt` becoming populated.
Actual: `ListExecutions` always answers 200 with an empty list and `GetExecution` always answers the not-found refusal for every id, for every export, no matter how long it has existed or what cadence it declares — so the whole execution half of this API is unreachable from the wire and `ExportStatus.LastRefreshedAt` is never set.

## Evidence

Spec: the model binds `ListExecutions` (@paginated over `Executions`) and `GetExecution` with an `Export` snapshot plus `ExecutionStatus`, and documents an execution as a delivery of the export — an export with a `RefreshCadence` of SYNCHRONOUS does run and does record executions on real AWS. Handler/state: `src/state.rs` (`StoredExecution` doc, lines ~116-124) states "NOTHING in this emulator creates one", and `src/state.rs` `fresh_status` leaves `last_refreshed_at: None` with no code path that ever sets it; `src/ops/list_executions.rs` and `src/ops/get_execution.rs` only read that store. Test coverage: `tests/all/execution_listing.rs` (header lines 22-27) and `tests/all/execution_lookup.rs` seed executions via `record_execution` directly into state rather than through any operation, so no test exercises an execution produced by the service itself.

## Steps to Reproduce

1. CreateExport with a valid DataQuery and RefreshCadence (e.g. SYNCHRONOUS) and keep the returned ExportArn.
2. Call ListExecutions with that ExportArn.
3. Observe 200 with no Executions entries, now and on every later call.
4. Call GetExport and observe ExportStatus.LastRefreshedAt is absent forever.
5. Call GetExecution with any ExecutionId for that export and observe the not-found refusal — there is no id the emulator can ever serve.
