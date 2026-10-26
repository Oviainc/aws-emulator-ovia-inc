Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 22964030325ab1e634e5ef0028a831687ff48548

# The preview request's three-clause validation is implemented twice, once in each preview handler, while the selection it validates lives once in state.rs

_Filed: 2026-10-26T00:01:19Z_
_Operation: ListAutomationRulePreview | Class: maintainability_

> _Note: this issue was identified against the code base at commit `22964030325ab1e634e5ef0028a831687ff48548`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::previewed_actions` exists precisely so that the two preview operations cannot disagree about what a preview request selects — its doc says the selection "lives here once". The VALIDATION of that same selection, however, is copy-pasted into both handlers: the empty-`recommendedActionTypes` check, the `organizationScope`-only-for-`OrganizationRule` check (including its message text) and the `criteria` check appear verbatim in `list_automation_rule_preview.rs` and `list_automation_rule_preview_summaries.rs`, as does the token-fingerprint tuple over the same five members. Two copies of one rule can drift: a change to which selections are refusable (or to the `organizationScope` wording) applied to one handler silently leaves the other accepting a request its sibling refuses, even though both then feed the same `previewed_actions`.

The fix is a single `crate::state` helper (e.g. `preview_selection_violation(rule_type, organization_scope, recommended_action_types, criteria) -> Option<String>`) that both handlers call, beside the `previewed_actions` it guards. The clause order and every message string are preserved, so all responses and both operations' existing tests stay byte-identical.

## Evidence

The crate's own statement that the preview selection is shared code — `src/state.rs:1360-1364`:
```
/// A preview request names its selection the same way whichever preview
/// operation reads it — `ListAutomationRulePreview` reports the selected actions
/// one by one and `ListAutomationRulePreviewSummaries` aggregates the very same
/// set — so the selection lives here once.
```

Copy 1 — `src/ops/list_automation_rule_preview.rs:201-221`:
```
if let Some(report) =
    crate::state::recommended_action_types_violation(&input.recommended_action_types)
{
    return Err(invalid_parameter(report));
}
if input.rule_type != "OrganizationRule" && input.organization_scope.is_some() {
    return Err(invalid_parameter(crate::state::constraint_report(
        "organizationScope",
        "Member is valid only for the OrganizationRule rule type",
    )));
}
if let Some(report) = input
    .criteria
    .as_ref()
    .and_then(crate::state::criteria_violation)
{
    return Err(invalid_parameter(report));
}
```

Copy 2 — `src/ops/list_automation_rule_preview_summaries.rs:207-227`: the same three clauses in the same order with the same strings:
```
if let Some(report) =
    crate::state::recommended_action_types_violation(&input.recommended_action_types)
{
    return Err(invalid_parameter(report));
}
if input.rule_type != "OrganizationRule" && input.organization_scope.is_some() {
    return Err(invalid_parameter(crate::state::constraint_report(
        "organizationScope",
        "Member is valid only for the OrganizationRule rule type",
    )));
}
if let Some(report) = input
    .criteria
    .as_ref()
    .and_then(crate::state::criteria_violation)
{
    return Err(invalid_parameter(report));
}
```

Tests that pin the behaviour the extraction must not change: `tests/all/automation_rule_preview.rs:969-1000` (`a_request_the_service_cannot_evaluate_is_refused`, covering `recommendedActionTypes: []` and `organizationScope` on an `AccountRule`) and the matching `tests/all/automation_rule_preview_summaries.rs:926-960`, plus the token-binding tests `automation_rule_preview.rs:879` and `automation_rule_preview_summaries.rs:795`.

## Steps to Reproduce

1. Open `src/ops/list_automation_rule_preview.rs` at lines 201-221 and `src/ops/list_automation_rule_preview_summaries.rs` at lines 207-227 side by side: the three validation clauses and their message strings are identical.
2. Note `src/state.rs:1360-1364`, where the crate states that the preview selection itself is kept in one place for exactly this reason, and `src/state.rs:1365-1386` (`previewed_actions`), which both handlers call.
3. Change the `organizationScope` requirement text (or add a fourth clause) in one handler only and run the suite: `tests/all/automation_rule_preview.rs` and `tests/all/automation_rule_preview_summaries.rs` now describe two different contracts for one request shape.
