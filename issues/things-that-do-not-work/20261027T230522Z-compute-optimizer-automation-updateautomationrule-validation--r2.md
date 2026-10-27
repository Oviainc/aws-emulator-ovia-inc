Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# UpdateAutomationRule re-validates the merged rule but never checks that the organizationConfiguration accounts are associated

_Filed: 2026-10-27T23:05:22Z_
_Operation: UpdateAutomationRule | Class: validation_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateAutomationRule` explicitly re-validates the MERGED rule against the rules a create holds it to — it recomputes `merged_rule_type`/`merged_org_config` and refuses an `OrganizationRule` left without an organization configuration. That same merged check is the place the association rule belongs, and it is not applied: an update may set `ruleType` to `OrganizationRule` and/or replace `organizationConfiguration.accountIds` with accounts the organization has never associated (or has explicitly disassociated), and the handler stores them.

The model states the rule: once disassociated, "the management account (or delegated administrator) cannot create Automation rules for that account"; an association is what "permits the management account to create Automation rules that automatically apply actions to that account". The crate owns the predicate (`is_associated`/`may_initiate_in`) and two sibling handlers call it, so an update that re-points a rule at an unassociated member account reaches a state the model says cannot exist — and it does so through the one code path in the handler whose stated job is "The merged rule must still satisfy the rules a create does".

## Evidence

Smithy, `smithy_query(query="doc",shape="DisassociateAccounts")`:
> "Once disassociated, organization rules no longer apply to the member account, and the management account (or delegated administrator) cannot create Automation rules for that account."

The merged-validity check that stops at presence — `src/ops/update_automation_rule.rs:332-345`:
```
        // The merged rule must still satisfy the rules a create does: an
        // `OrganizationRule` needs an organization configuration, whether it
        // arrives in this request or is already stored.
        let merged_rule_type = input.rule_type.clone().unwrap_or(rule.rule_type.clone());
        let merged_org_config = input
            .organization_configuration
            .clone()
            .or_else(|| rule.organization_configuration.clone());
        if merged_rule_type == "OrganizationRule" && merged_org_config.is_none() {
            return Err(invalid_parameter(crate::state::constraint_report(
                "organizationConfiguration",
                "Member is required for the OrganizationRule rule type",
            )));
        }
```
and the supplied configuration is then written verbatim — `src/ops/update_automation_rule.rs:362-364`:
```
        if let Some(organization_configuration) = input.organization_configuration.clone() {
            rule.organization_configuration = Some(organization_configuration);
        }
```
The predicate the crate already owns and quotes the documented rule in — `src/state.rs:3074-3080`:
```
    /// Association is what that rule mode records: `AssociateAccounts` sets it
    /// to "Any allowed", which is what "permits the management account to create
    /// Automation rules that automatically apply actions to that account". ...
    pub fn is_associated(&self, account_id: &str) -> bool {
```
Sibling that applies it before acting in another account — `src/ops/rollback_automation_event.rs:266-268`:
```
        if !guard.may_initiate_in(&account_id) {
            return Err(access_denied());
        }
```
Test coverage: `tests/all/automation_rule_lifecycle.rs:628` (`update_applies_only_what_it_carries_and_is_revision_checked`) exercises the merge and the revision check; no test sends an `organizationConfiguration` naming an unassociated account, so nothing pins the current acceptance either way.

## Steps to Reproduce

1. Opt in with `UpdateEnrollmentConfiguration {"status": "Active"}` and `AssociateAccounts {"accountIds":["444455556666"]}`.
2. Create an `OrganizationRule` whose `organizationConfiguration.accountIds` is `["444455556666"]` (accepted).
3. `DisassociateAccounts {"accountIds":["444455556666"]}`.
4. Call `UpdateAutomationRule` with the rule's `ruleArn`, its current `ruleRevision` and `organizationConfiguration: {"ruleApplyOrder":"BeforeAccountRules","accountIds":["444455556666","777788889999"]}`: the update is applied and `GetAutomationRule` reports an organization rule targeting two accounts the organization is not associated with.
