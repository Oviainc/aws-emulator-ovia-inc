Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: ae5324a422b43795ea9373a6578734529d7c3346

# DeleteAlternateContact answers 404 ResourceNotFoundException for an AccountId the store holds no record of, instead of 403 AccessDeniedException

_Filed: 2026-10-24T04:45:33Z_
_Operation: DeleteAlternateContact | Class: error-response_

> _Note: this issue was identified against the code base at commit `ae5324a422b43795ea9373a6578734529d7c3346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler reaches straight into `accounts.get_mut(&account_id)` and treats a missing account the same as a missing contact, so a delete aimed at an AccountId the emulator holds no record of answers HTTP 404 `ResourceNotFoundException` ("No contact of the inputted alternate contact type found.").

The crate's own rule for an account outside the caller's organization — applied by GetAccountInformation, GetContactInformation, GetPrimaryEmailUpdateStatus, GetRegionOptStatus and ListRegions through `state::account_not_accessible` — is HTTP 403 `AccessDeniedException`, which is what real AWS answers: the member-account authorization check in the `AccountId` documentation precedes the contact lookup. The mutating paths that DO create a record on write (`account_mut`) are a separate design choice; this operation neither creates nor denies, it just reports a not-found for an account it has no business reporting on.

Expected: DeleteAlternateContact(AccountId=<id with no record>, AlternateContactType=BILLING) → 403 AccessDeniedException. Actual: 404 ResourceNotFoundException.

## Evidence

Spec: model doc for `DeleteAlternateContactRequest.AccountId` (caller must be an identity in the organization's management account and the id must be a member account); the operation models both `AccessDeniedException` (403) and `ResourceNotFoundException` (404).

Handler: services/service-com-amazonaws-account-account/src/ops/delete_alternate_contact.rs:200-219 — `state.lock().accounts.get_mut(&account_id).and_then(|account| account.alternate_contacts.remove(&contact_type))`, and any `None` becomes `ResourceNotFoundException`.

Tests: tests/all/account_settings.rs:263 `deleting_an_unset_contact_type_is_not_found` covers only the caller's own (existing) account; no test deletes against an account id the store holds no record of.

## Steps to Reproduce

1. Boot the emulator.
2. Call GetAccountInformation with AccountId="210987654321" and observe 403 AccessDeniedException (the store's unknown-account rule).
3. Call DeleteAlternateContact with AccountId="210987654321", AlternateContactType="BILLING".
4. Observe 404 ResourceNotFoundException instead of 403 AccessDeniedException.
