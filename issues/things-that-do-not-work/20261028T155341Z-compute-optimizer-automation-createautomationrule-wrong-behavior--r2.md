Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e113b6e45c2c86d79f2618efd25a6c62e8ae6901

# An AccountRule create is refused 403 over organizationConfiguration.accountIds, a scope the crate says only the OrganizationRule type puts into effect

_Filed: 2026-10-28T15:53:41Z_
_Operation: CreateAutomationRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e113b6e45c2c86d79f2618efd25a6c62e8ae6901`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateAutomationRule runs the association gate over `input.organization_configuration` unconditionally, without regard to the rule type the create stores. So a create of an `AccountRule` that carries an `organizationConfiguration` whose `accountIds` name an account that is not currently associated is refused with `AccessDeniedException` (403) — even though this crate's own rule is that the configuration's account scope is only in force for the `OrganizationRule` type.

The crate contradicts itself three ways:

1. `UpdateAutomationRule`'s comment states that the `OrganizationRule` type "is what puts a configuration into effect", and `may_scope_rule_to`'s doc justifies the gate by saying the judged account ids "are the accounts that call makes the rule apply its actions in". For an `AccountRule` neither premise holds: the rule applies its actions in the caller's own account, so the gated `accountIds` scope nothing.

2. The end state the create is refused for is reachable without any refusal: the crate's own suite pins that turning a stored `OrganizationRule` into an `AccountRule` after its scoped member was disassociated is SERVED ("Turning an organization rule into an account rule stops it applying anywhere else, so it is not held to the stale scope"). After that update the store holds exactly what step 2 of the reproduction was refused for — an `AccountRule` whose stored `organizationConfiguration` names an unassociated account — so the gate does not protect any invariant; it only refuses one of the two paths to the same state.

3. The sibling preview handlers, which take the analogous organization-only member (`organizationScope`), do not gate it for a non-organization rule type at all: they REFUSE the member as inapplicable with 400 ("Member is valid only for the OrganizationRule rule type"), on the stated ground that such a member "names nothing for a rule type scoped to one account". `CreateAutomationRule` instead enforces the same kind of member as if it were in force, answering 403 for a rule type the crate says it does not scope.

`UpdateAutomationRule` carries the same unconditional treatment whenever the request writes a configuration (`update_automation_rule.rs:302`), so the root cause is shared; the create is named here because it is the path that first stores the rule.

## Evidence

The gate, applied to the configuration whatever the rule type is — `src/ops/create_automation_rule.rs:254-263`:
```
        // The caller's STANDING in each account the rule would apply actions in:
        // an organization rule automatically applies its actions to the accounts
        // its configuration names, which is what an association permits, so an
        // account that is not associated right now may not be scoped — see
        // `crate::state::…::may_scope_rule_to`. Judged AHEAD of the token ledger
        // and of the store, so a refused create neither consumes its client
        // token nor mints a rule.
        if !guard.may_scope_rule_to(input.organization_configuration.as_ref()) {
            return Err(access_denied());
        }
```
and the create's ONLY rule-type clause about this member — `src/ops/create_automation_rule.rs:448-453`:
```
    if input.rule_type == "OrganizationRule" && input.organization_configuration.is_none() {
        return Some(crate::state::constraint_report(
            "organizationConfiguration",
            "Member is required for the OrganizationRule rule type",
        ));
    }
```

The crate's own statement that the TYPE is what puts the configuration into effect — `src/ops/update_automation_rule.rs:290-294`:
```
        // `crate::state::…::may_scope_rule_to`. Two members decide that, so
        // either of them makes this request one about where the rule applies:
        // the organization configuration, whose `accountIds` ARE that scope, and
        // the `OrganizationRule` type, which is what puts a configuration into
        // effect
```
and the gate helper's premise — `src/state.rs:3125-3128`:
```
    /// Judged over the account ids of the configuration the RESULTING rule would
    /// carry — the one a create supplies, or the one an update merges over the
    /// stored rule — which are the accounts that call makes the rule apply its
    /// actions in;
```

The same end state is served on the other path — `tests/all/organization_rule_scope_gate.rs:681-691`:
```
        RetypeRow {
            // Turning an organization rule into an account rule stops it
            // applying anywhere else, so it is not held to the stale scope.
            case: "retype-to-an-account-rule-after-a-disassociation",
            stored_type: "OrganizationRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&de_escalated)),
            associate: de_escalated.clone(),
            then: vec![(false, de_escalated)],
            update: json!({"ruleType": "AccountRule"}),
            answer: Answer::Served,
        },
```

How the sibling preview path treats an organization-only member on a non-organization rule type — `src/state.rs:1467-1470` and `src/state.rs:1481-1486`:
```
/// * an `organizationScope` is the set of accounts of an ORGANIZATION-wide
///   preview, so it names nothing for a rule type scoped to one account, and
///   silently ignoring a member the caller sent would be worse than reporting
///   it;
```
```
    if rule_type != "OrganizationRule" && organization_scope.is_some() {
        return Some(constraint_report(
            "organizationScope",
            "Member is valid only for the OrganizationRule rule type",
        ));
    }
```

Test coverage: no test creates an `AccountRule` whose `organizationConfiguration` names an unassociated account — `tests/all/organization_rule_scope_gate.rs:538-540` states every row of the retype table associates the named accounts before the create ("Associated before the create, so the create itself is always served"), and the first table only builds `OrganizationRule` creates (`organization_rule`, :69-74). `tests/all/automation_rule_lifecycle.rs:358-372` likewise associates `SCOPED_MEMBER` before its organization-rule round trip. So the behaviour reported here is pinned by nothing and no existing assertion changes if the gate is made consistent with the rule type.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate.
2. Call CreateAutomationRule with {"name": "acct", "ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "schedule": {"scheduleExpression": "0 3 * * ? *"}, "status": "Active", "organizationConfiguration": {"ruleApplyOrder": "BeforeAccountRules", "accountIds": ["000000009999"]}} — an account no AssociateAccounts call has ever named. Observe 403 AccessDeniedException from src/ops/create_automation_rule.rs:261-263, although an AccountRule applies its actions only in the caller's own account.
3. Reach the same stored state on the other path: AssociateAccounts ["000000009999"]; CreateAutomationRule the same rule with "ruleType": "OrganizationRule" (served); DisassociateAccounts ["000000009999"]; UpdateAutomationRule {"ruleArn": …, "ruleRevision": 1, "ruleType": "AccountRule"} — served, as tests/all/organization_rule_scope_gate.rs:681-691 pins. GetAutomationRule now reports an AccountRule whose organizationConfiguration names an unassociated account: exactly the state step 2 was refused for.
4. Compare the sibling treatment of the analogous organization-only member: ListAutomationRulePreview with {"ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "organizationScope": {"accountIds": ["000000009999"]}} answers 400 InvalidParameterValueException, "Member is valid only for the OrganizationRule rule type" (src/state.rs:1481-1486) — the crate refuses an organization-only member on a non-organization rule type there instead of enforcing it as a live scope.
