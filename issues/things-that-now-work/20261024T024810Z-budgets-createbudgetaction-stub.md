Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# Implement budgets CreateBudgetAction: unimplemented scaffold stub

_Filed: 2026-10-24T02:48:10Z_
_Operation: CreateBudgetAction | Class: stub_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

> _RESOLVED by the fix for `20261024T024810Z-budgets-executebudgetaction-stub`: `CreateBudgetAction` is implemented (it mints the UUID `ActionId`, persists the action on its budget in `STANDBY`, and enforces the `ActionType`/`Definition` rule) and its `//! SCAFFOLDED, NOT IMPLEMENTED:` census line is deleted. `ExecuteBudgetAction` has nothing to act on without it, so it could not be left to this issue. Nothing is left to do here; this report is kept only until a scan files it as done._

`CreateBudgetAction` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_budget_action.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_budget_action.rs`.

## Steps to Reproduce

1. Boot the budgets emulator
2. Call CreateBudgetAction and observe HTTP 501 with no state persisted
