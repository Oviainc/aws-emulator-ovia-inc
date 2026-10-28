Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e94a5165da5e1a7dc4e65daf2da688ae995eb229

# member_accounts() re-reads the clock instead of taking the request's `now`, so one request can see the administrator active and then gone

_Filed: 2026-10-28T21:14:48Z_
_Operation: BatchAssociateResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e94a5165da5e1a7dc4e65daf2da688ae995eb229`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other clock-derived predicate in `crate::state` takes the request's single `now` and documents why: `AdminAccountRecord::is_active_at(now)`, `admin_account_summaries(now)`, `other_administrator_accounts(now)`, `managing_admin_accounts(now)` and `organization_admin_at(.., now)`. `ComAmazonawsFmsAwsfms20180101State::member_accounts()` is the one exception: it takes no `now` and calls `is_active()`, which reads `SystemTime::now()` itself (state.rs:80-82).

That makes a SECOND clock reading inside a request that has already made its decision from a first one. `BatchAssociateResource` reads `now` once (batch_associate_resource.rs:220) and uses it for the membership write, and it passes the administrator gate (`require_administrator`, which is also `is_active()`), but the organization's accounts it classifies every item against come from `organization_accounts(&store, discoverable)` -> `member_accounts()` (batch_associate_resource.rs:282, state.rs:2807), which reads the clock again. If the stored `removal_due` instant falls between those two readings, the gate admits the call while the account set no longer contains the administrator's own account, so every item whose ARN names that account is classified `NOT_VALID_ACCOUNT_ID` (state.rs:2759-2761) and silently lands in `FailedItems` instead of being associated. The same second reading reaches `policy_scope_accounts` -> `member_accounts()` (state.rs:1628), which decides the not-found of `GetComplianceDetail` / `GetViolationDetails` and the page content of `ListComplianceStatus`, so the same request can be authorized by one clock reading and answer `ResourceNotFoundException` from another.

The defect is the missing `now` parameter: `member_accounts()` cannot be made to agree with the reading its caller already made.

## Evidence

The rule the crate states for exactly this predicate, twice:

- `services/.../src/state.rs:1461-1462` — "It takes `now` for the reason [`AdminAccountRecord::is_active_at`] does: one request decides from ONE reading of a clock that is not monotonic."
- `services/.../src/state.rs:1494-1497` — "It takes `now` for the same reason [`AdminAccountRecord::is_active_at`] does: one request decides from ONE reading of a clock that is not monotonic, so two reads inside one handler cannot straddle a settle boundary."
- `services/.../src/state.rs:72-75` — "It takes `now` so one request decides from ONE reading of a clock that is not monotonic: an operation that checks the association and then writes to it must not be able to see it active and then gone."

The code that breaks it:

- `services/.../src/state.rs:1544-1548`:
  ```
  pub fn member_accounts(&self) -> BTreeSet<String> {
      let mut accounts = BTreeSet::new();
      if let Some(admin) = self.admin_account.as_ref().filter(|a| a.is_active()) {
  ```
  (`is_active()` is `self.is_active_at(SystemTime::now())`, state.rs:80-82.)

The caller that already holds one reading:

- `services/.../src/ops/batch_associate_resource.rs:220` — `let now = ::std::time::SystemTime::now();`
- `services/.../src/ops/batch_associate_resource.rs:230` — `crate::state::require_administrator(&store, &region)?;`
- `services/.../src/ops/batch_associate_resource.rs:282` — `let accounts = crate::state::organization_accounts(&store, discoverable);` which is `store.member_accounts()` (state.rs:2807) — a second clock read, with `now` in scope and unused for it.

Test coverage: `tests/all/member_accounts.rs` pins what `member_accounts()` reports, `tests/all/admin_account_disassociation.rs` pins the tear-down span, and `tests/all/associable_discovered_resources.rs` pins the item classification — none of them asserts that the two readings may differ.

## Steps to Reproduce

1. `AssociateAdminAccount` with the emulator's own account id, so that account is the only entry `member_accounts()` contributes.
2. `PutResourceSet` with a `ResourceTypeList` naming a discoverable type, and create one such resource in the owning service so its ARN carries the administrator's account.
3. `DisassociateAdminAccount`, which sets `removal_due = now + DISASSOCIATION_SPAN` (state.rs:296).
4. Call `BatchAssociateResource` with that resource's ARN timed so that `crate::state::DISASSOCIATION_SPAN` (500 ms) elapses between the gate at batch_associate_resource.rs:230 and the account derivation at batch_associate_resource.rs:282.
5. The gate admits the call, but `member_accounts()` no longer reports the administrator's account, so the item is returned in `FailedItems` with reason `NOT_VALID_ACCOUNT_ID` instead of being associated — two readings of one clock disagreeing inside one request, which the quoted doc says must not be possible.
