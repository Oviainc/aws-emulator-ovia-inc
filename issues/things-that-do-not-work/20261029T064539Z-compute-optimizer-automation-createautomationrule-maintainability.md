Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# Three rule-value rules the create and the update both hold a request to are written out twice, against the crate's own single-home convention for a shared rule

_Filed: 2026-10-29T06:45:39Z_
_Operation: CreateAutomationRule | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAutomationRule` and `UpdateAutomationRule` enforce the same three rules the Smithy constraints cannot carry, each spelled out separately inside its own handler, with the requirement text duplicated verbatim in both places:

1. `name` must not be empty ("Member must have length greater than or equal to 1");
2. `priority` must be a decimal 0..1 with up to 30 fractional digits ("Member must be a decimal number between 0 and 1 with up to 30 digits after the decimal point");
3. an `OrganizationRule` must carry an `organizationConfiguration` ("Member is required for the OrganizationRule rule type").

That is the one shape of duplication this crate otherwise refuses: every other rule two operations share is written ONCE in `crate::state` precisely so the two cannot drift over which requests they refuse or how they word the refusal (`account_ids_violation` for the association pair, `preview_selection_violation` for the preview pair, `tag_keys_violation` / `duplicate_tag_key_violation` for the tagging paths, `recommended_action_types_violation`, `schedule_violation`, `criteria_violation` — which the create and update both reach for). Only these three clauses are copies, so a change to one copy (a reworded requirement, a widened bound) silently applies to one of the two operations.

The fix is to move the three clauses beside the helpers they sit among in `src/state.rs` and have both handlers call them; the messages are already byte-identical in the two copies, so every response and every existing test stays unchanged. Covered by tests/all/automation_rule_validation.rs and tests/all/rule_value_validation.rs (both drive the clauses over create and update), plus tests/all/rule_projection.rs for the accepted values.

## Evidence

Copy 1 — src/ops/create_automation_rule.rs:442-462:
```
    if input.name.is_empty() {
        return Some(crate::state::constraint_report(
            "name",
            "Member must have length greater than or equal to 1",
        ));
    }
    if input.rule_type == "OrganizationRule" && input.organization_configuration.is_none() {
        return Some(crate::state::constraint_report(
            "organizationConfiguration",
            "Member is required for the OrganizationRule rule type",
        ));
    }
    if let Some(priority) = input.priority.as_deref() {
        if !crate::state::priority_is_valid(priority) {
            return Some(crate::state::constraint_report(
                "priority",
                "Member must be a decimal number between 0 and 1 with up to 30 digits after the \
                 decimal point",
            ));
```
Copy 2 — src/ops/update_automation_rule.rs:226-242 and 373-378:
```
        if let Some(priority) = input.priority.as_deref() {
            if !crate::state::priority_is_valid(priority) {
                return Err(invalid_parameter(crate::state::constraint_report(
                    "priority",
                    "Member must be a decimal number between 0 and 1 with up to 30 digits after \
                     the decimal point",
                )));
        ...
        if input.name.as_deref().is_some_and(str::is_empty) {
            return Err(invalid_parameter(crate::state::constraint_report(
                "name",
                "Member must have length greater than or equal to 1",
            )));
        ...
        if merged_rule_type == "OrganizationRule" && merged_org_config.is_none() {
            return Err(invalid_parameter(crate::state::constraint_report(
                "organizationConfiguration",
                "Member is required for the OrganizationRule rule type",
            )));
```
The convention both copies break, stated in the module that owns every other shared rule — src/state.rs:1465-1469 (`preview_selection_violation`): "The selection a preview request names is read by both preview operations ([`previewed_actions`]), so what makes a selection refusable is judged here once as well: a request one preview operation cannot evaluate is one the other cannot evaluate either, and the two cannot drift apart over which requests they refuse or how they word the refusal." — and src/state.rs:448-452 (`account_ids_violation`): "...so what makes it refusable is judged here once as well ... and the two cannot drift apart over which requests they refuse, over the bound they refuse them at or over how they word the refusal."

## Steps to Reproduce

1. Read src/ops/create_automation_rule.rs:441-487 — the create's `invalid_parameter` holds the name / organizationConfiguration / priority clauses inline
2. Read src/ops/update_automation_rule.rs:224-242 and 364-378 — the update holds the same three clauses with the same requirement strings inline
3. Read src/state.rs:444-472 and src/state.rs:1461-1496 — every other rule two operations share is single-homed in state.rs, with the doc stating that is so the two cannot drift over which requests they refuse or how they word the refusal
4. Change one copy's requirement wording (or widen `name` to allow the empty string) and observe that only one of the two operations changes, while tests/all/automation_rule_validation.rs and tests/all/rule_value_validation.rs assert the same wording for both
