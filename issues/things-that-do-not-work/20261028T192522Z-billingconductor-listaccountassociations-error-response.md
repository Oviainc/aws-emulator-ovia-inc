Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8ff612b8d27f8140aa4881a7fd23caef0de9fa96

# The billing-group Association filter is a resource lookup (404) instead of a predicate, so it refuses an ARN the same listing reports for that period

_Filed: 2026-10-28T19:25:22Z_
_Operation: ListAccountAssociations | Class: error-response_

> _Note: this issue was identified against the code base at commit `8ff612b8d27f8140aa4881a7fd23caef0de9fa96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAccountAssociations` treats `Filters.Association` — when it carries a billing group ARN rather than `MONITORED`/`UNMONITORED` — as a resource LOOKUP: an ARN that is not a key of `state.billing_groups` right now is answered `ResourceNotFoundException` (HTTP 404) instead of simply selecting no account.

That contradicts this crate twice over.

1. The crate states the opposite rule for its own billing-group ARN filters, in so many words: `ListBillingGroupCostReports` says a filter "is a predicate rather than a resource lookup: an ARN that names no stored group selects none, exactly as the `Arns` filter of `ListBillingGroups` does", and `ListBillingGroups::matches` implements exactly that (`return Ok(false)`, never a 404). All three operations model `ResourceNotFoundException`, so the model is not what forces the difference — the handlers simply disagree.

2. It makes the refusal observably wrong, not just differently worded. `remove_billing_group` deliberately keeps the memberships a deleted group held so that "a read scoped to a billing period the group existed in still has to report the accounts it held then", and the suite pins that: after `DeleteBillingGroup`, `ListAccountAssociations(BillingPeriod = <an earlier period>)` still reports that group's ARN as the account's `BillingGroupArn`. Yet `ListAccountAssociations(BillingPeriod = <the same period>, Filters.Association = <that same ARN>)` answers 404 "the billing group does not exist" — the listing reports an ARN that its own filter calls nonexistent, so a caller cannot narrow a past period to a group that has since been deleted. The crate even holds the record needed to decide the question (`DeletedBillingGroupRecord::existed_in` / `deleted_billing_group_occupying_name`), and the filter path consults neither.

The defect is the existence check itself: the filter should select the accounts whose membership in the period asked about names that ARN, exactly as the `MONITORED` / `UNMONITORED` / `AccountId` / `AccountIds` arms of the same `for` loop do.

## Evidence

The lookup — `src/ops/list_account_associations.rs:192-211`:
```
        // An `Association` that is neither MONITORED nor UNMONITORED is a billing
        // group ARN (or bare id) to restrict the listing to. One that names no stored
        // group is a reference to a resource that doesn't exist.
        let association_group = match filters.association.as_deref() {
            None | Some(MONITORED | UNMONITORED) => None,
            Some(value) => {
                let arn = crate::state::normalize_billing_group_arn(value);
                if !state.billing_groups.contains_key(&arn) {
                    return Err(ListAccountAssociationsErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: "the billing group does not exist".to_string(),
```

The crate's own rule for the SAME kind of filter — `src/ops/list_billing_group_cost_reports.rs:180-185`:
```
        // "A `ListBillingGroupCostReportsFilter` to specify billing groups to retrieve
        // reports from": its one member NARROWS the listing to the groups it names, in
        // either spelling the `BillingGroupArn` pattern admits, so it is normalised to the
        // identity the store keys by. An absent filter narrows nothing, and a filter is a
        // predicate rather than a resource lookup: an ARN that names no stored group
        // selects none, exactly as the `Arns` filter of `ListBillingGroups` does.
```
and the sibling implementation — `src/ops/list_billing_groups.rs:143-151`:
```
    if let Some(arns) = &filters.arns {
        let wanted: Vec<String> = arns
            .iter()
            .map(|arn| crate::state::normalize_billing_group_arn(arn))
            .collect();
        if !wanted.contains(&group.arn) {
            return Ok(false);
        }
    }
```

Why the 404 is reachable for an ARN the listing itself reports — `src/state.rs:2635-2637` (`remove_billing_group`):
```
        // The memberships the group held END with it, rather than being forgotten: a
        // read scoped to a billing period the group existed in still has to report
        // the accounts it held then.
```

Test coverage, which pins the two halves separately and never crosses them:
- `tests/all/account_association_billing_periods.rs:451-454` — `a_membership_ended_by_a_release_or_a_delete_still_covers_its_own_periods`: "RULE: how a membership ended does not change what the periods it ran for report — a disassociation and a deleted billing group both end it at the period the call was made in, so an earlier period still reports the group". That test reads through `association_in` (`Filters.AccountId`, `:228`), never through the group-ARN filter.
- `tests/all/account_association_billing_periods.rs:342` — "// The filters select on the same per-period answer.", asserted at `:358` with `em.filtered_accounts(&case, &group, Some(&period))`, but only for groups that are still live (`a_standing_membership_is_reported_from_the_period_it_began_in_onward`).
So no test pins the 404 for a since-deleted group, and the suite's own stated rule is that the group filter must agree with the `BillingGroupArn` the listing reports.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `payer_account_id`, as the suites do).
2. POST /create-pricing-plan, then POST /create-billing-group (name G) and POST /associate-accounts to put account A into G.
3. POST /list-account-associations with {"Filters":{"Association":"<G arn>"}} and observe 200 with A — the filter works while G is live.
4. POST /delete-billing-group with {"Arn":"<G arn>"}.
5. POST /list-account-associations with {"BillingPeriod":"<a period the membership covered>","Filters":{"AccountId":"<A>"}} and observe 200 reporting BillingGroupArn = <G arn> (the membership is deliberately kept).
6. POST /list-account-associations with {"BillingPeriod":"<the same period>","Filters":{"Association":"<G arn>"}} and observe 404 ResourceNotFoundException "the billing group does not exist" instead of the 200 listing A.
7. Compare POST /list-billing-groups with {"Filters":{"Arns":["<G arn>"]}} and POST /list-billing-group-cost-reports with {"Filters":{"BillingGroupArns":["<G arn>"]}}: both answer 200 with an empty list for the same unresolvable ARN.
