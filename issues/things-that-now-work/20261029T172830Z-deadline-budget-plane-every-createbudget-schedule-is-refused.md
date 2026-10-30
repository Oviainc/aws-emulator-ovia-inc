Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.budget-actions-exceed-limit-rejected deadline.budget-dollar-limit-below-minimum-rejected deadline.budget-status-toggle-active-inactive
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.budget-actions-exceed-limit-rejected|run#13|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: deadline.budget-dollar-limit-below-minimum-rejected|run#13|AssertionError|assert message == _MSG_BELOW_MIN, f"unexpected message: {message!r}"
Issue-Key: deadline|CreateBudget|ValidationException
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# deadline budget plane: every CreateBudget schedule is refused as an unparseable int timestamp, and the budget constraint/status rules are unserved (3 causes)

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBudget

## Description
Three net-new root causes in deadline's budget plane. All three checks below die on the SAME first call (CreateBudget), so cause 1 is what the replay observed directly and causes 2 and 3 are the rules the same operation still has to serve once a valid request can get through.

Cause 1 of 3 — a FixedBudgetSchedule with @timestampFormat date-time members is rejected at the wire boundary.
AWS: Smithy (services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json): CreateBudget is POST /2023-10-12/farms/{farmId}/budgets, success 200 with budgetId (pattern ^budget-[0-9a-f]{32}$); schedule is a required BudgetSchedule union whose only member `fixed` targets FixedBudgetSchedule, and FixedBudgetSchedule.startTime (StartsAt) and .endTime (EndsAt) are both required timestamps carrying timestampFormat "date-time". An SDK therefore puts RFC3339 strings on the wire for those two members, and AWS accepts them: the recorded behavior is a 200 carrying budgetId, after which the new budget is visible to ListBudgets/GetBudget.
Emulator: every CreateBudget that carries a schedule is refused with HTTP 400 ValidationException and the message "invalid timestamp: failed to parse int" (all fixed text, no request-derived part — it is not AWS wording for any error class). The refusal happens in the request walk's deserialize/validate step, before the handler runs, so a wholly valid request is declared invalid.
Why: no client can create a budget at all against this emulator. Worse than a missing feature: the client is told its own correct, SDK-serialized request is malformed, so retry/validation logic concludes the caller is at fault. It also masks every later budget behavior.
Fix: the request deserializer/schema binding for date-time timestamps reached through a union member (CREATE_BUDGET_INPUT_SCHEMA in services/service-com-amazonaws-deadline-deadline/src/ops/types.rs and the shared smithy-server-runtime restJson1 timestamp deserializer it drives) — a date-time member must accept an RFC3339 string, not only an integer epoch. Then the operation's handler (src/ops/create_budget.rs), which is still the scaffolded 501 stub (line 182 returns Scaffolded::not_implemented("CreateBudget")), has to store the budget.
Traces: deadline.budget-actions-exceed-limit-rejected, deadline.budget-dollar-limit-below-minimum-rejected, deadline.budget-status-toggle-active-inactive (identical 400 on the first CreateBudget).

Cause 2 of 3 — CreateBudget does not serve AWS's constraint-refusal wording for actions and approximateDollarLimit, and must create nothing when it refuses.
AWS: Smithy: CreateBudget.actions (BudgetActionsToAdd) @length min 0 max 10; CreateBudget.approximateDollarLimit (ConsumedUsageLimit, float) @range min 0.01; modeled error ValidationException (HTTP 400). Recorded wording for these two classes, as the checks assert it — fixed text plus values taken from the request and the model's limit:
  * too many actions: '[array is too long: must have at most 10 elements but instance has 11 elements, format attribute "float" not supported]'. Fixed: '[array is too long: must have at most <max> elements but instance has <n> elements, format attribute "float" not supported]' where <max> is the modeled maximum (10) and <n> is the number of entries in the request's actions list (11 in the recording); the trailing 'format attribute "float" not supported' clause is fixed text AWS appends for this input shape.
  * dollar limit below minimum: '[numeric instance is lower than the required minimum (minimum: 0.01, found: 0.001), format attribute "float" not supported]'. Fixed: '[numeric instance is lower than the required minimum (minimum: <min>, found: <value>), format attribute "float" not supported]' where <min> is the modeled minimum (0.01) and <value> is the number the request sent (0.001 in the recording). Both refusals leave no budget behind: ListBudgets must not report the rejected displayName, and the same request at the limit (10 actions / 0.01) succeeds.
Emulator: both requests are refused with the cause-1 message "invalid timestamp: failed to parse int", so neither constraint class is ever reached; nothing in the crate produces AWS's bracketed JSON-schema-style wording for either class, and the service's single-homed ValidationException helpers (field_validation_failed / cannot_parse in src/state.rs:384-413) emit a bare member message with reason FIELD_VALIDATION_FAILED / CANNOT_PARSE instead.
Why: a client that distinguishes "too many budget actions" from "dollar limit too low" by the refusal message or fieldList cannot tell them apart, and a client probing the limits reads the same message for every bad value.
Fix: the modeled-constraint refusal path for this operation — the boundary validator's message rendering plus this crate's ValidationException helpers in src/state.rs — and the effect-absent guarantee in src/ops/create_budget.rs's handler (refuse before inserting).
Traces: deadline.budget-actions-exceed-limit-rejected, deadline.budget-dollar-limit-below-minimum-rejected.

Cause 3 of 3 — the budget status lifecycle is not implemented: created ACTIVE, and INACTIVE is terminal.
AWS: recorded behavior only (the model gives the BudgetStatus enum and UpdateBudget's ValidationException 400, but documents no transition rule). Recording: GetBudget immediately after CreateBudget reports status ACTIVE; UpdateBudget(status=INACTIVE) succeeds and GetBudget then reports INACTIVE; UpdateBudget(status=ACTIVE) on that INACTIVE budget is refused with HTTP 400 ValidationException and the message "Reactivating of INACTIVE budget is not allowed" (entirely fixed text — no id, name or value is interpolated), and the refused call leaves the budget's status INACTIVE. The transition is one-way, not a toggle.
Emulator: unobservable because the check cannot get past CreateBudget (cause 1); beyond that, CreateBudget, GetBudget and UpdateBudget are all scaffolded 501 stubs (src/ops/create_budget.rs, src/ops/get_budget.rs, src/ops/update_budget.rs all carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker), so no budget status is stored and no reactivation is refused.
Why: a cost-control client reads status to know whether a budget is enforcing, and relies on reactivation being refused rather than silently re-enabling spend limits; an emulator that accepts the reactivation (or serves no status at all) lets that logic pass tests it would fail against AWS.
Fix: budget state in src/state.rs (a Budget record under its farm with a status field) plus the three budget handlers; the one-way ACTIVE->INACTIVE gate belongs in src/ops/update_budget.rs with the wording single-homed next to the crate's other refusal helpers.
Traces: deadline.budget-status-toggle-active-inactive.

## Evidence
Suite: 0/22 deadline checks passed.

[mismatch] deadline.budget-actions-exceed-limit-rejected — AssertionError: expected message '[array is too long: must have at most 10 elements but instance has 11 elements, format attribute "float" not supported]', got 'invalid timestamp: failed to parse int'.

[mismatch] deadline.budget-dollar-limit-below-minimum-rejected — AssertionError: unexpected message: 'invalid timestamp: failed to parse int' (expected '[numeric instance is lower than the required minimum (minimum: 0.01, found: 0.001), format attribute "float" not supported]').

[error] deadline.budget-status-toggle-active-inactive — botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CreateBudget operation: invalid timestamp: failed to parse int. The check never reached GetBudget/UpdateBudget.

Model evidence: FixedBudgetSchedule.startTime/endTime -> {"timestampFormat": "date-time", "required": true}; CreateBudget.actions @length {min 0, max 10}; CreateBudget.approximateDollarLimit @range {min 0.01}; errors include ValidationException (httpError 400).

Code evidence: services/service-com-amazonaws-deadline-deadline/src/ops/create_budget.rs:182 returns crate::scaffolded::Scaffolded::not_implemented("CreateBudget"); src/ops/get_budget.rs:4 and src/ops/update_budget.rs:4 carry the same SCAFFOLDED marker; src/state.rs:384-413 are the only ValidationException helpers in the crate.

## Steps to Reproduce
1. deadline.create_farm(displayName=...) -> farmId; deadline.create_queue(farmId=..., displayName=...) -> queueId.
2. Call CreateBudget with an SDK-serialized schedule: schedule={"fixed": {"startTime": now, "endTime": now + 1 day}} (datetime objects, so date-time RFC3339 strings on the wire), usageTrackingResource={"queueId": queueId}, approximateDollarLimit=10.0, actions=[{"type": "STOP_SCHEDULING_AND_COMPLETE_TASKS", "thresholdPercentage": 80.0}].
   Observed: HTTP 400 ValidationException 'invalid timestamp: failed to parse int'. Expected: HTTP 200 with budgetId.
3. Repeat step 2 with actions holding 11 entries -> expect ValidationException '[array is too long: must have at most 10 elements but instance has 11 elements, format attribute "float" not supported]' and no budget created (ListBudgets must not show that displayName).
4. Repeat step 2 with approximateDollarLimit=0.001 on a client built with Config(parameter_validation=False) -> expect ValidationException '[numeric instance is lower than the required minimum (minimum: 0.01, found: 0.001), format attribute "float" not supported]'; the same request with 0.01 must succeed.
5. On a created budget: GetBudget -> status ACTIVE; UpdateBudget(status='INACTIVE') -> GetBudget reports INACTIVE; UpdateBudget(status='ACTIVE') -> expect HTTP 400 ValidationException 'Reactivating of INACTIVE budget is not allowed' with the status left INACTIVE.
