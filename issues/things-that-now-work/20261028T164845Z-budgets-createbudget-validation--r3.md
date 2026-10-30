Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 62bd53a2191a4112059965729a42c5aa0aaceb39

# A TimePeriod.Start after 06/15/87 with no End is accepted and stored with Start after End — the one shape validate_budget refuses when both dates are sent

_Filed: 2026-10-28T16:48:45Z_
_Operation: CreateBudget | Class: validation_

> _Note: this issue was identified against the code base at commit `62bd53a2191a4112059965729a42c5aa0aaceb39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both budget write paths run the cross-member rules over the budget EXACTLY AS THE CALLER SENT IT, and only afterwards stamp the service-owned members — including the documented default `TimePeriod.End` of `06/15/87 00:00 UTC`. The ordering rule "The start date must come before the end date" is therefore never re-checked against the End the service itself supplies.

Consequence: `CreateBudget` with `TimePeriod: {"Start": <any instant after 2087-06-15T00:00:00Z>}` and no `End` is accepted (200) and persisted with `Start` = 2100-01-01 and `End` = 2087-06-15. Every later `DescribeBudget` / `DescribeBudgets` reports a budget whose `TimePeriod.Start` comes AFTER its `TimePeriod.End` — exactly the shape the same handler refuses with `InvalidParameterException` when the caller spells both dates out (an explicit `End` past the bound is refused by `validate_time_period_end`, and an explicit `Start > End` pair is refused by the ordering check). The rule is the whole shape's, so the stored budget is in a state the model says cannot exist, and the crate's own doc for `validate_budget` claims no write path can leave one there.

The side effects follow the bad period: such a budget's `DescribeBudgetPerformanceHistory` reports an empty `BudgetedAndActualAmountsList` for every retained period, because `period_overlaps(period, covered)` can never hold for a window whose Start is after its End.

`UpdateBudget` has the identical ordering (update_budget.rs:197 validates the raw `NewBudget`, update_budget.rs:227 stamps the default afterwards), so an update can put an existing, well-formed budget into the same shape.

The fix is to apply the same ordering rule to the budget as the write path LEAVES it (after `stamp_service_owned` supplies the defaults), not only to the members the request carried.

## Evidence

Smithy, `Budget$TimePeriod` (smithy_query doc, com.amazonaws.budgets#Budget member TimePeriod): "The period of time that's covered by a budget. You set the start date and end date. The start date must come before the end date. The end date must come before 06/15/87 00:00 UTC." and, in the same paragraph, "If you didn't specify an end date, Amazon Web Services set your end date to 06/15/87 00:00 UTC."

The check runs on the RAW request — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_budget.rs:211:
    crate::state::validate_budget(&input.budget).map_err(|message| {
        CreateBudgetErrorModeled::InvalidParameterException(InvalidParameterException {
and the default End is supplied only afterwards — src/ops/create_budget.rs:354:
    crate::state::stamp_service_owned(&mut budget, now, None);

The rule that is skipped — src/state.rs:1809-1817:
    if budget
        .time_period
        .as_ref()
        .is_some_and(|period| !time_period_ordered(period))
    {
        return Err(
            "Unable to create/update budget: TimePeriod Start must come before End.".to_string(),
        );
    }

The End the service then writes over it — src/state.rs:762-764:
    if period.end.is_none() {
        period.end = Some(default_time_period_end());
    }
with src/state.rs:515-517:
    pub fn default_time_period_end() -> ::aws_smithy_types::DateTime {
        midnight_at("2087-06-15T00:00:00Z").expect("06/15/87 00:00 UTC is a date-time")

The crate's own claim that this cannot happen — src/state.rs:1788-1791 (`validate_budget` doc):
    /// Every rule holds for the whole `Budget` shape, so both write paths
    /// (`CreateBudget$Budget` and `UpdateBudget$NewBudget`) run all of them before anything
    /// is stored: an update that would leave a budget in a shape a create could not have
    /// produced is refused, and the stored budget is left as it was.

The same ordering on the update path — src/ops/update_budget.rs:197 `crate::state::validate_budget(&input.new_budget)` then src/ops/update_budget.rs:227 `crate::state::stamp_service_owned(&mut budget, now, stored.budget.health_status.clone());`.

Test coverage shows the gap is unpinned: tests/all/budget_validation_rules.rs:292 `a_time_period_end_after_the_documented_last_instant_is_refused` varies only the END (its rows at :298-309 and the "no End sent" case at :344-354 use `start = at("2024-01-01T00:00:00Z")`), and tests/all/budget_lifecycle.rs:1365 pins the refusal only for an EXPLICIT pair (`"TimePeriod": {"Start": 1_735_689_600, "End": 1_704_067_200}`). No test sends a Start past the default End with no End at all.

## Steps to Reproduce

1. CreateBudget with AccountId = the caller's account and Budget = {"BudgetName":"start-past-the-bound","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"1","Unit":"USD"},"TimePeriod":{"Start":4102444800}} (2100-01-01T00:00:00Z) and NO "End" member.
2. Observe HTTP 200: the request is accepted, because src/ops/create_budget.rs:211 ran the ordering rule before src/ops/create_budget.rs:354 supplied the default End.
3. DescribeBudget for the same name: the reported Budget.TimePeriod is {"Start": 2100-01-01T00:00:00Z, "End": 2087-06-15T00:00:00Z} — Start after End.
4. Send the SAME two instants explicitly ("TimePeriod":{"Start":4102444800,"End":3675196800}): the call is refused with InvalidParameterException "Unable to create/update budget: TimePeriod Start must come before End." — the same stored shape, two different answers.
5. Repeat step 1 as UpdateBudget on an existing well-formed budget (NewBudget carrying only the future Start): the stored budget is moved into the same Start-after-End shape, which src/state.rs:1788-1791 states no write path can leave behind.
