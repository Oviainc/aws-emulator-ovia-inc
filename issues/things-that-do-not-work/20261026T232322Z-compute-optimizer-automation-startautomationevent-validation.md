Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# StartAutomationEvent accepts a recommended action of a DISASSOCIATED member account

_Filed: 2026-10-26T23:23:22Z_
_Operation: StartAutomationEvent | Class: validation_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy documentation for `StartAutomationEvent` states that the caller may start an automation for a member account's recommended action ONLY while that member account is associated. The handler resolves `recommendedActionId` against the whole `recommended_actions` store and never asks whether the action's `accountId` is still associated, even though the crate already has `is_associated` and uses it in two sibling handlers.

The gap is reachable because `DisassociateAccounts` deliberately leaves the member account's recommended-action inventory in place (its own doc comment says so), while flipping the account's `organizationRuleMode` to `NoneAllowed`. So after `AssociateAccounts` -> `DisassociateAccounts`, the member's analysed actions are still listed by `ListRecommendedActions` and `StartAutomationEvent` still mints an `InProgress` event in that account — the exact thing the model says the management account may no longer do.

The crate's own reasoning for this operation assumes association and inventory presence are the same fact (the test comment "An account that was never associated has no inventory, so an id of its own is not found"), which disassociation breaks.

## Evidence

Smithy model (`smithy_query(query="doc",shape="StartAutomationEvent")`):
> "Initiates a one-time, on-demand automation for the specified recommended action." ... "Management accounts and delegated administrators can **only** initiate recommended actions for associated member accounts. You can associate a member account using `AssociateAccounts`."

Handler — no association check at all; the only gate is existence in the store:
`src/ops/start_automation_event.rs:251-257`
```
        let Some(action) = guard
            .recommended_actions
            .get(&input.recommended_action_id)
            .cloned()
        else {
            return Err(not_found());
        };
```

The helper the crate already has, and the siblings that call it:
`src/state.rs:3015`
```
    pub fn is_associated(&self, account_id: &str) -> bool {
        self.enrollment(account_id).reported_organization_rule_mode()
            == ORGANIZATION_RULE_MODE_ANY_ALLOWED
```
`src/ops/associate_accounts.rs:301`: `.filter(|account_id| guard.is_associated(account_id))`
`src/ops/disassociate_accounts.rs:277`: `.filter(|account_id| !guard.is_associated(account_id))`

Why the state survives disassociation — `src/state.rs:3081-3086`:
```
    /// The account's OWN enrollment is left alone: ... so a member account that enabled
    /// the Automation feature keeps it enabled (and keeps the inventory that
    /// enrollment produced) once the organization stops reaching it.
```

Test coverage: `tests/all/recommended_action_inventory.rs:777-813` pins the two cases the crate does handle — a start for an ASSOCIATED member succeeds (`"member-start"`), and an id of an account "that was never associated" is 404 — but there is no case for an account that was associated and then disassociated (`grep -i disassoci tests/all/recommended_action_inventory.rs` -> no matches; `grep Disassociate tests/all/automation_event*.rs` -> no matches), so nothing pins today's behaviour.

## Steps to Reproduce

1. Call `UpdateEnrollmentConfiguration` with `{"status":"Active"}` so the caller is opted in.
2. Call `AssociateAccounts` with `{"accountIds":["111111111111"]}` — the member account is now stored with `organizationRuleMode: AnyAllowed` and its analysed inventory is recorded.
3. Call `ListRecommendedActions` and note a `recommendedActionId` whose `accountId` is `111111111111`.
4. Call `DisassociateAccounts` with `{"accountIds":["111111111111"]}`; confirm via `ListAccounts` that the account now reports `organizationRuleMode: NoneAllowed`.
5. Call `StartAutomationEvent` with that `recommendedActionId`.
6. Observe HTTP 200 with a new `eventId` in `InProgress`, and `GetAutomationEvent` reporting `accountId: 111111111111` — an automation initiated for an account that is no longer associated, which the operation's documentation says can only be done for associated member accounts.
