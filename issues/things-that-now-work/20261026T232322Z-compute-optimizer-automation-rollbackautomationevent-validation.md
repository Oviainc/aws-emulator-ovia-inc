Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# RollbackAutomationEvent accepts an event belonging to a DISASSOCIATED member account

_Filed: 2026-10-26T23:23:22Z_
_Operation: RollbackAutomationEvent | Class: validation_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy documentation for `RollbackAutomationEvent` states that a rollback may be initiated for a member account's event ONLY while that member account is associated. The handler resolves `eventId` against the event store and judges eligibility purely on `eventStatus` (`rollback_can_start()`), never on whether the event's `accountId` is still associated — even though the crate already has `is_associated` and uses it in `AssociateAccounts`/`DisassociateAccounts`.

Since an automation event stores the member `accountId` it ran in and `DisassociateAccounts` removes nothing but the `organizationRuleMode`, an event started while a member was associated stays fully rollback-able after the association is removed. That contradicts the documented "only ... for events belonging to associated member accounts".

## Evidence

Smithy model (`smithy_query(query="doc",shape="RollbackAutomationEvent")`):
> "Initiates a rollback for a completed automation event." ... "Management accounts and delegated administrators can **only** initiate a rollback for events belonging to associated member accounts. You can associate a member account using `AssociateAccounts`."

Handler — the only two gates are existence and status:
`src/ops/rollback_automation_event.rs:246-262`
```
        let Some(event) = guard.events.get_mut(&input.event_id) else {
            return Err(not_found());
        };
        ...
        event.settle(now);
        if !event.rollback_can_start() {
            return Err(
                RollbackAutomationEventErrorModeled::InvalidParameterValueException(
```
and `src/state.rs:1880-1882`:
```
    pub fn rollback_can_start(&self) -> bool {
        self.event_status == EVENT_STATUS_COMPLETE
    }
```
The event carries the member account it ran in — `src/state.rs:1806`: `pub account_id: String,` — so the information needed for the documented check is present and unused.

The helper the crate already has: `src/state.rs:3015` `pub fn is_associated(&self, account_id: &str) -> bool`, called by `src/ops/associate_accounts.rs:301` and `src/ops/disassociate_accounts.rs:277`.

Why the event survives disassociation — `src/state.rs:3081-3086`: "The account's OWN enrollment is left alone ... (and keeps the inventory that enrollment produced) once the organization stops reaching it."

Test coverage: `tests/all/automation_event_rollback.rs` exercises the status gate and idempotency only; `grep Disassociate tests/all/automation_event*.rs` -> no matches, so no test pins today's behaviour for a disassociated account.

## Steps to Reproduce

1. Call `UpdateEnrollmentConfiguration` with `{"status":"Active"}`.
2. Call `AssociateAccounts` with `{"accountIds":["111111111111"]}`.
3. Call `ListRecommendedActions`, pick a `recommendedActionId` whose `accountId` is `111111111111`, and call `StartAutomationEvent` with it; keep the returned `eventId`.
4. Wait for the event to settle and confirm `GetAutomationEvent` reports `eventStatus: Complete` and `accountId: 111111111111`.
5. Call `DisassociateAccounts` with `{"accountIds":["111111111111"]}`.
6. Call `RollbackAutomationEvent` with that `eventId`.
7. Observe HTTP 200 and `eventStatus: RollbackInProgress` — a rollback initiated for an event belonging to an account that is no longer associated, which the operation's documentation says can only be done for associated member accounts.
