Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: fc350f637d730e9c7a46ff0b276c9700aec9957e

# An organizationConfiguration sent with ruleType=AccountRule is stored instead of refused, although both preview handlers refuse the analogous organizationScope for exactly that reason

_Filed: 2026-10-29T18:25:45Z_
_Operation: CreateAutomationRule | Class: validation_

> _Note: this issue was identified against the code base at commit `fc350f637d730e9c7a46ff0b276c9700aec9957e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAutomationRule` checks only ONE half of the rule-type/organization-configuration relationship: it refuses an `OrganizationRule` that carries no `organizationConfiguration`, but accepts an `AccountRule` that DOES carry one, stores it on the rule and reports it back from every read path.

The crate's own sibling handlers refuse the mirror-image request. Both preview operations route through `crate::state::preview_selection_violation`, which refuses an `organizationScope` whenever `ruleType` is not `OrganizationRule`, and its doc states the reason: the member "names nothing for a rule type scoped to one account, and silently ignoring a member the caller sent would be worse than reporting it". `UpdateAutomationRule`'s own comment agrees that the configuration is inert on anything but an organization rule ("the `OrganizationRule` type, which is what puts a configuration into effect"), yet it applies the same one-sided check as the create path (it only verifies that a MERGED `OrganizationRule` has a configuration, never that a merged `AccountRule` has none).

Two observable consequences inside this crate:

1. The stored `AccountRule` carries a configuration that, by the crate's own reading, puts nothing into effect, and `GetAutomationRule` / `ListAutomationRules` report it as if it did.
2. `crate::state::rule_matches_filters` reads `organization_configuration.rule_apply_order` without regard to `rule_type`, so the `OrganizationConfigurationRuleApplyOrder` filter selects such an `AccountRule` on a member that is inert for it.

The fix is the missing half of the clause: an `organizationConfiguration` on a rule whose (merged) `ruleType` is not `OrganizationRule` is refused with the same `InvalidParameterValueException` the preview operations already answer, before anything is stored. The same hole exists on `UpdateAutomationRule` (an update that sets `ruleType: AccountRule`, or that writes a configuration onto a stored `AccountRule`).

## Evidence

The check the siblings make — services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/state.rs:1621-1626 (`preview_selection_violation`, called by BOTH preview handlers):
```rust
    if rule_type != "OrganizationRule" && organization_scope.is_some() {
        return Some(constraint_report(
            "organizationScope",
            "Member is valid only for the OrganizationRule rule type",
        ));
    }
```
with its stated reason at state.rs:1607-1610:
"* an `organizationScope` is the set of accounts of an ORGANIZATION-wide preview, so it names nothing for a rule type scoped to one account, and silently ignoring a member the caller sent would be worse than reporting it;"

The check CreateAutomationRule skips — src/ops/create_automation_rule.rs:448-453 (`invalid_parameter`), which tests only the converse:
```rust
    if input.rule_type == "OrganizationRule" && input.organization_configuration.is_none() {
        return Some(crate::state::constraint_report(
            "organizationConfiguration",
            "Member is required for the OrganizationRule rule type",
        ));
    }
```
and then stores the member unconditionally at create_automation_rule.rs:350: `organization_configuration: input.organization_configuration,`.

The crate's own statement that the member is inert on an AccountRule — src/ops/update_automation_rule.rs:292-295:
"// ... Two members decide that, so
// either of them makes this request one about where the rule applies:
// the organization configuration, whose `accountIds` ARE that scope, and
// the `OrganizationRule` type, which is what puts a configuration into effect"
while its merged-rule check (update_automation_rule.rs:373-378) again only covers `merged_rule_type == "OrganizationRule" && merged_org_config.is_none()`.

The filter that then selects on the inert member — state.rs:3623-3626:
```rust
            "OrganizationConfigurationRuleApplyOrder" => rule
                .organization_configuration
                .as_ref()
                .and_then(|c| c.rule_apply_order.clone()),
```

Model text for the member — `smithy_query(query="doc",shape="CreateAutomationRuleRequest",member="organizationConfiguration")`: "Configuration for organization-level rules. Required for OrganizationRule type."

Test coverage: tests/all/automation_rule_lifecycle.rs:364-431 round-trips an `AccountRule` and an `OrganizationRule`, and tests/all/automation_rule_validation.rs / organization_rule_scope_gate.rs cover the `OrganizationRule`-requires-a-configuration direction; no test pins the accepted `AccountRule` + `organizationConfiguration` combination (grep for `organizationConfiguration` over tests/ shows no such case), so nothing in the crate justifies the asymmetry.

## Steps to Reproduce

1. Opt the caller in: POST / with X-Amz-Target: ComputeOptimizerAutomationService.UpdateEnrollmentConfiguration and {"status":"Active"}.
2. Call ListAutomationRulePreview with {"ruleType":"AccountRule","recommendedActionTypes":["UpgradeEbsVolumeType"],"organizationScope":{"accountIds":["000000000001"]}} — refused 400 InvalidParameterValueException, "Member is valid only for the OrganizationRule rule type" (state.rs:1621).
3. Call CreateAutomationRule with the same mismatch: {"name":"acct-rule","ruleType":"AccountRule","organizationConfiguration":{"ruleApplyOrder":"BeforeAccountRules"},"recommendedActionTypes":["UpgradeEbsVolumeType"],"schedule":{"scheduleExpression":"30 12 * * ? *"},"status":"Active"} — answered 200 and the rule is stored with the configuration.
4. Call GetAutomationRule with the returned ruleArn — the response reports `ruleType: AccountRule` together with an `organizationConfiguration` that, per update_automation_rule.rs:292-295, puts nothing into effect.
5. Call ListAutomationRules with filters [{"name":"OrganizationConfigurationRuleApplyOrder","values":["BeforeAccountRules"]}] — the AccountRule is selected on that inert member.
