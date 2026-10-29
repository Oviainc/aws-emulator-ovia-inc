Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 709cb32244d1fde05c39e9dcd2199db49cf26e6b

# Re-designating the same delegated administrator while its own removal is still in flight is refused as "already has a delegated administrator", a reversal every other transitional record in the crate turns around

_Filed: 2026-10-29T10:48:31Z_
_Operation: EnableOrganizationAdminAccount | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `709cb32244d1fde05c39e9dcd2199db49cf26e6b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`OrganizationAdminAccountRecord` is a transitional record exactly like the IPSet / threat-intel-set / entity-set records: a removal is ACCEPTED now, the record reports the transient `DISABLE_IN_PROGRESS` status, and the CLOCK owes the actual removal `ADMIN_DISABLE_SPAN_MILLIS` later (state.rs:8187-8197, 8226-8233). `ListOrganizationAdminAccounts` reports that record with `adminStatus: DISABLE_IN_PROGRESS` (list_organization_admin_accounts.rs:184-188 through `crate::state::organization_admin_account_view`, state.rs:8171-8177).

Every OTHER transitional record of this crate states, and implements, the rule that a request which reverses a transition still running TURNS IT AROUND rather than being refused or queued behind it (`EntitySetRecord::set_activation` state.rs:4185-4190, `ThreatIntelSetRecord::set_activation` state.rs:4386-4392, `IpSetRecord::set_activation` state.rs:3522-3539). `OrganizationAdminAccountRecord` implements only the other half of that rule — `begin_disable` is idempotent (state.rs:8190-8197) — and carries no way at all to reverse a pending removal.

The consequence is a refusal that contradicts the record's own reported status and its own message. `EnableOrganizationAdminAccount` gates its no-op branch on `current.disable_due_millis.is_none()` (enable_organization_admin_account.rs:166-170), so a designation naming the SAME account whose removal is in flight falls through to the "anything else would be a SECOND administrator" branch at :171-175 and is refused HTTP 400 with `ORGANIZATION_ADMIN_ALREADY_DESIGNATED_MESSAGE` — a message that tells the caller to "Remove the current delegated administrator before designating another account" when (a) the removal has already been accepted and is the very reason the call was refused, and (b) the request names that same account, not another one. No second administrator could have resulted: the request names the account the record already holds.

So during the removal span the account is stuck: the same account cannot be re-designated, and the caller is told to do the thing it has already done.

## Evidence

THE RULE THE CRATE STATES AND IMPLEMENTS FOR EVERY OTHER TRANSITIONAL RECORD — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:4185-4190 (`EntitySetRecord::set_activation`):
```
    /// Asking for the status the set has already settled on, or is already
    /// transitioning to, owes nothing and answers `false`, so a repeated request
    /// leaves the status and the instant the clock owes it exactly where they are
    /// rather than re-opening a transition. Asking for the other one replaces
    /// whatever was owed, so a request that reverses a transition still running
    /// turns it around instead of queueing behind it.
```
and state.rs:4386-4392 (`ThreatIntelSetRecord::set_activation`):
```
    /// Asking for the status the list has already settled on, or is already
    /// transitioning to, owes nothing, so a repeated request leaves the status and
    /// the instant the clock owes it exactly where they are rather than re-opening
    /// a transition. Asking for the other one replaces whatever was owed, so a
    /// request that reverses a transition still running turns it around instead of
    /// queueing behind it.
```

THE ADMIN RECORD IS THE SAME SHAPE OF TRANSITION — state.rs:8161-8164 and 8187-8197:
```
    /// When a removal has been accepted, the instant the CLOCK owes it — until
    /// then the record reports the transient status, and from then on it is gone.
    /// `None` while no removal is pending.
    pub disable_due_millis: Option<i64>,
```
```
    /// Accept a removal of this delegated administrator, due
    /// [`ADMIN_DISABLE_SPAN_MILLIS`] after `now`.
    ///
    /// Idempotent: a second removal of the same administrator leaves the instant
    /// the first one made due where it was, so repeating the call cannot postpone
    /// the removal for ever.
    pub fn begin_disable(&mut self, now: i64) {
        if self.disable_due_millis.is_none() {
            self.disable_due_millis = Some(now.saturating_add(ADMIN_DISABLE_SPAN_MILLIS));
        }
    }
```
— the idempotent half of the rule is there; the reversal half is nowhere on the type.

WHAT THE RECORD REPORTS WHILE THE REMOVAL IS PENDING — state.rs:8171-8177:
```
    pub fn status(&self) -> &'static str {
        if self.disable_due_millis.is_some() {
            ADMIN_DISABLE_IN_PROGRESS
        } else {
            ENABLED
        }
    }
```
reported to the caller by src/ops/list_organization_admin_accounts.rs:184-188:
```
        let page: Vec<AdminAccount> = administrators[window]
            .iter()
            .copied()
            .map(crate::state::organization_admin_account_view)
            .collect();
```

THE VIOLATION — src/ops/enable_organization_admin_account.rs:162-176:
```
        if let Some(current) = state.organization_admin_account_in(&region) {
            // Designating the account the Region already holds is the same
            // designation: it answers success and changes nothing, so a retried
            // call is not a second designation.
            if current.admin_account_id == input.admin_account_id
                && current.disable_due_millis.is_none()
            {
                return Ok(EnableOrganizationAdminAccountOutput {});
            }
            // Anything else would be a SECOND administrator for one organization,
            // which a read reporting at most one cannot describe.
            return Err(crate::scaffolded::Scaffolded::invalid_input(
                crate::state::ORGANIZATION_ADMIN_ALREADY_DESIGNATED_MESSAGE,
            ));
        }
```
The comment's justification ("Anything else would be a SECOND administrator") does not hold on this path: `current.admin_account_id == input.admin_account_id`, so there is only ever one account involved.

THE MESSAGE IT ANSWERS WITH — state.rs:8257:
```
pub const ORGANIZATION_ADMIN_ALREADY_DESIGNATED_MESSAGE: &str = "The request is rejected because the organization already has a GuardDuty delegated administrator. Remove the current delegated administrator before designating another account.";
```

TEST COVERAGE: tests/all/organization_admin_removal.rs pins only the SETTLED case — `a_settled_removal_frees_the_region_for_any_well_formed_account_id` (:138-201) sleeps `past_the_removal()` at :188 before re-designating, and its first row is literally "the same account designated again" (:146-151). Nothing in that suite, nor in tests/all/organization_admin_accounts.rs, sends an `EnableOrganizationAdminAccount` for the same account WHILE `DISABLE_IN_PROGRESS` is still reported (asserted at :185 and :134), so no test pins this refusal and none states a reason for it.

## Steps to Reproduce

1. Designate a delegated administrator through us-east-1: `POST /admin/enable` with `{"adminAccountId": "100000000001"}` — 200.
2. `GET /admin` reports `adminAccounts: [{"adminAccountId": "100000000001", "adminStatus": "ENABLED"}]`.
3. Ask for its removal: `POST /admin/disable` with `{"adminAccountId": "100000000001"}` — 200 (disable_organization_admin_account → `begin_disable`, state.rs:1070-1074).
4. `GET /admin` now reports `"adminStatus": "DISABLE_IN_PROGRESS"` (state.rs:8171-8177), i.e. the removal is accepted and the clock owes it `ADMIN_DISABLE_SPAN_MILLIS` later.
5. Within that span, re-designate THE SAME account: `POST /admin/enable` with `{"adminAccountId": "100000000001"}`.
6. Actual: HTTP 400 `InvalidInputException` "The request is rejected because the organization already has a GuardDuty delegated administrator. Remove the current delegated administrator before designating another account." (enable_organization_admin_account.rs:173-175) — a refusal whose message instructs the caller to remove the administrator it has just removed, about a request that names no other account.
7. Contrast with the crate's own transitional sub-resources: `POST /detector/{id}/trustedentityset/{setId}` with `{"activate": true}` issued while the set is still `DEACTIVATING` turns that transition around instead of refusing it (state.rs:4185-4190, 4196-4207) — and `POST /admin/disable` repeated during the span is itself accepted as a no-op (state.rs:8190-8197), so the reversal is the only direction the admin record refuses.
