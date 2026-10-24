Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ae5324a422b43795ea9373a6578734529d7c3346

# GetAlternateContact answers 404 ResourceNotFoundException for an AccountId the emulator holds no record of, instead of the 403 AccessDeniedException every other read of this service uses

_Filed: 2026-10-24T04:45:33Z_
_Operation: GetAlternateContact | Class: error-response_

> _Note: this issue was identified against the code base at commit `ae5324a422b43795ea9373a6578734529d7c3346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler looks the account up and the contact up in one chain, so an AccountId that names no record in the store is indistinguishable from an account that simply has no contact of that type: both produce HTTP 404 `ResourceNotFoundException`.

Every OTHER reporting operation in this crate refuses an unknown account with HTTP 403 `AccessDeniedException` built by `state::account_not_accessible` — `GetAccountInformation`, `GetContactInformation`, `GetPrimaryEmailUpdateStatus`, `GetRegionOptStatus` and `ListRegions` all do. That rule is the right one and is grounded in the model's own `AccountId` documentation ("the caller must be an identity in the organization's management account or a delegated administrator account, and the specified account ID must be a member account in the same organization"): an account outside that organization is an authorization refusal, not a missing sub-resource. Real AWS evaluates that authorization before the contact lookup, so a non-member/nonexistent AccountId yields 403 `AccessDeniedException`.

Expected: GetAlternateContact(AccountId=<id the store holds no record of>, AlternateContactType=BILLING) → 403 AccessDeniedException. Actual: 404 ResourceNotFoundException with the alternate-contact not-found message, which also tells a caller that the contact type is unset on an account it is not allowed to read at all.

## Evidence

Spec: model doc for `GetAlternateContactRequest.AccountId` (the management-account / member-account rule); `GetAlternateContact` models both `AccessDeniedException` (403) and `ResourceNotFoundException` (404).

Handler: services/service-com-amazonaws-account-account/src/ops/get_alternate_contact.rs:181-196 — `store.account(&account_id).and_then(|account| account.alternate_contacts.get(&contact_type))`; `None` (either cause) returns `ResourceNotFoundException`. Compare services/service-com-amazonaws-account-account/src/ops/get_contact_information.rs:177-187 and get_account_information.rs:182-193, which return `state::account_not_accessible` (403) for the unknown-account case before looking at the sub-resource.

Tests: tests/all/account_settings.rs:577-587 only covers an account the store DOES hold with the contact unset (correctly 404). No test calls GetAlternateContact with an AccountId that has no record, so the 404-vs-403 divergence is uncovered.

## Steps to Reproduce

1. Boot the emulator.
2. Call GetAccountInformation with AccountId="210987654321" (an id no request has ever written) and observe 403 AccessDeniedException — the store's rule for an unknown account.
3. Call GetAlternateContact with AccountId="210987654321", AlternateContactType="BILLING".
4. Observe 404 ResourceNotFoundException instead of the 403 AccessDeniedException real AWS (and every other read of this emulator) answers for that same unknown account.
