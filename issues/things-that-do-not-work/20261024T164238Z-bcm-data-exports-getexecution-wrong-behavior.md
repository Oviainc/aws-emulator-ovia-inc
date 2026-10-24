Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# Every recorded execution terminates in QUERY_FAILURE/INTERNAL_FAILURE instead of a successful delivery

_Filed: 2026-10-24T16:42:38Z_
_Operation: GetExecution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator schedules a delivery for each persisted definition and materialises it on the clock, but `crate::state::complete_delivery` moves EVERY execution to the terminal status `QUERY_FAILURE` with `StatusReason: INTERNAL_FAILURE`, and `crate::state::fresh_status` leaves `ExportStatus.LastRefreshedAt` absent for the life of the export.

Expected (real AWS): an export whose definition and destination are valid delivers successfully — its execution progresses through the modelled `ExecutionStatusCode` ladder to `DELIVERY_SUCCESS` with `CompletedAt` set and no `StatusReason`, and the export's `ExportStatus.LastRefreshedAt` is set to the instant the file was generated (the export stays `HEALTHY`). A client that polls `GetExecution`/`ListExecutions` waiting for a successful delivery, or that reads `LastRefreshedAt` to decide whether data is available, sees success on AWS.

Actual (emulator): `GetExecution` (and the `ExecutionReference`s from `ListExecutions`) report `StatusCode: QUERY_FAILURE`, `StatusReason: INTERNAL_FAILURE` for every export, including a freshly created, fully valid one, and `GetExport` never reports `LastRefreshedAt`. Any client branching on execution status takes the failure path forever.

## Evidence

Spec: `smithy_query(query="shape",shape="ExecutionStatusCode")` models `INITIATION_IN_PROCESS, QUERY_QUEUED, QUERY_IN_PROCESS, QUERY_FAILURE, DELIVERY_IN_PROCESS, DELIVERY_SUCCESS, DELIVERY_FAILURE` — the success terminal state `DELIVERY_SUCCESS` is modelled and unreachable here; `ExportStatus.LastRefreshedAt` is "The timestamp of when the export was last generated" and is likewise never populated.
Implementation: `src/state.rs` `complete_delivery` (lines ~444-452) hardcodes `EXECUTION_STATUS_QUERY_FAILURE` + `EXECUTION_STATUS_REASON_INTERNAL_FAILURE`; `fresh_status` (lines ~730-737) hardcodes `last_refreshed_at: None`; the module docs state the choice explicitly ("the delivery it records cannot succeed").
Tests: `tests/all/delivery_executions.rs` currently pins the emulator's failure outcome (asserts `QUERY_FAILURE`/`INTERNAL_FAILURE` and an absent `LastRefreshedAt`), i.e. the divergence is deliberate and would need that test updated — no test asserts the AWS-side success path.

## Steps to Reproduce

1. CreateExport with a valid definition (valid catalog query, TEXT_OR_CSV/GZIP destination) and note the ExportArn
2. Wait past the emulator's delivery schedule (DELIVERY_INITIATION_DELAY + DELIVERY_SETTLES_AFTER) and call ListExecutions for that ARN
3. Call GetExecution with the reported ExecutionId: observe ExecutionStatus.StatusCode = QUERY_FAILURE and StatusReason = INTERNAL_FAILURE (real AWS reports a successful delivery, DELIVERY_SUCCESS, for a valid export)
4. Call GetExport for the same ARN: observe ExportStatus.LastRefreshedAt absent even though an execution has completed
