Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a7f9a115bc9a804744e4f0c3783ea962e700da67

# A cost-report NextToken is not scoped to the BillingPeriod that minted it, so another period reads it as a cut point

_Filed: 2026-11-09T10:54:02Z_
_Operation: ListBillingGroupCostReports | Class: pagination_

> _Note: this issue was identified against the code base at commit `a7f9a115bc9a804744e4f0c3783ea962e700da67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingGroupCostReports` mints and reads its pagination token under a scope that is only the operation name (`Self::NAME`), although `BillingPeriod` is exactly the kind of request member the crate's own token-scope rule says has to travel in the scope: it decides which groups the sequence holds (the groups that were billing in that month) AND what every reported element's four amounts cover (`AWSCost` / `ProformaCost` / `Margin` / `MarginPercentage` are the charges of THAT period).

The consequence is a silently wrong page rather than a refusal. A token minted while paging one month decodes under another month's request, because `decode_next_token` only compares the scope, which is identical. The second call then resumes a DIFFERENT sequence at the ARN cursor of the first: every group of the second period whose ARN sorts below the cursor is skipped, and because the truncated remainder fits in one page the caller is handed no `NextToken` and cannot tell that it never saw them. The crate states the opposite outcome for such a token — it is to be "refused with [`invalid_next_token`] ... instead of being read as a cut point into a keyspace it names nothing in".

The sibling cost report already does it the documented way: `GetBillingGroupCostReport` puts the canonical months its range enumerates into the scope through `page_scope_shaped`, for the very reason quoted below. The two readers of the same margin summary therefore disagree about whether the period a report is of is part of the sequence a cursor points into. (The other period-scoped listings of this crate — `ListBillingGroups`, `ListCustomLineItems`, `ListPricingPlans`, `ListPricingRules`, `ListResourcesAssociatedToCustomLineItem` — pass their `BillingPeriod` over in the same way; this operation is the one where the crate's wording applies literally, since its elements ARE period-scoped amounts.)

## Evidence

The crate's rule for what travels in a token's scope — `src/state/pagination.rs:132-140`:

    /// [`page_scope_within`] is the whole scope of a read whose request members only SELECT
    /// elements of one fixed sequence. A member that decides what ONE element IS — which
    /// attributes a reported group aggregates over, and so what the cursor key of an element
    /// even spells, or which periods an element's amounts cover — names a DIFFERENT sequence
    /// for every value it takes, and a cursor into one of those sequences names no position in
    /// another, so it travels in the scope: a token minted under one shape is then refused with
    /// [`invalid_next_token`], the answer every token this service did not mint for the
    /// sequence being read gets, instead of being read as a cut point into a keyspace it names
    /// nothing in.

The handler nevertheless mints under the bare operation name — `src/ops/list_billing_group_cost_reports.rs:220-226`:

        let (page, next_token) = crate::state::paginate(
            Self::NAME,
            selected,
            input.next_token.as_deref(),
            input.max_results,
        )

while the same handler's period decides both the membership of the sequence and each element's amounts — `src/ops/list_billing_group_cost_reports.rs:178-180` and `:236-240`:

        let billing_period = input
            .billing_period
            .unwrap_or_else(crate::state::current_billing_period);
    ...
                let proforma_cost: f64 = state
                    .proforma_custom_line_item_charges(&group.arn, &billing_period)
                    .values()
                    .sum();

The sibling report does carry the periods in the scope — `src/ops/get_billing_group_cost_report.rs:387-407`:

        // The sequence this read pages over is NOT fixed by the billing group alone: the
        // breakdown decides what one row IS and what its cursor key spells, and the range
        // decides which periods a row's amounts cover and so which rows exist at all. Both
        // therefore travel in the scope of the token a truncated page mints
    ...
        let (billing_group_cost_report_results, next_token) = crate::state::paginate_bounded(
            &crate::state::page_scope_shaped(
                Self::NAME,
                &group.arn,
                &[&breakdown, &periods_covered],
            ),

and only the scope is compared when a token is read back — `src/state/pagination.rs:100-104`:

        let (minted_for, cursor) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
        if minted_for != scope {
            return None;
        }

Test coverage: `tests/all/pagination_token_opacity.rs:292-332` drives this listing with `body: json!({})` — no `BillingPeriod` at all — and `tests/all/cost_report_token_breakdown_scope.rs:6-14` pins the cross-sequence rule for `GetBillingGroupCostReport` only ("the billing periods the range covers (which decide which rows exist at all and what each one's amounts cover) ... a token handed back on a request that names a DIFFERENT sequence is refused"). No test pins the current cross-period acceptance for `ListBillingGroupCostReports`.

## Steps to Reproduce

1. Start the emulator with the emulated account designated the payer account (as the suites do: `engine.state::<BillingConductorState>().lock().payer_account_id = Some(emulator_account_id())`).
2. POST /create-billing-group twice or more so the account holds more groups than one page (e.g. 3 groups), each with `ComputationPreference.PricingPlanArn = arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan`.
3. POST /list-billing-group-cost-reports with `{"BillingPeriod": "<previous billing period>", "MaxResults": 1}` -> a page of 1 and a `NextToken`.
4. POST /list-billing-group-cost-reports with `{"BillingPeriod": "<current billing period>", "NextToken": "<the token from step 3>"}`.
5. Observed: HTTP 200 with a page of the CURRENT period's groups cut at the previous period's cursor — the groups whose ARN sorts below that cursor are omitted and no NextToken reports them. Expected (per `src/state/pagination.rs:132-140`): the modelled 400 ValidationException with Reason CANNOT_PARSE and Fields naming NextToken, because the token was not minted for the sequence being read.
