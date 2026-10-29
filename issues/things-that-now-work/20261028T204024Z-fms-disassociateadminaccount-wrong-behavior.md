Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6d79682d5fafb93b7e79af5f6250590ad910f660

# Once PutAdminAccount onboards any administrator, DisassociateAdminAccount is refused forever: the crate has no path that removes a delegated administrator or gives one the OFFBOARDING_COMPLETE status its own precondition tests for

_Filed: 2026-10-28T20:40:24Z_
_Operation: DisassociateAdminAccount | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6d79682d5fafb93b7e79af5f6250590ad910f660`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DisassociateAdminAccount` refuses the default administrator's tear-down while `other_administrator_accounts(now)` is non-empty, and its own module doc tells the caller the remedy: "The refusal names the administrators that still have to leave, so the caller knows which accounts must call this operation first." That remedy is unreachable in this crate, so the refusal is PERMANENT.

Two independent reasons, both inside this crate:

1. `other_administrator_accounts` drops a delegated administrator only when its summary `Status` is `OFFBOARDING_COMPLETE`. That status is never written to an `OrganizationAdminRecord`: `PutAdminAccount` is the only writer of `organization_admins`, and it inserts `status: ORGANIZATION_STATUS_ONBOARDING` with `settles_to: ORGANIZATION_STATUS_ONBOARDING_COMPLETE`, so `OrganizationAdminRecord::status_at` can only ever answer `ONBOARDING` or `ONBOARDING_COMPLETE`. A repository grep for `organization_admins.remove` / `organization_admins.retain` across `src/` returns nothing, so an onboarded administrator is also never dropped from the map. The filter branch at state.rs:1468 is therefore dead code, and `OFFBOARDING` / `OFFBOARDING_COMPLETE` are two of the four modelled `OrganizationStatus` values that no code path in this crate can produce for a delegated administrator.

2. Even if a delegated administrator wanted to leave, it cannot call the operation: the handler refuses any caller that is not the account holding the default association (`if admin.account_id != caller`), and `caller` is always `::engine::emulator_account_id()` because, as the same file states, "Every request this emulator serves is signed by the single emulator account". So the accounts the refusal names can never be the caller of a successful disassociation.

Consequence: after a single successful `PutAdminAccount`, the organisation's administrator lifecycle has no exit. `DisassociateAdminAccount` answers HTTP 400 `InvalidOperationException` for every subsequent call, the default administrator can never reach `PENDING_DELETION`/`DELETED`, and `GetAdminScope` can never report `OFFBOARDING` or `OFFBOARDING_COMPLETE` for an onboarded administrator.

## Evidence

The documented remedy, services/service-com-amazonaws-fms-awsfms-20180101/src/ops/disassociate_admin_account.rs:40-44:
```
//! the tear-down the caller asks for is not possible, which is exactly what the
//! modelled `InvalidOperationException` reports ("the operation failed because
//! there was nothing to do or the operation wasn't possible"). The refusal names
//! the administrators that still have to leave, so the caller knows which
//! accounts must call this operation first.
```

...contradicted by the caller check in the same file, disassociate_admin_account.rs:239-241 and :266-271:
```
        // The account every request this emulator serves is signed by — so, the
        // account asking to disassociate.
        let caller = ::engine::emulator_account_id();
...
        if admin.account_id != caller {
            return Err(crate::scaffolded::Scaffolded::access_denied(format!(
                "account {caller} is not the Firewall Manager default administrator, \
                 so it cannot disassociate the default administrator account"
            )));
        }
```

The precondition and the only status that can clear it — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:1463-1472:
```
    pub fn other_administrator_accounts(&self, now: SystemTime) -> Vec<String> {
        self.admin_account_summaries(now)
            .into_values()
            .filter(|summary| {
                !summary.default_admin
                    && summary.status.as_deref() != Some(ORGANIZATION_STATUS_OFFBOARDING_COMPLETE)
            })
            .filter_map(|summary| summary.admin_account)
            .collect()
    }
```
with its doc, state.rs:1456-1459: "an account whose status is [`ORGANIZATION_STATUS_OFFBOARDING_COMPLETE`] \"has been removed as an Firewall Manager administrator\" (`AdminAccountSummary$Status`), so it is no longer one of them."

But the ONLY writer of `organization_admins` can only produce the two onboarding statuses — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_admin_account.rs:298-311:
```
                store.organization_admins.insert(
                    input.admin_account.clone(),
                    crate::state::OrganizationAdminRecord {
                        account_id: input.admin_account.clone(),
                        admin_scope: input.admin_scope.clone().unwrap_or_default(),
                        status: crate::state::ORGANIZATION_STATUS_ONBOARDING.to_string(),
                        settles_to: Some(
                            crate::state::ORGANIZATION_STATUS_ONBOARDING_COMPLETE.to_string(),
                        ),
                        settles_at: Some(now + crate::state::ADMIN_ONBOARDING_SPAN),
```
and the summary's status is read straight off that record — state.rs:1416: `status: Some(admin.status_at(now).to_string()),`, where `status_at` (state.rs:198-203) returns only `settles_to` or `status`.

Grep evidence that no record is ever removed or offboarded: `grep -n 'organization_admins\.(remove|retain)|settles_to = Some' services/service-com-amazonaws-fms-awsfms-20180101/src` matches only state.rs:659 (`self.settles_to = Some(THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE...)`, the third-party-firewall record, not the administrator record).

Model text the dead status comes from — `smithy_query(query="doc", shape="AdminAccountSummary", member="Status")`: "`OFFBOARDING` - The account is being removed as an Firewall Manager administrator. ... `OFFBOARDING_COMPLETE` - The account has been removed as an Firewall Manager administrator."

Test coverage: services/service-com-amazonaws-fms-awsfms-20180101/tests/all/admin_disassociation_first_in_last_out.rs:6-11 states the rule as "...the tear-down is accepted only once the organization holds no other administrator", but the only row of `cases()` (lines 85-89) that asserts acceptance is `name: "no other administrator", others: Vec::new()` — i.e. an organisation where `PutAdminAccount` was never called. No test exercises an administrator LEAVING, because no operation can make one leave.

## Steps to Reproduce

1. POST / with X-Amz-Target AWSFMS_20180101.AssociateAdminAccount and body {"AdminAccount": "<the emulator account id>"} — 200
2. POST / with X-Amz-Target AWSFMS_20180101.PutAdminAccount and body {"AdminAccount": "222222222222", "AdminScope": {"AccountScope": {"AllAccountsEnabled": true}}} — 200
3. POST / with X-Amz-Target AWSFMS_20180101.DisassociateAdminAccount and body {} — 400 InvalidOperationException naming 222222222222 as an administrator that must disassociate first
4. Try to make 222222222222 leave: there is no operation that removes a delegated administrator. Calling DisassociateAdminAccount again (the only tear-down the model offers) repeats the same 400 — every request is signed by the single emulator account, so the handler's caller check can never be satisfied for 222222222222
5. POST / with X-Amz-Target AWSFMS_20180101.GetAdminScope and body {"AdminAccount": "222222222222"} — 200 with Status ONBOARDING_COMPLETE; wait any amount of time and repeat: Status never becomes OFFBOARDING or OFFBOARDING_COMPLETE
6. Repeat step 3 after any delay — still 400 InvalidOperationException. The default administrator's role never reaches PENDING_DELETION/DELETED, so GetAdminAccount keeps reporting READY forever
