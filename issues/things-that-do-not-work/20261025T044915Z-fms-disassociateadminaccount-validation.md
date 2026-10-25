Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# DisassociateAdminAccount ignores the documented first-in-last-out rule now that PutAdminAccount onboards delegated administrators

_Filed: 2026-10-25T04:49:15Z_
_Operation: DisassociateAdminAccount | Class: validation_

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents a precondition on disassociating the DEFAULT administrator: every other Firewall Manager administrator of the organization must disassociate first. The handler never consults `store.organization_admins` — the map `PutAdminAccount` writes delegated administrators into — so the default administrator's tear-down is always accepted with HTTP 200 even while delegated administrators are onboarded.

The handler's own module doc states the rule and then excuses itself from it on a premise that is no longer true: it says `PutAdminAccount` "is still a 501 and this service's state therefore holds no administrator other than the default one". `PutAdminAccount` is implemented and inserts into `organization_admins`, so the rule now has exactly the case the comment says does not exist. After the tear-down the organisation is left holding onboarded delegated administrators with no default administrator, and because `require_administrator` then refuses every organisation-scoped call, those records can no longer be read (`GetAdminScope`, `ListAdminAccountsForOrganization`) nor replaced (`PutAdminAccount`) until a new default administrator is associated.

## Evidence

Smithy model, `com.amazonaws.fms#DisassociateAdminAccount` documentation (smithy_query query="doc"): "Disassociation of the default administrator account follows the first in, last out principle. If you are the default administrator, all Firewall Manager administrators within the organization must first disassociate their accounts before you can disassociate your account."

The handler admits the gap and names the stale reason — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/disassociate_admin_account.rs:30-34:
```
//! The first-in-last-out rule in the same paragraph concerns the NON-default
//! administrators `PutAdminAccount` creates. That operation is still a 501 and
//! this service's state therefore holds no administrator other than the default
//! one, so the rule has no case to refuse yet; it belongs to `PutAdminAccount`'s
//! own gap issue.
```

But `PutAdminAccount` is implemented and persists those administrators — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_admin_account.rs:298-303:
```
                store.organization_admins.insert(
                    input.admin_account.clone(),
                    crate::state::OrganizationAdminRecord {
                        account_id: input.admin_account.clone(),
                        admin_scope: input.admin_scope.clone().unwrap_or_default(),
                        status: crate::state::ORGANIZATION_STATUS_ONBOARDING.to_string(),
```

And the whole decision the handler makes — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/disassociate_admin_account.rs:241-258 — reads only `admin_account`, never `organization_admins`:
```
        let Some(admin) = store.active_admin_account_mut_at(now) else {
            return Err(crate::state::no_administrator_refusal(&region));
        };
        ...
        if !admin.removal_pending() {
            admin.role_status = crate::state::ROLE_STATUS_PENDING_DELETION.to_string();
            admin.removal_due = Some(now + crate::state::DISASSOCIATION_SPAN);
        }
        Ok(DisassociateAdminAccountOutput)
```

Test coverage: services/service-com-amazonaws-fms-awsfms-20180101/tests/all/admin_account_disassociation.rs covers only the caller-identity refusal, the pending span and re-association (`disassociate_is_refused_for_a_caller_that_does_not_hold_the_association`, `the_holder_disassociates_and_the_role_is_deleted_after_the_pending_span`, `associating_over_a_pending_disassociation_is_refused`); a repository-wide grep for `first.in.*last.out|first-in-last-out|FILO` under tests/ returns "No matches.", so no test pins the current behaviour.

## Steps to Reproduce

1. POST / with X-Amz-Target AWSFMS_20180101.AssociateAdminAccount and body {"AdminAccount": "<the emulator account id>"} — 200, the emulator account is now the default administrator
2. POST / with X-Amz-Target AWSFMS_20180101.PutAdminAccount and body {"AdminAccount": "222222222222", "AdminScope": {"AccountScope": {"AllAccountsEnabled": true}}} — 200, a delegated administrator is now onboarded (visible via GetAdminScope / ListAdminAccountsForOrganization)
3. POST / with X-Amz-Target AWSFMS_20180101.DisassociateAdminAccount and body {} — answers 200 and moves the default administrator to PENDING_DELETION, although the delegated administrator 222222222222 has not disassociated; the documented first-in-last-out precondition is never checked
4. After the disassociation span, call GetAdminScope with {"AdminAccount": "222222222222"} — now AccessDeniedException, so the still-onboarded delegated administrator is unreachable
