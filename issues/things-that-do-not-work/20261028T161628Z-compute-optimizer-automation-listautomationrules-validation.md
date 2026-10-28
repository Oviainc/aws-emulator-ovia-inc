Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: eba09938c5757f116b88f59710a766eb7f22de4f

# A list filter carrying an empty values list is accepted and quietly matches nothing, the one thing this crate's own empty-value-list rule refuses

_Filed: 2026-10-28T16:16:28Z_
_Operation: ListAutomationRules | Class: validation_

> _Note: this issue was identified against the code base at commit `eba09938c5757f116b88f59710a766eb7f22de4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`FilterValues` is modelled as a list whose MEMBER carries `length 1..255` but which carries no list-level `@length` of its own, so `filters: [{"name": "Status", "values": []}]` passes the generated boundary walk (`values` is `@required`, and an empty array satisfies `@required`) and reaches the handler.

In the handler the crate then treats that filter as one that matches NOTHING: every filter predicate asks whether the stored field is among `filter.values`, and an empty list contains nothing, so every candidate is dropped and the operation answers 200 with an empty page. The caller named no value to select on, and gets back a well-formed, empty, authoritative-looking result set.

That is exactly the outcome this crate refuses everywhere else it reads a caller-supplied value list, as its own single rule for such a list, stated in `criteria_violation`'s doc: a condition with no values "[cannot] select resources, so each is refused rather than quietly matching nothing" (`src/state.rs:2466-2469`), implemented at `src/state.rs:2603-2608`; an empty `recommendedActionTypes` "selects no action at all" and is refused (`src/state.rs:2641-2654`); an empty `accountIds` "is refused rather than acted on" (`src/state.rs:455-458`). Where the model DOES bound such a list (`OrganizationConfigurationAccountIds`, `@length(min: 1)`), the crate explicitly leans on the boundary for the same rule (`src/state.rs:2177-2180`). The filter path is the one caller-supplied value list that gets neither the boundary bound nor the handler check.

The defect is in the shared filter-matching helpers in `state.rs`, so it shows on all five filtering listings with the same input: `ListAutomationRules` (`rule_matches_filters`, `src/state.rs:3406`), `ListAutomationEvents` and `ListAutomationEventSummaries` (`event_matches_filters`, `src/state.rs:2900`), and `ListRecommendedActions` and `ListRecommendedActionSummaries` (`recommended_action_matches_filters`, `src/state.rs:3481`). The handlers validate the filter's NAME before matching (`src/ops/list_automation_rules.rs:168-175`) precisely "so a filter is never silently ignored" — but say nothing about a filter that carries no values at all.

## Evidence

The filter predicate that turns an empty `values` list into "matches nothing" — `src/state.rs:3384-3407`:
```
pub fn rule_matches_filters(rule: &StoredRule, filters: &[Filter]) -> bool {
    filters.iter().all(|filter| {
        let field: Option<String> = match filter.name.as_str() {
            "Name" => Some(rule.name.clone()),
...
        field.is_some_and(|value| filter.values.contains(&value))
```
and the same shape in its two siblings — `src/state.rs:2900`:
```
        field.is_some_and(|value| filter.values.iter().any(|candidate| candidate == value))
```
`src/state.rs:3481`:
```
        field.is_some_and(|value| filter.values.contains(&value))
```

The crate's OWN rule for a caller-supplied value list that selects nothing — `src/state.rs:2464-2471`:
```
/// The model types `comparison` and `values` as OPTIONAL members of every
/// condition and types `comparison` as the whole `ComparisonOperator` enum, so
/// the protocol boundary admits a condition with no operator, with no values, or
/// with an operator from the wrong family (a `Numeric*` operator on a string
/// criterion). None of those can select resources, so each is refused rather
/// than quietly matching nothing.
```
implemented at `src/state.rs:2603-2608`:
```
    if value_count == 0 {
        return Some(constraint_report(
            &format!("{path}.member.values"),
            "Member must have length greater than or equal to 1",
        ));
    }
```
and the same rule for the other two caller-supplied lists — `src/state.rs:2643-2653`:
```
/// The list names the action types the rule may execute, so an empty one selects
/// no action at all and leaves a rule that automates nothing; the model carries
/// no `@length` on `RecommendedActionTypeList`, so the check lives here.
...
    if action_types.is_empty() {
        return Some(constraint_report(
            "recommendedActionTypes",
            "Member must have length greater than or equal to 1",
```
`src/state.rs:455-465`:
```
/// ([`MAX_ACCOUNT_IDS_PER_REQUEST`]), so a request naming none, and one naming
/// more, is refused rather than acted on for a prefix of the list.
pub fn account_ids_violation(account_ids: &[String]) -> Option<String> {
    if account_ids.is_empty() {
        return Some(constraint_report(
            "accountIds",
            "Member must have length greater than or equal to 1",
```

The handler checks the filter's name but never its values — `src/ops/list_automation_rules.rs:167-175`:
```
        let filters = input.filters.unwrap_or_default();
        // `Filter$name` is typed as the `AutomationRuleFilterName` value set, but
        // a name outside it is refused here too so a filter is never silently
        // ignored.
        if crate::state::unknown_filter_name(&filters).is_some() {
            return Err(invalid_parameter(
                crate::state::unknown_filter_name_message(),
            ));
        }
```

The model cannot refuse it at the boundary: `smithy_query(query="shape",shape="FilterValues")` reports `{"shape_id": "...#FilterValues", "type": "list", "member": {"target": "FilterValue", "length": {"min": 1, "max": 255}, ...}}` — a length on the MEMBER only, no list-level length — while `Filter$values` is `{"target": "FilterValues", "required": true, "type": "list"}`. Contrast `OrganizationConfigurationAccountIds`, which the crate relies on the boundary for because the list itself is bounded — `src/state.rs:2177-2180`:
```
/// An EMPTY account list cannot reach here: the model constrains
/// `OrganizationConfigurationAccountIds` at `length(min: 1, max: 50)`, so the
/// protocol boundary refuses `accountIds: []` before the handler runs.
```

Test coverage: the sibling refusal IS pinned — `tests/all/automation_rule_preview.rs:1007-1010` + `:1024-1031`:
```
        (
            "a condition with no values",
            preview_request(json!({ "region": [{ "comparison": "StringEquals", "values": [] }] })),
        ),
...
    for (case, request) in cases {
        assert_refused(
            call(&router, "ListAutomationRulePreview", &request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
```
(the same case appears in `tests/all/preview_selection_guard.rs:215`). No test anywhere in the suite sends a FILTER with `"values": []` — a search for `"values": []` across `tests/` returns only criteria conditions — so the behaviour reported here is pinned by nothing, and `tests/all/filter_name_refusals.rs` only covers filter NAMES.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate.
2. Call CreateAutomationRule with a minimal valid rule ({"name": "r1", "ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "schedule": {"scheduleExpression": "0 3 * * ? *"}, "status": "Active"}).
3. Call ListAutomationRules with {} — 200, the rule is reported.
4. Call ListAutomationRules with {"filters": [{"name": "Status", "values": []}]} — 200 with "automationRules": [], because src/state.rs:3406 asks whether the rule's status is among an EMPTY value list. The caller named no value to select on and is handed an empty, authoritative-looking page instead of a refusal.
5. Repeat the same empty-values filter against ListAutomationEvents, ListAutomationEventSummaries, ListRecommendedActions and ListRecommendedActionSummaries (after StartAutomationEvent for the event listings): each answers 200 with an empty page/summary list, via src/state.rs:2900 and src/state.rs:3481.
6. Compare the crate's treatment of the same kind of value list elsewhere: ListAutomationRulePreview with {"ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "criteria": {"region": [{"comparison": "StringEquals", "values": []}]}} answers 400 InvalidParameterValueException "... Value at 'criteria.region.member.values' failed to satisfy constraint: Member must have length greater than or equal to 1" (src/state.rs:2603-2608), as tests/all/automation_rule_preview.rs:1007-1031 pins; CreateAutomationRule with "recommendedActionTypes": [] and AssociateAccounts with "accountIds": [] are refused the same way.
