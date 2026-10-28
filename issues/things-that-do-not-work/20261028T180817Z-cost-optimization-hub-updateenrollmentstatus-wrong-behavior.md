Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 35520d9c06247da719cc1093e5db81eb1d87c7db

# UpdateEnrollmentStatus stores includeMemberAccounts but never enrolls the organization's member accounts

_Filed: 2026-10-28T18:08:17Z_
_Operation: UpdateEnrollmentStatus | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `35520d9c06247da719cc1093e5db81eb1d87c7db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy model documents `UpdateEnrollmentStatus` as the action that also enrolls the organization's MEMBER accounts when the caller is the management account, and models `includeMemberAccounts` as the flag that asks for it. The handler does the opposite: it resolves exactly one account id (the caller's) and writes exactly one record, storing `includeMemberAccounts` as an inert per-account flag. No other account's `Enrollment` is created or updated, whatever the caller is and whatever the flag says.

The consequence is observable through this crate's own organization-scoped reader: a management account that opts in with `includeMemberAccounts: true` still sees every member account's record at its previous status in `ListEnrollmentStatuses{includeOrganizationInfo:true}`, and those members' catalog rows stay invisible, because `visible_recommendations_for` judges enrollment per account (`state.rs:1076-1092`). The crate already owns the account set this write would need — `addressed_accounts(caller, named, whole_organization)` returns "every account of the organization where the request asks for the organization's view" and is what `ListEnrollmentStatuses` scopes with — so this is a write path skipping a helper the crate has, not a missing subsystem. The output member is likewise reported back to a caller for whom nothing member-related happened: `ListEnrollmentStatuses` answers `includeMemberAccounts` straight out of the caller's stored flag (`list_enrollment_statuses.rs:195-198`).

## Evidence

Smithy model (`smithy_query(query="doc", shape="UpdateEnrollmentStatus")`): "If the account is a management account of an organization, this action can also be used to enroll member accounts of the organization." — mirrored into the handler's own header, `src/ops/update_enrollment_status.rs:57-62`.

Member doc, `src/ops/update_enrollment_status.rs:66-67`: "`includeMemberAccounts` (Boolean) — Indicates whether to enroll member accounts of the organization if the account is the management account or delegated administrator."

Handler, `src/ops/update_enrollment_status.rs:182` and `:190-222`:
```
let account_id = crate::state::caller_account_id().to_string();
...
let enrollment = store
    .enrollments
    .entry(account_id.clone())
    .or_insert_with(|| crate::state::Enrollment { account_id: account_id.clone(), ... });
...
enrollment.include_member_accounts = include_member_accounts;
```
Only `enrollments[caller]` is touched; `include_member_accounts` is stored and nothing else is derived from it anywhere in the crate (grep for `include_member_accounts` finds only this write and the read-back in `src/ops/list_enrollment_statuses.rs:195-198`).

The helper this path skips, `src/state.rs:1042-1058`: "A request that NAMES accounts ... naming none it addresses the calling account, widened to every account of the organization where the request asks for the organization's view (`includeOrganizationInfo`)" — used by `src/ops/list_enrollment_statuses.rs:204-209`.

The gap is admitted in the test suite, `tests/all/account_scoped_reads.rs:277-288`, which sends the management-account request `{"status": "Active", "includeMemberAccounts": true}` and then comments: "The caller enrolls over the wire, as a client would; the members are recorded in the store, because no operation enrolls another account." — the members' `Enrollment` rows are injected by `record_enrollment(...)` instead. No test asserts that member accounts stay unenrolled as a RULE.

## Steps to Reproduce

1. Build the service over a store that records an organization, as `tests/all/cost_optimization_hub.rs:70-79` (`management_account_router_with_store`) does, and insert a member account's enrollment with status `Inactive` the way `tests/all/account_scoped_reads.rs:47-61` (`record_enrollment`) does.
2. Call `UpdateEnrollmentStatus` as the management account with `{"status": "Active", "includeMemberAccounts": true}` — HTTP 200, status `Active`.
3. Call `ListEnrollmentStatuses` with `{"includeOrganizationInfo": true}`: the member account is still reported `Inactive`, although the model says this action "can also be used to enroll member accounts of the organization".
4. Call `ListRecommendations` with `{"includeAllRecommendations": true, "filter": {"accountIds": ["<member account>"]}}`: still empty, because the member's enrollment never became Active (`state.rs:1076-1092`).
