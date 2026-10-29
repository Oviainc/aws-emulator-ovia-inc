Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# A cost-report NextToken is not scoped to the GroupBy breakdown that minted it, so another breakdown reads it as a cut point and serves an empty report

_Filed: 2026-10-29T00:02:13Z_
_Operation: GetBillingGroupCostReport | Class: pagination_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetBillingGroupCostReport` mints its `NextToken` under the scope `page_scope_within(Self::NAME, &group.arn)` — the operation name plus the billing group ARN — and nothing else. But the sequence this operation pages over is not determined by the group alone: the rows, and the CURSOR KEY of every row, are built out of the request's `GroupBy` (and the periods its `BillingPeriodRange` names). `row_key` is `"<billing period>#<product name>"` where each half is present only when `GroupBy` asked for that attribute, so the keyspace a cursor points into changes shape with `GroupBy`:

* `GroupBy: ["PRODUCT_NAME"]` pages over keys `"#<product>"`;
* `GroupBy: ["BILLING_PERIOD","PRODUCT_NAME"]` pages over keys `"<period>#<product>"`;
* no `GroupBy` at all pages over the single key `"#"`.

Because the scope does not carry `GroupBy`, a token minted by one of those reads is ACCEPTED by another: `decode_next_token` matches the scope, and `paginate_bounded` then keeps only the rows whose key is `>= cursor`. A cursor from a finer breakdown is lexically above every key of a coarser one, so the second page is silently EMPTY (and carries no `NextToken`) instead of being refused — the caller is told the billing group has no margin summary rows at all. Narrowing `GroupBy` from `["BILLING_PERIOD","PRODUCT_NAME"]` to `["BILLING_PERIOD"]` does the same thing: every row of the coarser sequence sorts below the cursor and the whole remainder of the report is dropped. The same hole exists for `BillingPeriodRange`, which decides which period rows exist at all.

This is the ONLY paginated read of the crate whose cursor keyspace is derived from an input member that the scope omits. Its siblings are consistent with the rule `page_scope_within` states: `ListCustomLineItemVersions` keys on the generation's position in `item.versions()`, `ListResourcesAssociatedToCustomLineItem` on the association ARN and `ListPricingRulesAssociatedToPricingPlan` on the rule ARN — in each of those the request's own filters only SELECT elements of one fixed sequence, exactly as the helper's doc says a filter does. `GroupBy` is not a filter: it decides what one element IS.

The crate already has the machinery for the fix (the scope string is the one thing the token carries besides the cursor), so the token either names the breakdown it was minted under or is refused with the crate's own `invalid_next_token` (`CANNOT_PARSE`) — the answer every other token this service did not mint for this sequence gets.

## Evidence

The scope the token is minted under — `src/ops/get_billing_group_cost_report.rs:349-356`:
```
        let (billing_group_cost_report_results, next_token) = crate::state::paginate_bounded(
            &crate::state::page_scope_within(Self::NAME, &group.arn),
            results,
            input.next_token.as_deref(),
            input.max_results,
            MAX_PAGE_SIZE,
        )
        .map_err(GetBillingGroupCostReportErrorModeled::ValidationException)?;
```

The cursor key, built from the REQUEST's breakdown — `src/ops/get_billing_group_cost_report.rs:114-125`:
```
/// The cursor key of one row — what orders the rows and what a `NextToken` names.
///
/// It is built from the row's OWN attribute values, so the rows run by billing period
/// (chronological, since a period is enumerated in the canonical zero-padded spelling)
/// and then by product name, and resuming from a token lands on the row it named.
fn row_key(billing_period: Option<&str>, product_name: Option<&str>) -> String {
    format!(
        "{}#{}",
        billing_period.unwrap_or_default(),
        product_name.unwrap_or_default()
    )
}
```
and the two halves are present only when `GroupBy` named them — `src/ops/get_billing_group_cost_report.rs:285-303`:
```
        let by_billing_period = group_by.contains(&GROUP_BY_BILLING_PERIOD);
        let by_product_name = group_by.contains(&GROUP_BY_PRODUCT_NAME);
...
                let key = row_key(
                    by_billing_period.then_some(billing_period.as_str()),
                    by_product_name.then_some(product_name.as_str()),
                );
```
(and a request with no `GroupBy` has exactly one row, key `"#"` — `:292-295`: `if group_by.is_empty() { rows.insert(row_key(None, None), ReportRow::default()); }`).

The rule the scope is supposed to satisfy, stated on the helper the handler calls — `src/state.rs:4019-4029`:
```
/// The scope of the tokens a listing read WITHIN one resource mints: the operation and
/// that resource.
///
/// The scope identifies the SEQUENCE a cursor points into, and for these reads that
/// sequence belongs to one parent resource (a plan's rules, an item's versions, a group's
/// cost rows), so a cursor minted while paging one parent names no position in another's
/// sequence. The filters a request narrows a sequence with are not part of it: they select
/// which elements of the one sequence are reported, not which sequence is being read.
pub fn page_scope_within(operation: &str, resource_arn: &str) -> String {
```

What accepting the foreign cursor then does — `src/state.rs:4092-4102`:
```
    let start = match next_token {
        Some(token) => Some(decode_next_token(scope, token).ok_or_else(invalid_next_token)?),
        None => None,
    };
    let mut page: Vec<(String, T)> = match start.as_deref() {
        Some(start) => items
            .into_iter()
            .filter(|(key, _)| key.as_str() >= start)
            .collect(),
        None => items,
    };
```

Test coverage: none for this dimension, and the suite says so — `tests/all/pagination_token_opacity.rs:57-62` explains that this operation mints no token over the fixture (`@range(200..300)`) and only pins the cross-PARENT case (`other_parent: Some(json!({ "Arn": groups[1] }))`, `mints: false`, `:370-377`). `tests/all/billing_group_cost_report.rs` walks minted tokens but never changes `GroupBy` between pages.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. POST /create-pricing-plan, then POST /create-billing-group naming that plan; keep the group's Arn.
3. POST /create-custom-line-item 201 times against that group, each with {"ChargeDetails":{"Type":"FEE","Flat":{"ChargeValue":1.0}}} and a DISTINCT "PresentationDetails":{"Service":"Svc<N>"} (MAX_CUSTOM_LINE_ITEMS is 1000, so 201 items are storable), which gives the report 201 distinct product rows.
4. POST /get-billing-group-cost-report with {"Arn":"<group arn>","GroupBy":["PRODUCT_NAME"],"MaxResults":200} — 200 rows plus a NextToken.
5. POST /get-billing-group-cost-report again with {"Arn":"<group arn>","NextToken":"<that token>"} and NO GroupBy.
6. Observe HTTP 200 with an EMPTY BillingGroupCostReportResults and no NextToken: the token was accepted (its scope is only the operation plus the group ARN) and the one row of the no-breakdown sequence, key "#", sorts below the cursor "#Svc<N>", so the whole report is dropped instead of the token being refused with the crate's CANNOT_PARSE ValidationException.
7. Repeat with the first call using GroupBy ["BILLING_PERIOD","PRODUCT_NAME"] over a two-month BillingPeriodRange and the second using GroupBy ["BILLING_PERIOD"]: the same empty page, although both calls name the same group.
8. Compare: the same experiment against another group's token IS refused 400 ValidationException, because the parent resource — but not the breakdown — is part of the token's scope.
