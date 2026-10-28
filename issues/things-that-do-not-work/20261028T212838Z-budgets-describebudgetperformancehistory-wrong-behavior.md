Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1faf41e59ff01533a48e04d029136d23581e5c75

# A CUSTOM budget whose TimePeriod.Start is before the Unix epoch reports a history record with neither BudgetedAmount nor ActualAmount, although DescribeBudget reports its limit and spend

_Filed: 2026-10-28T21:28:38Z_
_Operation: DescribeBudgetPerformanceHistory | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1faf41e59ff01533a48e04d029136d23581e5c75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::amounts_in_period` builds each history record by projecting the stored budget AS OF the period's start instant. It reaches that instant through `crate::state::instant_at`, which answers `None` for "an epoch before the epoch" — and when it does, BOTH reported figures silently become absent: `budgeted_amount: None`, `actual_amount: None`, with only `TimePeriod` left on the record.

That outcome is reachable from input the model allows. `TimePeriod$Start` is a plain `GenericTimestamp` with no range constraint, and `crate::state::validate_budget` only checks that Start precedes End and that End is not after 06/15/87, so a `CUSTOM` budget created with `TimePeriod {Start: -315619200 (1960-01-01), End: <a future instant>}` is accepted and stored. For a `CUSTOM` budget the handler takes the budget's OWN period as the single history candidate (`describe_budget_performance_history.rs:277-281`), so that pre-epoch period is exactly the period handed to `amounts_in_period`, and the one record the history reports comes back with no amounts at all.

Two statements in the crate are broken by this:

1. `amounts_in_period`'s own doc says the two figures are the ones a read of the budget reports as of that period, "so a performance history cannot disagree with `DescribeBudget`". `DescribeBudget` on the very same stored budget reports `BudgetLimit {Amount: "100", Unit: "USD"}` and `CalculatedSpend.ActualSpend {"0", "USD"}` (`budget_as_read` / `calculated_spend` do no instant arithmetic at all), so the history and the single read disagree about the same budget.
2. The same doc names the ONE case in which neither figure is reported — "a budget whose read reports no limit at all (the auto-adjusting budget …)". This budget reports a limit, so the omission is a second, undocumented cause.

The crate's sibling treatment of the identical unrepresentable-instant case fails CLOSED: `crate::state::is_period_start` refuses to treat such an epoch as a period boundary at all, with a comment explaining why. This path instead fails open, emitting a record that claims a period was measured and reports nothing for it.

Only `CUSTOM` is affected: the DAILY/MONTHLY/QUARTERLY candidates come from `retained_periods`, which only ever produces periods near the clock.

## Evidence

The guard that drops both figures, and the doc directly above it that says it cannot happen — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:1988-2021:

    /// The budgeted and actual amounts `stored` reports for one `period` it covers — "the
    /// amount of cost or usage that you created the budget for, compared to your actual costs
    /// or usage" (`BudgetedAndActualAmounts`).
    ///
    /// Both figures are the ones a read of the budget reports AS OF that period
    /// ([`project_budget`]), so a performance history cannot disagree with `DescribeBudget`:
    ...
    /// Neither is reported for a budget whose read reports no limit at all (the auto-adjusting
    /// budget of [`calculated_spend`]): nothing in such a record names a unit of measurement,
    /// and `Spend$Unit` is `@required`.
    pub fn amounts_in_period(stored: &StoredBudget, period: &TimePeriod) -> BudgetedAndActualAmounts {
        let projected = period
            .start
            .as_ref()
            .and_then(|start| instant_at(start.secs()))
            .map(|at| project_budget(stored, at));
        BudgetedAndActualAmounts {
            budgeted_amount: projected
                .as_ref()
                .and_then(|budget| budget.budget_limit.clone()),
            actual_amount: projected
                .as_ref()
                .and_then(|budget| budget.calculated_spend.as_ref())
                .map(|spend| spend.actual_spend.clone()),
            time_period: Some(period.clone()),
        }
    }

Why `instant_at` answers `None` for a pre-1970 Start — src/state.rs:702-711:

    /// The instant `secs` seconds after the Unix epoch, or `None` when that is no instant
    /// this host can represent: an epoch before the epoch, or one so far after it that the
    /// platform's clock type cannot hold it. ...
    fn instant_at(secs: i64) -> Option<SystemTime> {
        let secs = u64::try_from(secs).ok()?;
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))
    }

The CUSTOM path that feeds the budget's own period straight into it — src/ops/describe_budget_performance_history.rs:274-302:

        // The periods this budget HAS: the retained series of its own time unit, or — for
        // the time unit that names no repeating period — the single period its caller
        // chose, which is the whole of such a budget's history.
        let candidates = if stored.budget.time_unit == crate::state::TIME_UNIT_CUSTOM {
            stored.budget.time_period.clone().into_iter().collect()
        } else {
            crate::state::retained_periods(&stored.budget.time_unit, now)
        };
        ...
            .map(|period| crate::state::amounts_in_period(stored, period))

The model admits the input: `smithy_query(query="shape",shape="TimePeriod")` gives `"Start": {"target": "GenericTimestamp", "type": "timestamp", ...}` — no range — and `"TimeUnit"` enumerates `CUSTOM`. The only cross-member rules the write path applies are `src/state.rs:1809-1818` (`time_period_ordered`, Start strictly before End) and `validate_time_period_end` (End not after 06/15/87), neither of which bounds Start from below.

The sibling that fails CLOSED on the very same unrepresentable instant — src/state.rs:716-728:

    fn is_period_start(time_unit: &str, secs: i64) -> bool {
        let Some(at) = instant_at(secs) else {
            return false;
        };
        // An instant the calendar cannot render names no period at all.

The member the record drops is the one the model describes as the point of the record — `BudgetedAndActualAmounts$BudgetedAmount`: "The amount of cost or usage that you created the budget for."

Test coverage: none. tests/all/budget_performance_history.rs uses only post-epoch starts — `const LONG_RUNNING_START: &str = "2000-01-01T00:00:00Z";` (:57) and `month_start_offset(..)` offsets from the current month (:399, :511, :701, :760) — so the CUSTOM rule it pins ("RULE: a budget whose time unit names no repeating period covers exactly the one `TimePeriod` its caller chose", :740-793) and the budgeted-amount rule (:674) both stay unchanged by a fix that makes a pre-epoch period project the budget the way every other period does.

## Steps to Reproduce

1. CreateBudget for the caller's own AccountId with {"BudgetName":"pre-epoch","BudgetType":"COST","TimeUnit":"CUSTOM","BudgetLimit":{"Amount":"100","Unit":"USD"},"TimePeriod":{"Start":-315619200,"End":1900000000}} — accepted (HTTP 200): Start precedes End and End is before 06/15/87, the only two bounds src/state.rs:1809-1818 applies.
2. DescribeBudget for that budget. Observe it reports BudgetLimit {"Amount":"100","Unit":"USD"} and CalculatedSpend.ActualSpend {"Amount":"0","Unit":"USD"}.
3. DescribeBudgetPerformanceHistory {"AccountId":<caller>,"BudgetName":"pre-epoch"}. The CUSTOM branch (src/ops/describe_budget_performance_history.rs:277-281) makes the budget's own period the single candidate, and it has begun, so one record is reported.
4. Inspect BudgetPerformanceHistory.BudgetedAndActualAmountsList[0]. Expected (amounts_in_period's own doc, src/state.rs:1992-1993): the figures the budget's read reports — BudgetedAmount {"100","USD"} and ActualAmount {"0","USD"}. Actual: only TimePeriod is present; BudgetedAmount and ActualAmount are both absent, because instant_at(-315619200) is None (src/state.rs:708-710).
5. Re-create the same budget with "Start":315619200 (1980) instead and repeat: both amounts are now reported. The only difference is that the Start is representable, which nothing in the model or in the crate's documented rules makes a condition for reporting the amounts.
