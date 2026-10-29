Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7b0476a706ac79cc0f7894740758ad8ab457f053

# The refusal formatter suppresses the value of Filters.Names, a member the model does not mark @sensitive, contradicting the rule its own module states and the case its own comment claims to handle

_Filed: 2026-10-29T01:41:25Z_
_Operation: ListBillingGroups | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b0476a706ac79cc0f7894740758ad8ab457f053`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service/validation.rs` states one rule for every clause of this service's `ValidationException.Message`: the value is echoed unless the member is one the MODEL marks `@sensitive`. It decides that from the deepest named path segment against a hard-coded list of three segment NAMES (`name`, `names`, `description`), not from the model, and `ListBillingGroupsFilter$Names` is spelled `names` while targeting `StringSearches` — a list of the plain `StringSearch` structure, which carries no `sensitive: true` anywhere in its tree.

`StringSearches` carries its own `@length(min: 1, max: 1)`, so a violation lands on the LIST member itself and the violation path is `filters.names`. `subject()` tests `is_sensitive(path)` BEFORE it consults the already-rendered list values, so that clause comes out as `Value at 'filters.names' failed to satisfy constraint: Member must have length less than or equal to 1` with the list the caller sent withheld, while the structurally identical `filters.arns`, `filters.statuses`, `filters.primaryAccountIds`, `filters.billingGroupTypes` and `filters.responsibilityTransferArns` on the SAME filter all echo theirs.

The module's own comment on `SENSITIVE_MEMBERS` asserts this exact shape is handled correctly, reasoning that the path "ends at `searchValue`, not at `names`". That holds only for a violation on `StringSearch$SearchValue`/`$SearchOption`; it does not hold for the `StringSearches` length bound, which is the one constraint the member itself declares. So the crate suppresses a value it says it keeps, on the one member of this filter where the rule and the code disagree.

## Evidence

The rule the module states — `src/service/validation.rs:17-19`:
```
//! 2. a clause states the submitted value, the member's dotted path and the requirement it
//!    failed — except on a member the model marks `@sensitive`, whose value is never
//!    echoed ([`SENSITIVE_MEMBERS`], [`subject`]);
```

The comment that claims THIS shape is handled correctly — `src/service/validation.rs:55-58`:
```
/// Matched on the deepest NAMED segment of the violation path, which is why
/// `ListBillingGroupsFilter$Names` — a list of `StringSearch`, whose constrained member
/// is the non-sensitive `SearchValue` — correctly keeps its value: that path ends at
/// `searchValue`, not at `names`.
const SENSITIVE_MEMBERS: &[&str] = &["name", "names", "description"];
```
(`src/service/validation.rs:59` is that list.)

The code that breaks it — `src/service/validation.rs:186-200`, where the sensitivity test precedes the list rendering:
```
fn subject(path: &str, violation: &Violation, lists: &HashMap<String, String>) -> String {
    if violation.value.is_none() {
        return "Value null".to_string();
    }
    if is_sensitive(path) {
        return "Value".to_string();
    }
    if let Some(rendered) = lists.get(path) {
        return format!("Value '{rendered}'");
    }
```
and `src/service/validation.rs:234-240`, which matches on the segment NAME only:
```
fn is_sensitive(path: &str) -> bool {
    path.split('.')
        .rfind(|segment| {
            *segment != "member" && !segment.bytes().all(|byte| byte.is_ascii_digit())
        })
        .is_some_and(|segment| SENSITIVE_MEMBERS.contains(&segment))
}
```

The model says the member is NOT sensitive, and says its only own constraint is on the LIST (`smithy_query(query="shape",shape="ListBillingGroupsFilter,StringSearches,StringSearch,SearchValue")`):
```
"Names": {"target": "StringSearches", "length": {"min": 1, "max": 1}, "type": "list", "doc": "<p> Filter billing groups by their names. </p>"}
"StringSearches": {"type": "list", "length": {"min": 1, "max": 1}, "member": "StringSearch"}
"StringSearch": {"type": "structure", "members": {"SearchOption": {...}, "SearchValue": {"target": "SearchValue", "required": true, "length": {"min": 1, "max": 128}, "pattern": "^[a-zA-Z0-9_\\+=\\.\\-@ ]+$"}}}
"SearchValue": {"type": "string", "length": {"min": 1, "max": 128}, "pattern": "^[a-zA-Z0-9_\\+=\\.\\-@ ]+$"}
```
No `sensitive: true` anywhere in that tree — unlike `ListCustomLineItemsFilter$Names`, whose `CustomLineItemNameList` member IS `"sensitive": true`, which is the case the heuristic was written for.

The constraint really does reach the formatter: `src/ops/types/list_billing_groups_types.rs:14` declares `ListBillingGroupsInput` with NO `skip_validation`, so the boundary walk enforces `StringSearches`' length (contrast `src/ops/types/tag_resource_types.rs:22-25`, which opts out explicitly), and `src/service.rs:265` registers `list_billing_groups::ListBillingGroups` in the `PreHandlerChecks` table that words the refusal through `refusal_message`.

Test coverage: `tests/all/validation_messages.rs:264-276` pins only the genuinely `@sensitive` case (`UpdatePricingPlan` `name` -> "Value at 'name' failed to satisfy constraint: ..."), and `tests/all/billing_group_core_loop.rs:1031-1042` / `tests/all/billing_group_billing_periods.rs:671-676` exercise `Filters.Names` only with conforming single-element lists (`json!({"Filters": {"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "Alpha"}]}})`). Nothing asserts the message for a `Filters.Names` length violation either way, so echoing it changes no existing test.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. POST /list-billing-groups with body {"Filters": {"Names": []}} — an empty list, which violates StringSearches' @length(min: 1).
3. Observe HTTP 400 ValidationException whose Message reads "1 validation error detected: Value at 'filters.names' failed to satisfy constraint: Member must have length greater than or equal to 1" — the list the caller sent is withheld.
4. POST /list-billing-groups with body {"Filters": {"Arns": []}} (BillingGroupArnList is also @length(min: 1), and `arns` is not in SENSITIVE_MEMBERS): the clause for the same class of violation reads "Value '[]' at 'filters.arns' ...", echoing the list.
5. POST /list-billing-groups with body {"Filters": {"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": ""}]}}: the violation path ends at `searchValue`, so that value IS echoed — showing the suppression is keyed on the segment name `names` rather than on any @sensitive trait the model declares.
