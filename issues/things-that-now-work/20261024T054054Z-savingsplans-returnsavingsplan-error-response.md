Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# ReturnSavingsPlan never produces its modeled ServiceQuotaExceededException for the documented per-month return limits

_Filed: 2026-10-24T05:40:54Z_
_Operation: ReturnSavingsPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator enforces exactly one restriction on a return: the plan must be in `payment-pending`/`active` and still inside the 7-day (168-hour) window measured from its start. Every other refusal path answers 400 `ValidationException`.

AWS additionally caps returns per payer account per calendar month (see "Returning a Savings Plan" in the AWS Savings Plans User Guide: besides the 168-hour window, there is a cap on how many Savings Plans and how much cumulative commitment may be returned in a calendar month). Exceeding a quota is what the operation's modeled `ServiceQuotaExceededException` (HTTP 402, "A service quota has been exceeded") is for, and no request can reach it in the emulator: `ServiceQuotaExceededException` is declared in `ReturnSavingsPlanErrorModeled` but is never constructed anywhere in the handler.

Expected: once an account has returned its monthly allowance of Savings Plans (count and/or cumulative commitment), a further `ReturnSavingsPlan` is refused with HTTP 402 `ServiceQuotaExceededException` and the plan keeps its state.
Actual: every in-window return is accepted indefinitely — a caller can return any number of plans of any commitment in the same month, all answering 200.

## Evidence

Spec: `ReturnSavingsPlan` declares `ServiceQuotaExceededException (HTTP 402)` in the Smithy model (OPERATIONS.md "## ReturnSavingsPlan" and `smithy_query operation=ReturnSavingsPlan`); the 402 shape's doc is "A service quota has been exceeded". Well-established behavior: AWS documents per-calendar-month limits on Savings Plan returns in addition to the 168-hour window that `src/state.rs::RETURN_WINDOW_MILLIS` already implements. Handler: `src/ops/return_savings_plan.rs` lines 169-204 — the only refusals are `ValidationException`; grep shows no `ServiceQuotaExceededException` construction in that file (unlike `create_savings_plan.rs`, which does enforce a quota). Tests: `tests/all/savings_plans_lifecycle.rs::returning_a_plan_moves_it_out_of_its_returnable_state` and `a_plan_cannot_be_returned_twice` assert only the window/state refusals; no test covers a monthly return quota.

## Steps to Reproduce

1. POST /CreateSavingsPlan many times (e.g. 15 purchases) against a seeded offering with a large `commitment`
2. POST /ReturnSavingsPlan for each of the freshly purchased plans in turn, all inside the 7-day window
3. Observe every call answers 200 with the returned plan id
4. Expected: after the account's documented monthly return allowance is used up, further returns are refused with HTTP 402 ServiceQuotaExceededException and DescribeSavingsPlans still reports those plans as active/payment-pending
