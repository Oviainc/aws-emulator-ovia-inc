Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# CreateAutomationRule stores an OrganizationRule whose organizationConfiguration.accountIds names accounts the organization is not associated with

_Filed: 2026-10-27T23:05:22Z_
_Operation: CreateAutomationRule | Class: validation_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAutomationRule` validates the PRESENCE of `organizationConfiguration` for an `OrganizationRule`, but it never looks at the accounts that configuration names. Any well-formed account id is accepted and stored, including an account that has never been associated and an account whose association was explicitly removed by `DisassociateAccounts`.

The model states the rule the handler skips: an association is exactly what "permits the management account to create Automation rules that automatically apply actions to that account", and once an account is disassociated "the management account (or delegated administrator) cannot create Automation rules for that account". The crate already owns the predicate for that fact — `ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState::is_associated`, whose own doc comment quotes that very sentence — and two sibling handlers (`StartAutomationEvent`, `RollbackAutomationEvent`) consult it through `may_initiate_in` before acting in another account. The create path calls neither, so the one operation whose documentation the association sentence is actually about (rule creation) is the one operation that does not check it.

Effect: a caller can create (and later read back through `GetAutomationRule`/`ListAutomationRules`) an organization rule scoped to member accounts the organization has no reach into, i.e. exactly the state `DisassociateAccounts` documents as impossible.

## Evidence

Smithy, `smithy_query(query="doc",shape="DisassociateAccounts")`:
> "Disassociates member accounts from your organization's management account, removing centralized automation capabilities. Once disassociated, organization rules no longer apply to the member account, and **the management account (or delegated administrator) cannot create Automation rules for that account**."

Smithy, `smithy_query(query="doc",shape="AssociateAccounts")`:
> "When you associate a member account, its organization rule mode is automatically set to \"Any allowed,\" which **permits the management account to create Automation rules that automatically apply actions to that account**."

The only clause the handler applies to `organizationConfiguration` — `src/ops/create_automation_rule.rs:426-431`:
```
    if input.rule_type == "OrganizationRule" && input.organization_configuration.is_none() {
        return Some(crate::state::constraint_report(
            "organizationConfiguration",
            "Member is required for the OrganizationRule rule type",
        ));
    }
```
and the configuration is then stored as it arrived — `src/ops/create_automation_rule.rs:339`:
```
            organization_configuration: input.organization_configuration,
```
The predicate the crate already holds, stating the rule — `src/state.rs:3070-3083`:
```
    /// Is `account_id` ASSOCIATED with the organization's management account,
    /// i.e. does its stored enrollment report
    /// [`ORGANIZATION_RULE_MODE_ANY_ALLOWED`]?
    ///
    /// Association is what that rule mode records: `AssociateAccounts` sets it
    /// to "Any allowed", which is what "permits the management account to create
    /// Automation rules that automatically apply actions to that account". An
    /// account that has never been associated reports the other mode
    /// ([`EnrollmentConfig::reported_organization_rule_mode`]), so it is not
    /// associated.
    pub fn is_associated(&self, account_id: &str) -> bool {
```
Siblings that DO consult it before acting in another account — `src/ops/start_automation_event.rs:266-275`:
```
        // The caller's STANDING in the account the automation would run in: an
        // automation is initiated only for a recommended action of an
        // associated member account. ...
        if !guard.may_initiate_in(&account_id) {
            return Err(access_denied());
        }
```
and `src/ops/rollback_automation_event.rs:266-268` (same call). `grep -r "is_associated\|may_initiate_in" src/ops` returns only `associate_accounts.rs:296`, `disassociate_accounts.rs:272`, `start_automation_event.rs:273` and `rollback_automation_event.rs:266` — no rule handler.

Test coverage today: `tests/all/automation_rule_lifecycle.rs:392-405` creates exactly this shape and expects 200, while its stated purpose is member round-tripping ("Every created rule is readable back field-for-field through `GetAutomationRule`", line 349-353), not association:
```
        (
            "organization-rule",
            json!({
                "name": "org-wide",
                "ruleType": "OrganizationRule",
                "organizationConfiguration": {
                    "ruleApplyOrder": "BeforeAccountRules",
                    "accountIds": ["000000000000", "000000000001"]
                },
```
No test states a reason for accepting an unassociated target account, and no comment in the crate admits the divergence; fixing this requires associating those ids in that fixture (as the `member_account_association_gate` suite does for the start/rollback gate).

## Steps to Reproduce

1. Opt the calling account in: `UpdateEnrollmentConfiguration {"status": "Active"}`.
2. Without associating anything, call `CreateAutomationRule {"name":"org-wide","ruleType":"OrganizationRule","organizationConfiguration":{"ruleApplyOrder":"BeforeAccountRules","accountIds":["444455556666"]},"recommendedActionTypes":["UpgradeEbsVolumeType"],"schedule":{"scheduleExpression":"15 1 * * ? *"},"status":"Active"}`: the rule is minted (200) although `444455556666` reports `organizationRuleMode: NoneAllowed` through `ListAccounts`/`GetEnrollmentConfiguration`.
3. Now `AssociateAccounts {"accountIds":["444455556666"]}` followed by `DisassociateAccounts {"accountIds":["444455556666"]}`, then repeat the create: it is still accepted, although `DisassociateAccounts` documents that the management account "cannot create Automation rules for that account".
4. Contrast with `StartAutomationEvent` for a recommended action of the same disassociated account, which is refused 403 `AccessDeniedException` via `may_initiate_in` (`src/ops/start_automation_event.rs:273`).
