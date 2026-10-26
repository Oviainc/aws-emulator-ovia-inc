Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 465e7599899ea277984bee5e564f3564a11f5f81

# NotEqual filter selects instances whose addressed member holds no value, contradicting the handler's own rule

_Filed: 2026-10-26T05:30:45Z_
_Operation: ListLinuxSubscriptionInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `465e7599899ea277984bee5e564f3564a11f5f81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListLinuxSubscriptionInstances` projects each discovered record onto the member a filter's `Name` addresses through `filtered_member`, whose doc states that a member the record leaves unset holds NO value and so cannot be selected on by any filter. The shared comparator `crate::state::filter_selects` breaks that rule for the `NotEqual` operator: it computes `equals` over the (possibly empty) list of held values and answers `!equals`, so a record that holds NOTHING for the addressed member satisfies every `NotEqual` filter and is returned.

Concretely, for the optional members `DualSubscription` / `OsVersion` / `SubscriptionProvider*Time` (all `Option<String>` on `DiscoveredInstanceRecord`) and for the list member `ProductCode` (a record may carry an empty `Vec`), a request such as `Filters: [{"Name": "DualSubscription", "Operator": "NotEqual", "Values": ["Yes"]}]` returns every instance that carries no `DualSubscription` at all, even though the handler documents those records as unselectable, and even though the response entries then carry no `DualSubscription` member for the client to see why they matched. The same holds for `ProductCode` `NotEqual` against an instance whose AMI reported no product codes.

The two in-crate statements cannot both hold: either `filtered_member`'s rule ("which no filter can then select on") is wrong, or `filter_selects` must not treat "holds nothing" as "differs from everything". One of them has to be corrected; today the code silently follows neither consistently (an unset member is unselectable by `Equal`/`Contains`, but selected by `NotEqual`).

## Evidence

Handler-side rule (src/ops/list_linux_subscription_instances.rs:218-222):
```
/// What the member a filter `Name` addresses holds on `record`: one value for a scalar
/// member, every value for the list member `ProductCode`, and none for a member the
/// record leaves unset (which no filter can then select on).
```
and its projection returning an empty vector for an unset member (src/ops/list_linux_subscription_instances.rs:227,230):
```
"DualSubscription" => record.dual_subscription.clone().into_iter().collect(),
...
"ProductCode" => record.product_codes.clone(),
```
The comparator it calls (src/state.rs:820-831):
```
pub fn filter_selects(filter: &Filter, member_values: &[String]) -> bool {
    let values = filter.values.as_deref().unwrap_or_default();
    let equals = member_values.iter().any(|held| values.contains(held));
    match filter.operator.as_deref().unwrap_or(OPERATOR_EQUAL) {
        ...
        OPERATOR_NOT_EQUAL => !equals,
```
With `member_values` empty, `equals` is `false`, so `!equals` is `true` — the record IS selected. The optional members that reach this path (src/state.rs:244-253): `pub os_version: Option<String>`, `pub dual_subscription: Option<String>`.

Test coverage: `tests/all/linux_subscriptions.rs:538-564` exercises `DualSubscription` only with `Equal` (`("DualSubscription", "Equal", &["Yes"], &["i-0d4"])`, `("DualSubscription", "Equal", &["No"], &["i-0a1"])`) and `NotEqual` only on `Status`, which every fixture row sets (`("Status", "NotEqual", &["running"], &["i-0c3", "i-0f6"])`) — so the unset-member case is untested, and the fixture table deliberately contains rows with `DualSubscription` unset (`tests/all/linux_subscriptions.rs:100,110,139,151`) and one with no product codes at all (`tests/all/linux_subscriptions.rs:125`).

## Steps to Reproduce

1. Seed the store with two discovered instances the way `tests/all/linux_subscriptions.rs::discover` does: one with `dual_subscription: Some("No")` and one with `dual_subscription: None`.
2. POST /subscription/ListLinuxSubscriptionInstances with body {"Filters":[{"Name":"DualSubscription","Operator":"NotEqual","Values":["Yes"]}]}.
3. Observe both instances in `Instances`, including the one that holds no `DualSubscription` value and reports no `DualSubscription` member in the response.
4. Compare with the handler's own rule at src/ops/list_linux_subscription_instances.rs:218-222, which states a member the record leaves unset is one 'no filter can then select on'.
5. Repeat with {"Name":"ProductCode","Operator":"NotEqual","Values":["pc-rhel"]} against an instance whose `product_codes` is empty: it is selected too.
