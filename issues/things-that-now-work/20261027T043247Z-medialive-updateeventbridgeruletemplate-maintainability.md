Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# The EventBridge rule-template `eventTargets` rule (and its `bounded` length check) is implemented twice, in the create and the update handler

_Filed: 2026-10-27T04:32:47Z_
_Operation: UpdateEventBridgeRuleTemplate | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole `EventTargets` validation rule — the per-template target-count ceiling, the per-ARN length bound and the "SNS topic or CloudWatch log group" ARN shape check — is written out once in `CreateEventBridgeRuleTemplate` and again, line for line, in `UpdateEventBridgeRuleTemplate` (the only difference is the `Option` wrapper the update needs for "member omitted"). The private `bounded()` length helper is duplicated the same way. The update's own doc comment says it is deliberately the same rule, which is exactly the drift risk: a change to the ARN-kind set, the count ceiling or the refusal wording has to be made in two files or the two operations start disagreeing about the same member of the same resource. Elsewhere this crate already centralises shared validation in `state` (e.g. `crate::state::whitelist_rules_from_request` is called by both `CreateInputSecurityGroup` and `UpdateInputSecurityGroup`, and `crate::state::network_ip_pools` / `network_routes` by both `CreateNetwork` and `UpdateNetwork`), so the single-owner pattern is already established. Folding the target check into one `state`-side helper (returning the validated ARN list) leaves every response and every existing test unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-medialive-medialive/src/ops/create_event_bridge_rule_template.rs:290-316`:

    fn event_targets(
        targets: Option<&[EventBridgeRuleTemplateTarget]>,
    ) -> Result<Vec<String>, CreateEventBridgeRuleTemplateError> {
        let targets = targets.unwrap_or_default();
        if targets.len() > crate::state::MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS {
            return Err(bad_request(format!(
                "eventTargets must carry at most {} targets, but carried {}",
    ...
            if !crate::state::is_event_target_arn(&target.arn) {
                return Err(bad_request(format!(
                    "eventTargets.arn must be the ARN of an SNS topic or a CloudWatch log group, but was {}",

Copy 2 — `src/ops/update_event_bridge_rule_template.rs:268-298`, whose doc admits it is the same rule (lines 266-267):

    /// group") and within the length its target shape names — the same rule the
    /// create applies, because it is the same member of the same resource.
    fn event_targets(
        targets: Option<&[EventBridgeRuleTemplateTarget]>,
    ) -> Result<Option<Vec<String>>, UpdateEventBridgeRuleTemplateError> {
        let Some(targets) = targets else {
            return Ok(None);
        };
        if targets.len() > crate::state::MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS {
            return Err(bad_request(format!(
                "eventTargets must carry at most {} targets, but carried {}",
    ...
            if !crate::state::is_event_target_arn(&target.arn) {
                return Err(bad_request(format!(
                    "eventTargets.arn must be the ARN of an SNS topic or a CloudWatch log group, but was {}",

The `bounded` helper is duplicated too: `create_event_bridge_rule_template.rs:359-375` and `update_event_bridge_rule_template.rs:332-348` are byte-identical apart from the error type.

The already-established single-owner pattern, for contrast: `src/ops/create_input_security_group.rs:169-170` and `src/ops/update_input_security_group.rs:175-177` both call `crate::state::whitelist_rules_from_request(...)` (state.rs:1527); `src/ops/create_network.rs` and `src/ops/update_network.rs:179-192` both call `crate::state::network_ip_pools` / `crate::state::network_routes`.

Tests that cover both copies and must keep passing unchanged: `tests/all/event_bridge_rule_templates.rs`, `tests/all/template_updates.rs`, `tests/all/event_bridge_rule_template_not_found.rs`, `tests/all/event_bridge_rule_template_listing.rs`.

## Steps to Reproduce

1. Open src/ops/create_event_bridge_rule_template.rs and read fn event_targets (lines 290-316) and fn bounded (lines 359-375)
2. Open src/ops/update_event_bridge_rule_template.rs and read fn event_targets (lines 268-298) and fn bounded (lines 332-348) — the count ceiling, the per-ARN length bound, the ARN-kind check and both refusal messages are the same code twice
3. Compare with src/ops/create_input_security_group.rs:169 and src/ops/update_input_security_group.rs:175, where the create and the update share one `crate::state::whitelist_rules_from_request` helper
4. Change the ARN-kind refusal wording or MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS handling in one file only and observe the two operations now answer differently for the same input
