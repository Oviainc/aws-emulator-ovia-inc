Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 56752714ba65963d122a47feae790978b4aace00

# PutAdminAccount answers 200 for an administrator whose removal is in flight, leaving the pending OFFBOARDING_COMPLETE in place so the "created or updated" administrator is removed moments later

_Filed: 2026-10-29T10:21:53Z_
_Operation: PutAdminAccount | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `56752714ba65963d122a47feae790978b4aace00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutAdminAccount` splits `organization_admins` into exactly two cases: an account that `DisassociateAdminAccount` has ALREADY removed (`!is_administrator_at(now)`) is re-created from scratch through `OrganizationAdminRecord::onboarding`, and any other existing record is "UPDATE"d by overwriting `admin_scope` and `updated_at` only.

An account whose removal is still IN FLIGHT falls into the second case: `OrganizationAdminRecord::is_administrator_at` is `status_at(now) != ORGANIZATION_STATUS_OFFBOARDING_COMPLETE`, and during `OFFBOARDING` the pending transition is not due yet, so `status_at(now) == "OFFBOARDING"` and the record counts as an administrator. The UPDATE arm then writes the new scope but leaves `settles_to = OFFBOARDING_COMPLETE` / `settles_at` untouched. `ADMIN_OFFBOARDING_SPAN` later the clock settles the record to `OFFBOARDING_COMPLETE`: the account that `PutAdminAccount` just answered 200 for — "creates or updates an Firewall Manager administrator account" — is an administrator of the organization no longer (`is_administrator_at` false), manages no account (`managing_admin_accounts` requires `ONBOARDING_COMPLETE`), and `GetAdminScope` / `ListAdminAccountsForOrganization` report it `OFFBOARDING_COMPLETE` while carrying the scope the successful call wrote. Nothing in the call is undone and nothing is refused; the write is simply thrown away by a transition the handler never looked at.

This is the one lifecycle writer in the crate that ignores an in-flight transition. The record type itself states the mirror rule for the other direction (`start_offboarding` SUPERSEDES an onboarding still in flight), `AssociateThirdPartyFirewall` re-designates an association "whose teardown is still in flight" by resetting the record, and the sibling writer of the OTHER administrator record, `AssociateAdminAccount`, refuses exactly this state with the modelled `InvalidOperationException`. Either of those two resolutions is self-consistent; silently updating a record that is on its way out is neither.

## Evidence

The handler's two arms — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_admin_account.rs:302-325:
```
        match store.organization_admins.get_mut(&input.admin_account) {
            // CREATE AGAIN: the account was an administrator and HAS BEEN
            // REMOVED as one (`DisassociateAdminAccount` took it out, and the
            // clock completed the removal), ...
            Some(existing) if !existing.is_administrator_at(now) => {
                *existing = crate::state::OrganizationAdminRecord::onboarding(
                    &input.admin_account,
                    input.admin_scope.clone().unwrap_or_default(),
                    now,
                );
            }
            // UPDATE: this account already is an administrator, so only its
            // scope changes; an absent `AdminScope` leaves the stored one as it
            // is, which is what makes a repeated call idempotent.
            Some(existing) => {
                if let Some(scope) = input.admin_scope.clone() {
                    existing.admin_scope = scope;
                }
                existing.updated_at = now;
            }
```
The module doc enumerates only those two cases and never the removal in flight — put_admin_account.rs:26-35:
```
//!   `DisassociateAdminAccount` has already REMOVED as an administrator is
//!   created again this way rather than updated, since it is no administrator
//!   for this call to update;
//! * an UPDATE replaces the stored scope, leaves the account's onboarding and
//!   its `onboarded_at` alone ...
```
An `OFFBOARDING` record IS "an administrator" to that predicate — src/state.rs:298-300:
```
    pub fn is_administrator_at(&self, now: SystemTime) -> bool {
        self.status_at(now) != ORGANIZATION_STATUS_OFFBOARDING_COMPLETE
    }
```
and the pending removal the UPDATE arm leaves behind is what the clock then applies — src/state.rs:315-326:
```
    pub fn start_offboarding(&mut self, now: SystemTime) {
        ...
        self.status = ORGANIZATION_STATUS_OFFBOARDING.to_string();
        self.settles_to = Some(ORGANIZATION_STATUS_OFFBOARDING_COMPLETE.to_string());
        self.settles_at = Some(now + ADMIN_OFFBOARDING_SPAN);
```
The SUPERSEDE rule this record type states for the opposite direction — src/state.rs:308-311:
```
    /// An onboarding still in flight is SUPERSEDED rather than waited for: an
    /// account on its way out is being removed from that instant, so the
    /// transition the clock owed it is replaced by this one.
```
The sibling writer of the other administrator record refuses the same state — src/ops/associate_admin_account.rs:241-251:
```
            Some((existing, true)) => Err(
                AssociateAdminAccountErrorModeled::InvalidOperationException(
                    InvalidOperationException {
                        message: Some(format!(
                            "the Firewall Manager administrator association for account \
                             {existing} is pending deletion"
```
with its stated reason — associate_admin_account.rs:43-47:
```
//! * while a disassociation is still pending, the account is on its way out and
//!   its role is being deleted, so there is no association to make and no
//!   existing one to be idempotent about: that is the "wasn't possible" case of
//!   `InvalidOperationException`.
```
and the vendor-association writer, which resets an in-flight teardown — src/ops/associate_third_party_firewall.rs:20-22:
```
//! An association that was
//! torn down — or whose teardown is still in flight — is designated again,
//! starting a fresh `ONBOARDING`: the mirror of `DisassociateThirdPartyFirewall`.
```

Test coverage: tests/all/administrator_offboarding.rs drives `PutAdminAccount` only AFTER the removal has completed — it calls `wait_out_the_offboarding()` and asserts `OFFBOARDING_COMPLETE` (lines 381-388) before the re-onboarding call at line 390 — and tests/all/get_admin_scope_lifecycle_status.rs exercises `OFFBOARDING` through `GetAdminScope` alone. No suite calls `PutAdminAccount` for an account whose `OFFBOARDING` is still in flight, so nothing pins the current behaviour.

## Steps to Reproduce

1. Boot the emulator and call `AssociateAdminAccount` with `{"AdminAccount": "111111111111"}` (an account other than the emulator's own, so the calling account can hold a delegated role) → 200.
2. Call `PutAdminAccount` with `{"AdminAccount": "<the emulator account id>", "AdminScope": {"AccountScope": {"AllAccountsEnabled": true}, "PolicyTypeScope": {"AllPolicyTypesEnabled": true}, "RegionScope": {"AllRegionsEnabled": true}}}` → 200, and wait out `ADMIN_ONBOARDING_SPAN` (500 ms) so `GetAdminScope` reports `ONBOARDING_COMPLETE`.
3. Call `DisassociateAdminAccount` (no members) → 200; the caller's delegated role goes `OFFBOARDING` with `OFFBOARDING_COMPLETE` due `ADMIN_OFFBOARDING_SPAN` (500 ms) later.
4. WITHIN that 500 ms window, call `PutAdminAccount` again for the same account with a different `AdminScope` (e.g. `{"RegionScope": {"Regions": ["eu-central-1"]}}`) → 200.
5. Wait out the rest of the offboarding span and call `GetAdminScope` for that account.
6. Expected (the resolution either sibling writer applies): the call is refused with `InvalidOperationException` because the account is on its way out, OR it supersedes the pending removal the way `OrganizationAdminRecord::start_offboarding` supersedes a pending onboarding, so the account is reported `ONBOARDING` and then `ONBOARDING_COMPLETE`.
7. Actual: `GetAdminScope` reports `Status: OFFBOARDING_COMPLETE` with the scope the second `PutAdminAccount` wrote — the 200 "create or update" produced no administrator, the account manages no account, and `ListAdminAccountsForOrganization` reports it removed.
