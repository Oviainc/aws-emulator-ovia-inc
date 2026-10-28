Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 20ab55bd9317ee9a3c91528cd79e8ca342e82eb9

# The "an AccountId the store holds no record of is an authorization refusal" branch is written out twelve times instead of once beside target_account_id

_Filed: 2026-10-28T21:47:57Z_
_Operation: GetAccountInformation | Class: maintainability_

> _Note: this issue was identified against the code base at commit `20ab55bd9317ee9a3c91528cd79e8ca342e82eb9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate owns both halves of its account-target rule in `state.rs`: `target_account_id` resolves the account-id member (and is documented there as "the one resolution every operation taking an account id goes through"), and `account_not_accessible` is the one payload for an account the caller may not read. What is NOT stated once is their COMPOSITION — "resolve the member, then refuse an id this emulator holds no record of with `account_not_accessible`" — which is open-coded in twelve handlers: the eight reporting handlers (`GetAccountInformation`, `GetContactInformation`, `GetAlternateContact`, `GetPrimaryEmail`, `GetPrimaryEmailUpdateStatus`, `GetRegionOptStatus`, `ListRegions`, `GetGovCloudAccountInformation`, each via `store.account(..)`) and the four mutating handlers that must not create a record (`DeleteAlternateContact`, `SendPhoneNumberVerification`, `VerifyPhoneNumber`, `AcceptPrimaryEmailUpdate`, each via `store.existing_account_mut(..)`).

The copies are identical modulo the per-operation error enum name, and they can drift: a change to which refusal an unknown account earns, or to the order in which the member is resolved against the store lookup, has to be made twelve times and nothing ties the copies together. This is the next layer of exactly the duplication a previous maintainability fix removed from this crate (the own-account comparison, folded into `state::target_account_id`); the composition above it was left behind.

The fix is response-identical: one helper next to `target_account_id` in `state.rs` — e.g. `ComAmazonawsAccountAccountState::reporting_account(&self, requested: Option<&str>) -> Result<&Account, AccessDeniedException>` (and the `existing_account` counterpart) returning `cannot_target_own_account()` / `account_not_accessible(..)` unchanged — with each handler keeping only its own `Err(XErrorModeled::AccessDeniedException(denied).into())` mapping. Every status, body and message stays byte-identical, and every existing test keeps passing.

## Evidence

state.rs already owns both halves, and claims the resolution is stated once — services/service-com-amazonaws-account-account/src/state.rs:19-25:
```
//! Most Account Management operations take an OPTIONAL `AccountId`: absent, the
//! call lands on "the Amazon Web Services account of the identity used to call
//! the operation" ... Present, it names a MEMBER
//! account of the caller's organization, which the caller's own account is not:
//! that is [`target_account_id`], the one resolution every operation taking an
//! account id goes through.
```
state.rs:1189-1193 (the read lookup) and state.rs:1300-1308 (the one refusal payload):
```
    /// The account `account_id` names, for a REPORTING operation: no record is
    /// created, so a read leaves the store exactly as it found it.
    pub fn account(&self, account_id: &str) -> Option<&Account> {
        self.accounts.get(account_id)
    }
```
```
pub fn account_not_accessible(account_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "The operation failed because the calling identity doesn't have the minimum \
             required permissions for account {account_id}"
        ),
```
Copy 1 — src/ops/get_account_information.rs:182-202:
```
        let account_id = match crate::state::target_account_id(input.account_id.as_deref()) {
            Ok(account_id) => account_id,
            Err(denied) => {
                return Err(GetAccountInformationErrorModeled::AccessDeniedException(denied).into())
            }
        };
        ...
        let store = state.lock();
        let Some(account) = store.account(&account_id) else {
            ...
            return Err(GetAccountInformationErrorModeled::AccessDeniedException(
                crate::state::account_not_accessible(&account_id),
            )
            .into());
        };
```
Copy 2 — src/ops/get_contact_information.rs:180-199:
```
        let account_id = match crate::state::target_account_id(input.account_id.as_deref()) {
            Ok(account_id) => account_id,
            Err(denied) => {
                return Err(GetContactInformationErrorModeled::AccessDeniedException(denied).into())
            }
        };
        // A REPORTING operation: the store is read, never written, so no record is
        // created here and a later caller sees exactly what this one saw.
        let store = state.lock();
        let Some(account) = store.account(&account_id) else {
            ...
            return Err(GetContactInformationErrorModeled::AccessDeniedException(
                crate::state::account_not_accessible(&account_id),
            )
            .into());
        };
```
The same two steps again at src/ops/get_alternate_contact.rs:187-207, src/ops/get_primary_email.rs:182-204, src/ops/get_primary_email_update_status.rs:178-206, src/ops/get_region_opt_status.rs:185-200, src/ops/list_regions.rs:235-265 and src/ops/get_gov_cloud_account_information.rs:190-218; and the `existing_account_mut` form at src/ops/delete_alternate_contact.rs:219-224, src/ops/send_phone_number_verification.rs:202-209, src/ops/verify_phone_number.rs:202-207 and src/ops/accept_primary_email_update.rs:238-243, e.g. verify_phone_number.rs:202-207:
```
        let Some(account) = store.existing_account_mut(&account_id) else {
            return Err(VerifyPhoneNumberErrorModeled::AccessDeniedException(
                crate::state::account_not_accessible(&account_id),
            )
            .into());
        };
```
Tests that cover every copy and must stay unchanged — tests/all/unknown_account_refusal.rs:114-209 builds a row for all twelve operations (`GetAccountInformation`, `GetContactInformation`, `GetPrimaryEmail`, `GetPrimaryEmailUpdateStatus`, `GetGovCloudAccountInformation`, `GetRegionOptStatus`, `ListRegions` (plain and "filtered and paged"), `SendPhoneNumberVerification`, `VerifyPhoneNumber`, `AcceptPrimaryEmailUpdate`, plus `GetAlternateContact`/`DeleteAlternateContact` per contact type), and :238 `an_account_with_no_record_is_an_authorization_refusal`, :269 `a_refused_call_leaves_the_account_unknown`, :298 `an_account_with_a_record_is_not_an_authorization_refusal`, :336 `an_unset_contact_on_a_known_account_is_still_not_found` pin the 403 `AccessDeniedException` and its message; tests/all/own_account_target_refusal.rs and tests/all/account_target_resolution_is_shared.rs pin the other arm of the same composition.

## Steps to Reproduce

1. Open services/service-com-amazonaws-account-account/src/state.rs:19-25, :1189-1193 and :1300-1308: the crate states the member resolution (`target_account_id`), the read lookup (`account`) and the refusal payload (`account_not_accessible`) once each, but never their composition.
2. Open src/ops/get_account_information.rs:182-202 and src/ops/get_contact_information.rs:180-199 side by side: the same two steps (resolve the member, then refuse an id the store holds no record of with `account_not_accessible`) are written out in both, differing only in the error-enum name.
3. Repeat for src/ops/get_alternate_contact.rs:187-207, get_primary_email.rs:182-204, get_primary_email_update_status.rs:178-206, get_region_opt_status.rs:185-200, list_regions.rs:235-265, get_gov_cloud_account_information.rs:190-218, and for the `existing_account_mut` form in delete_alternate_contact.rs:219-224, send_phone_number_verification.rs:202-209, verify_phone_number.rs:202-207 and accept_primary_email_update.rs:238-243 — twelve copies of one branch.
4. Confirm the replacement is response-identical: a `state.rs` helper that returns `Err(cannot_target_own_account())` for the caller's own id, `Err(account_not_accessible(&id))` for an id with no record and `Ok(&account)` otherwise computes exactly what each copy computes today.
5. Run `cargo test -p service-com-amazonaws-account-account unknown_account_refusal own_account_target_refusal account_target_resolution_is_shared` — these suites cover all twelve copies and must keep passing unchanged after the rule is stated once.
