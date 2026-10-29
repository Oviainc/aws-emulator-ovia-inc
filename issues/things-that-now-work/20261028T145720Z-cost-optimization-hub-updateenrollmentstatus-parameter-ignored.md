Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 436fee4160e0c164196671a7c5bd1a9f84eddc25

# UpdateEnrollmentStatus stores includeMemberAccounts but never enrolls the organization's member accounts

_Filed: 2026-10-28T14:57:20Z_
_Operation: UpdateEnrollmentStatus | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `436fee4160e0c164196671a7c5bd1a9f84eddc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents `UpdateEnrollmentStatus` as the action that enrolls an organization's MEMBER accounts when the caller is the management account, and `includeMemberAccounts` as the switch that asks for it. The handler only touches the CALLER's own record: it does `enrollments.entry(account_id)` for the caller, writes the caller's `status`, and stores the flag on the caller's record. No other account's `Enrollment` is created or updated, and nothing else in the crate reads the stored flag except `ListEnrollmentStatuses`, which echoes it back.

Consequence: a management account that calls `UpdateEnrollmentStatus{status:"Active", includeMemberAccounts:true}` while the store holds member-account enrollment records leaves every member record exactly as it was (still `Inactive`/`Failed`), while the answer and a following `ListEnrollmentStatuses` report `includeMemberAccounts: true` — i.e. the service claims the member accounts were included when no member account was enrolled. Because the recommendation readers gate on each account's own enrollment (`Enrollment::recommendations_available`), the member accounts also keep reading the empty recommendation set.

The crate's own test suite admits the gap in a comment while working around it by writing the member records straight into the store.

## Evidence

Smithy, `smithy_query(query="doc", shape="UpdateEnrollmentStatus")`: "Updates the enrollment (opt in and opt out) status of an account to the Cost Optimization Hub service." / "**If the account is a management account of an organization, this action can also be used to enroll member accounts of the organization.**" and `smithy_query(query="doc", shape="UpdateEnrollmentStatusRequest", member="includeMemberAccounts")`: "**Indicates whether to enroll member accounts of the organization if the account is the management account or delegated administrator.**" (the same two sentences are pasted into the handler file: src/ops/update_enrollment_status.rs:57-59 and :66-67).

Handler, src/ops/update_enrollment_status.rs:190-223 — only the caller's record is touched:
```
let mut store = state.lock();
let enrollment = store
    .enrollments
    .entry(account_id.clone())            // account_id = crate::state::caller_account_id()
    .or_insert_with(|| crate::state::Enrollment { ... });
...
enrollment.status = input.status.clone();
enrollment.include_member_accounts = include_member_accounts;
```
and src/ops/update_enrollment_status.rs:184-188 shows the flag's only use is being stored:
```
// An absent `includeMemberAccounts` is a request that does not ask for the
// organization's member accounts to be enrolled, so it stores `false` —
let include_member_accounts = Some(input.include_member_accounts.unwrap_or(false));
```

The crate's tests admit the gap, tests/all/account_scoped_reads.rs:277-285:
```
// The caller enrolls over the wire, as a client would; the members are
// recorded in the store, because no operation enrolls another account.
ok(&router, "UpdateEnrollmentStatus", &json!({"status": "Active", "includeMemberAccounts": true})).await;
record_enrollment(&store, MEMBER_ACTIVE, "Active", true);
```
(the call sends `includeMemberAccounts: true` as the MANAGEMENT account — `management_account_router_with_store()` — and the test still has to write the member records itself). Existing coverage only pins the round-trip of the stored flag (tests/all/cost_optimization_hub.rs:349-390), which a fix would leave unchanged.

## Steps to Reproduce

1. Build a router whose store records the caller as the organization's management account, as tests/all/cost_optimization_hub.rs:70-79 does (`organization_management_account_id = Some(emulator_account_id())`), and record a member account's enrollment with status "Inactive" (tests/all/account_scoped_reads.rs:47-61 `record_enrollment`).
2. Call `ListEnrollmentStatuses{"includeOrganizationInfo": true}` and note the member account is `Inactive`.
3. Call `UpdateEnrollmentStatus{"status": "Active", "includeMemberAccounts": true}` — HTTP 200, `status: "Active"`.
4. Call `ListEnrollmentStatuses{"includeOrganizationInfo": true}` again: the member account is still `Inactive` (nothing was enrolled), while the answer reports `includeMemberAccounts: true`.
5. Expected per the model sentence quoted above: the management account's request with `includeMemberAccounts: true` also enrolls the organization's member accounts the store knows about.
