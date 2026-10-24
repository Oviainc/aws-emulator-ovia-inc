Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# GetContactInformation answers 404 for an account that has never been written, although every AWS account has primary contact information

_Filed: 2026-10-24T06:56:28Z_
_Operation: GetContactInformation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator's account record is created with `contact_information: None` (`Account::new`), and `GetContactInformation` refuses any account whose record has not yet been written by `PutContactInformation` with HTTP 404 `ResourceNotFoundException`. That includes the caller's OWN account, which the store seeds at start-up — so the very first `GetContactInformation` call against a fresh emulator is a 404.

Against real AWS, the primary contact information (full name, address, city, postal code, country code, phone number) is mandatory data captured when the account is created and inherited for accounts created through Organizations, so a read of your own account's primary contact always answers HTTP 200 with that structure plus `VerificationStatus`. A client workflow that reads the primary contact before writing one (the common "show me my account's address" path, and any read-modify-write that patches one member) therefore diverges: AWS returns the address, the emulator returns a not-found refusal.

The crate already applies the opposite — and correct — reasoning to the sibling member on the same record: `state::initial_account_name` exists precisely because "Every Amazon Web Services account HAS a name: the name is set when the account is created ... so no account ever reports that member absent", and `GetAccountInformation` therefore reports a name before any `PutAccountName`. `contact_information` is the one member of the same record that is left absent at creation (state.rs:388-393 "this emulator learns an account's postal address only by being told it, and never invents one"), which is what produces the divergent 404.

Expected: an account record this emulator holds reports a primary contact on the first read — a seeded, self-consistent `ContactInformation` satisfying the modelled constraints (all required members present, `CountryCode` two letters, `PhoneNumber` matching `^[+][\s0-9()-]+$`), replaced wholesale by the first `PutContactInformation`, with `ResourceNotFoundException` reserved for genuinely unknown resources. Actual: HTTP 404 `ResourceNotFoundException` ("No primary contact information is set for account <id>") until a put has happened.

## Evidence

Spec: `GetContactInformation` is documented as retrieving "the primary contact information of an Amazon Web Services account" (`smithy_query(query="operation", operation="GetContactInformation")`), and `ContactInformation` models `FullName`, `AddressLine1`, `City`, `PostalCode`, `CountryCode` and `PhoneNumber` as `@required` — i.e. an account's primary contact is a complete structure, not optional data. Well-established service behaviour: primary contact details are collected at sign-up and are always present for an existing account (the Billing/Account console always shows them), which is exactly the reasoning this crate already applies to `AccountName`.

Handler: services/service-com-amazonaws-account-account/src/ops/get_contact_information.rs:189-196 — `let Some(contact_information) = account.contact_information.clone() else { return Err(... ResourceNotFoundException(state::contact_information_not_set(&account_id)) ...) }`. State: src/state.rs:410-421 `Account::new` sets `contact_information: None` while seeding `account_name: initial_account_name(..)`; src/state.rs:66-83 documents why a name is seeded and src/state.rs:388-393 documents the deliberate choice not to seed an address; src/state.rs:504-511 seeds the caller's own account at start-up.

Tests: tests/all/contact_information.rs:408-448 `a_contact_that_was_never_stored_is_refused` pins the current behaviour (404 for the caller's own account "before any put"), i.e. the divergent response is asserted rather than the AWS one; no test reads a seeded primary contact.

## Steps to Reproduce

1. Boot the emulator (no prior calls to this service).
2. POST /getContactInformation with body {} (the caller's own account).
3. Observe HTTP 404 ResourceNotFoundException "No primary contact information is set for account 000000000000"; real AWS answers HTTP 200 with the account's ContactInformation and a VerificationStatus.
4. POST /putContactInformation with a complete ContactInformation, then POST /getContactInformation again — only now does the emulator answer 200, showing the read depends on a prior write that AWS does not require.
