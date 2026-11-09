Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6537d79e9f76a83fe65f7d2499927078c6ec62b1

# GetBillingGroupCostReport serves a 200 margin summary for billing periods before a LIVE group existed, where the same read answers 404 for a deleted group

_Filed: 2026-11-09T06:51:55Z_
_Operation: GetBillingGroupCostReport | Class: error-response_

> _Note: this issue was identified against the code base at commit `6537d79e9f76a83fe65f7d2499927078c6ec62b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler resolves the addressed billing group either from the live store or from the record a delete kept, and then checks that the requested `BillingPeriodRange` actually covers a period the group was billing in — but it applies that check ONLY on the deleted-group branch. For a LIVE group, a range that lies entirely before the group's creation billing period is answered HTTP 200 with a margin-summary row (zero amounts) instead of the modelled `ResourceNotFoundException`.

This contradicts three things inside this crate:

1. The handler's own comment, which states the rule it then enforces on one branch only: a report is of a group that was billing in the periods asked about, "the same groups the period-scoped listings report (`crate::state::billing_groups_in`)".
2. The sibling cost report, `ListBillingGroupCostReports`, which selects its elements through `state.billing_groups_in(&billing_period)` and therefore reports NO element for a group in a period that precedes its creation (`BillingGroupRecord::effective_in` -> `exists_in`: `wanted >= created`).
3. The crate's own test suite for the period-scoped reads, which states the rule as one rule over `ListBillingGroups`, `ListBillingGroupCostReports` AND `GetBillingGroupCostReport`, and pins HTTP 404 `ResourceNotFoundException` for a read of a period before the creation — but only exercises it for a group a delete has removed.

So the same question ("did this group bill in the period I am asking about?") is answered 404 for a deleted group and 200 for a live one.

Note for whoever fixes this: one row of `tests/all/billing_group_cost_report.rs` currently asks for a report over the FIXED months `2026-05..2026-06` on a group created now and asserts 200. That row names its period by a fixed month, which is exactly what that suite's own `period()` helper says must not be done ("every span an item here is created with is named relative to the service's own current billing period rather than by a fixed month"); re-expressing it with `period(...)` keeps the row's real subject (the 12-month maximum and the ordered-range rule) intact.

## Evidence

The rule the handler states, then enforces on one branch only — services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/get_billing_group_cost_report.rs:228-244:
```
// "The Amazon Resource Number (ARN) that uniquely identifies the billing group" —
// in either spelling the pattern admits, so it is normalised before the lookup. A
// report is of a group that was BILLING in the periods it is asked about, which a
// group a delete has since removed still was for the periods before that delete
// took effect — the same groups the period-scoped listings report
// (`crate::state::billing_groups_in`). So the record the delete kept is resolved
// here too, and which periods it answers for is checked once the range below is
// known; an ARN no stored and no deleted group carries names no group at all.
let group_arn = crate::state::normalize_billing_group_arn(&input.arn);
let live = state.billing_groups.get(&group_arn);
let deleted = match live { Some(_) => None, None => state.deleted_billing_group(&group_arn) };
let Some(group) = live.or(deleted.map(|deleted| &deleted.record)) else {
    return Err(no_such_billing_group(&group_arn));
};
```
and get_billing_group_cost_report.rs:287-298 — the period check exists for the DELETED record only; there is no counterpart for `live`:
```
// A group the store no longer holds is the subject of a report only for the
// billing periods before the delete took it
// (`crate::state::DeletedBillingGroupRecord::reported_in`): a range that covers
// none of them asks about a period that held no such group.
if let Some(deleted) = deleted {
    if !billing_periods.iter().any(|period| deleted.reported_in(period)) {
        return Err(no_such_billing_group(&group_arn));
    }
}
```

The sibling cost report DOES period-scope the group — src/ops/list_billing_group_cost_reports.rs:197-213:
```
// One row per billing group that was BILLING in that period, keyed by its ARN: ...
for (arn, (group, _version)) in state.billing_groups_in(&billing_period) {
```
and the period rule it reads through — src/state.rs:3243-3247:
```
for (arn, group) in &self.billing_groups {
    if let Some(version) = group.effective_in(billing_period) {
        groups.insert(arn.clone(), (group, version));
    }
}
```
with src/state.rs:1059-1072: "Does this resource exist in `billing_period`? A resource does not exist in a period that precedes the one it was created in" / `(Some(wanted), Some(created)) => wanted >= created`.

The crate's test that states the rule for all three reads and pins the 404 — tests/all/deleted_billing_group_period_reads.rs:7-17:
```
//! RULE: a billing group bills a consolidated family FOR A BILLING PERIOD ... so every
//! read scoped to a billing period reports the groups that were billing in THAT period
//! — from the period a group was created in through the last period before the delete,
//! whether or not the group is still stored ... One rule, so the three period-scoped
//! reads of a group (`ListBillingGroups`, `ListBillingGroupCostReports` and
//! `GetBillingGroupCostReport`) ... agree
```
its read offsets include a period a year BEFORE the creation (tests/all/deleted_billing_group_period_reads.rs:406-418: `row.created - 13`, `row.created - 1`) and it asserts (tests/all/deleted_billing_group_period_reads.rs:528-538):
```
} else {
    assert_eq!(status, 404, "case {case}: expected HTTP 404 for a period that held no such group, ...");
    assert_eq!(error_code(&body), "ResourceNotFoundException", ...);
}
```
The companion suite for a LIVE group (tests/all/billing_group_billing_periods.rs:46-47) drives only `/list-billing-groups`, so the live branch of this rule is untested — which is how the gap survives.

The one row that incidentally depends on today's behaviour, and the convention it breaks — tests/all/billing_group_cost_report.rs:884-892 (`"InclusiveStartBillingPeriod": "2026-05", "ExclusiveEndBillingPeriod": "2026-06"` expecting `200`) against tests/all/billing_group_cost_report.rs:293-300: "every span an item here is created with is named relative to the service's own current billing period rather than by a fixed month".

## Steps to Reproduce

1. Start the emulator with this service and make the emulated account the payer so the control plane is reachable.
2. POST /create-pricing-plan {"Name":"P1"}, then POST /create-billing-group with that plan, a PrimaryAccountId and one linked account — the group is created in the CURRENT billing period.
3. POST /get-billing-group-cost-report {"Arn":"<group>","BillingPeriodRange":{"InclusiveStartBillingPeriod":"<current minus 3 months>","ExclusiveEndBillingPeriod":"<current minus 2 months>"}}.
4. Observe HTTP 200 with one BillingGroupCostReportResults row naming the group, for a period the group never billed in.
5. POST /list-billing-group-cost-reports {"BillingPeriod":"<current minus 3 months>","Filters":{"BillingGroupArns":["<group>"]}} — the sibling report returns no element for that period.
6. Now POST /delete-billing-group for the group and repeat step 3 against the deleted ARN for the same pre-creation period: it is refused HTTP 404 ResourceNotFoundException (the behaviour tests/all/deleted_billing_group_period_reads.rs pins), showing the two branches disagree.
