Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ae1f47ba1ed091886b5d17d179a2bc0dadf4c0eb

# An accountId naming a member account is admitted for the management account, then filtered out of the answer

_Filed: 2026-10-27T12:58:47Z_
_Operation: ListEnrollmentStatuses | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ae1f47ba1ed091886b5d17d179a2bc0dadf4c0eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's permission gate and its row scoping disagree about what `accountId` asks for. The gate treats `accountId != caller` as a request for another account's enrollment and lets it through when the caller IS the organization's management account. The scoping that follows then drops every record whose `account_id` is not the caller's unless `includeOrganizationInfo` is also set, so the admitted request answers an EMPTY `items` list instead of the named account's record. The named account's row is reachable only by ALSO setting `includeOrganizationInfo: true`, which the gate and the module doc treat as a separate, independent way of asking for other accounts' data.

## Evidence

The handler doc, `src/ops/list_enrollment_statuses.rs:9-12`:
```
//! `includeOrganizationInfo` and an `accountId` naming another account both ask
//! for the enrollment of the organization's member accounts, which only the
//! organization's management account may read, so either from any other caller
//! is refused with the modelled `AccessDeniedException`.
```
The gate agrees with that doc — `src/ops/list_enrollment_statuses.rs:182-189`:
```
let asks_for_other_accounts = input.include_organization_info
    || input
        .account_id
        .as_deref()
        .is_some_and(|wanted| wanted != caller);
if asks_for_other_accounts && !store.is_management_account(caller) {
    return Err(not_an_organization_view());
}
```
The scoping contradicts it — `src/ops/list_enrollment_statuses.rs:197-210`:
```
let scoped: Vec<crate::state::Enrollment> = store
    .enrollments
    .values()
    .filter(|enrollment| {
        input.include_organization_info || enrollment.account_id == caller
    })
    .filter(|enrollment| {
        input
            .account_id
            .as_deref()
            .is_none_or(|wanted| wanted == enrollment.account_id)
    })
```
The first `filter` keeps only the caller's record unless `includeOrganizationInfo` is set, and the second then excludes it because `wanted != caller` — so the request the gate just admitted as "a read of that member account's enrollment" can only ever answer an empty list.
Test coverage: the management-account arm of this operation is untested. `tests/all/management_account_gating.rs:143-207` exercises `accountId: "111122223333"` only through the non-management router (expecting 403 at lines 164-168), and no test calls ListEnrollmentStatuses through `management_account_router()`, so nothing pins the empty answer.

## Steps to Reproduce

1. Build the management-account router the crate's own tests use (`tests/all/cost_optimization_hub.rs:56` `management_account_router()`), so `is_management_account(caller)` is true.
2. Add a second enrollment record to the store (`state.lock().enrollments.insert("111122223333", Enrollment{account_id: "111122223333", status: "Active", ..})`), the member account the request will name.
3. POST ListEnrollmentStatuses {"includeOrganizationInfo": false, "accountId": "111122223333"} → HTTP 200 with `items: []`: the gate at lines 182-189 admitted the request as a management-account read of that member's enrollment, but the scoping at lines 197-210 removed the only matching record.
4. POST the same request with `"includeOrganizationInfo": true` → the record appears, showing the row exists and that the two code paths disagree about which member asks for it.
