Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e28dc1337060a874b1991c751dde47e1a17d0887

# A GroupBy=BILLING_PERIOD margin summary omits every billing period of the range that carries no custom line item, although the same request with no breakdown reports that period's zero summary

_Filed: 2026-11-08T18:45:21Z_
_Operation: GetBillingGroupCostReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e28dc1337060a874b1991c751dde47e1a17d0887`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetBillingGroupCostReport` builds its rows ONLY out of the entries `proforma_custom_line_item_charges` returns for each period of the requested range. A period with no custom line item in effect yields an empty map, so no row is created for it at all — even though the period IS one of the periods the request asked about and its margin summary is fully determined (AWSCost 0, ProformaCost 0, Margin 0, MarginPercentage 0).

The same handler contradicts itself two lines earlier: for a request that asks for NO breakdown it pre-inserts a default (all-zero) row and says in a comment that the row is "reported even when the group has no charge in any of those periods, because the group's own cost is what the report is of". Under `GroupBy: ["BILLING_PERIOD"]` that same reason holds for every month of the range — each row IS the group's own cost for that month, and the months are already enumerated in `billing_periods` — yet those rows are dropped.

Concrete consequences, with the range `[M-3, M+1)` on a group whose only custom line item applies from M on:
- `GroupBy: ["BILLING_PERIOD"]` answers ONE row (M) instead of four, so a client that pages the breakdown sees gaps where the service does have an answer;
- a group with no custom line item at all answers an EMPTY `BillingGroupCostReportResults` under that breakdown, telling the caller the group has no margin summary rows whatsoever, while the identical request without `GroupBy` answers one row carrying the group's ARN and the zero amounts.

The state module states the rule the handler drops: a period with no item still HAS a pro forma cost, and a zero amount of this service is reported rather than withheld. The sibling cost report (`ListBillingGroupCostReports`) does exactly that: it reports a summary for every billing group the period holds, summing a possibly-empty charge map to 0.

(The PRODUCT_NAME breakdown is a different case and is not part of this defect: the universe of products with no charge is not enumerable, whereas the periods of the range are.)

## Evidence

The default zero row and the reason given for it — `src/ops/get_billing_group_cost_report.rs:326-333`:
```
        // One row per group of the requested breakdown, ordered by its cursor key. A
        // request that asks for no breakdown is asking about the billing group itself over
        // the whole range, which is ONE row — reported even when the group has no charge
        // in any of those periods, because the group's own cost is what the report is of.
        let mut rows: BTreeMap<String, ReportRow> = BTreeMap::new();
        if group_by.is_empty() {
            rows.insert(row_key(None, None), ReportRow::default());
        }
```

Rows for a breakdown come only from charge entries, so a charge-free period creates none — `src/ops/get_billing_group_cost_report.rs:334-341`:
```
        for billing_period in &billing_periods {
            for (product_name, charge) in
                state.proforma_custom_line_item_charges(&group.arn, billing_period)
            {
                let key = row_key(
                    by_billing_period.then_some(billing_period.as_str()),
                    by_product_name.then_some(product_name.as_str()),
                );
```
(the periods themselves are already in hand: `let billing_periods = ...` / `crate::state::billing_periods_in_range(start, end)`, `src/ops/get_billing_group_cost_report.rs:250-285`).

The rule the crate states for exactly a period with no item — `src/state.rs:3957-3961` (`proforma_custom_line_item_charges`):
```
    /// charge with the sign of its type ([`charge_sign`]), under the product it is
    /// presented as ([`custom_line_item_product_name`]). An empty map is a billing group
    /// with no item in effect in that period, so its pro forma cost there is its actual
    /// cost.
```
and `src/state.rs:603-606` (`METERED_AWS_COST`):
```
/// actual charges are zero. Zero is REPORTED as the amount it is rather than withheld or
/// invented, and it is what the pro forma cost — the custom line items the caller really
/// did create — is measured against.
```

The sibling report, which reports the zero summary rather than omitting the element — `src/ops/list_billing_group_cost_reports.rs:233-240`:
```
        let billing_group_cost_reports: Vec<BillingGroupCostReportElement> = page
            .into_iter()
            .map(|group| {
                let proforma_cost: f64 = state
                    .proforma_custom_line_item_charges(&group.arn, &billing_period)
                    .values()
                    .sum();
                let summary = crate::state::MarginSummary::of_proforma(proforma_cost);
```

Test coverage: none reaches the case. `tests/all/billing_group_cost_report.rs:671-704` exercises `GroupBy: ["BILLING_PERIOD"]` over a range in which EVERY period carries a charge (`let expected = [(period(-1), 11.0), (period(0), 6.0), (period(1), 6.0)];` with `assert_eq!(rows.len(), expected.len(), "...expected one row per period...")`), and the chargeless group is only asserted through the NO-breakdown form (`:335`: `cases.push(("a group with no custom line item", em.billing_group("empty").await, 0.0));`), which is the path that does pre-insert the zero row. `tests/all/cost_report_token_breakdown_scope.rs` changes the breakdown but always over populated periods.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. POST /create-pricing-plan, then POST /create-billing-group naming that plan; keep the group's Arn. Call the current billing period M.
3. POST /create-custom-line-item against that group with {"ChargeDetails":{"Type":"FEE","Flat":{"ChargeValue":100}}} and no BillingPeriodRange, so the item applies from M on.
4. POST /get-billing-group-cost-report with {"Arn":"<group>","BillingPeriodRange":{"InclusiveStartBillingPeriod":"M-3","ExclusiveEndBillingPeriod":"M+1"},"GroupBy":["BILLING_PERIOD"]}.
5. Observe ONE result element (the row for M). The three earlier months of the requested range carry no row at all, although each is a period the request asked about and each has a determined summary (AWSCost 0, ProformaCost 0, Margin 0, MarginPercentage 0).
6. POST the same request without "GroupBy": one row is answered, including for a range in which the group has no charge at all — the path that pre-inserts the default row.
7. Repeat steps 4 and 6 against a billing group with NO custom line item: the breakdown answers an empty BillingGroupCostReportResults while the no-breakdown form answers one row with the group's ARN and zero amounts.
8. Compare the sibling reader: POST /list-billing-group-cost-reports with {"BillingPeriod":"M-3","Filters":{"BillingGroupArns":["<group>"]}} reports the group with the zero summary rather than omitting it.
