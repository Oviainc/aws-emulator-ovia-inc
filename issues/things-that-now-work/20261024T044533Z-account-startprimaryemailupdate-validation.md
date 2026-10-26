Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ae5324a422b43795ea9373a6578734529d7c3346

# StartPrimaryEmailUpdate accepts a PrimaryEmail that is not an email address and records it as a PENDING update

_Filed: 2026-10-24T04:45:33Z_
_Operation: StartPrimaryEmailUpdate | Class: validation_

> _Note: this issue was identified against the code base at commit `ae5324a422b43795ea9373a6578734529d7c3346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PrimaryEmail` is modelled only `@length(min: 5, max: 64)`, and the handler deliberately applies no further check ("The model constrains the address no further, so no address format is invented here"). Any 5–64 character string — `aaaaa`, `not-an-email-address`, `@@@@@` — is therefore accepted with HTTP 200 and stored as the account's pending primary-email update, which `GetPrimaryEmailUpdateStatus` then reports as PENDING.

Real AWS refuses a malformed address on this operation with HTTP 400 `ValidationException` (the operation's modelled client error): the value becomes the root-user email address of a member account, and Account Management validates its syntax before sending the one-time passcode. This service already demonstrates that it validates email syntax where it can express it — `PutAlternateContact.EmailAddress` carries the modelled pattern `^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$` — so an address with no `@`/no domain is not a value this API admits.

Expected: 400 ValidationException (reason `fieldValidationFailed`, fieldList naming `PrimaryEmail`) for an address that is not of the form local@domain.tld, and no update recorded. Actual: 200 with Status=PENDING and the malformed value persisted.

## Evidence

Spec: `StartPrimaryEmailUpdate` models `ValidationException` (HTTP 400) and input `PrimaryEmail` (`PrimaryEmailAddress`, @length 5..64, @sensitive) documented as "The new primary email address (also known as the root user email address) to use in the specified account." The sibling member `PutAlternateContactRequest.EmailAddress` carries an explicit email pattern, showing the service rejects non-addresses.

Handler: services/service-com-amazonaws-account-account/src/ops/start_primary_email_update.rs:188-207 — only the length/pattern already enforced at the boundary is relied on; `PrimaryEmailUpdate::started(input.primary_email, now)` stores the value verbatim.

Tests: tests/all/primary_email_update.rs:393 `an_address_outside_the_modelled_bound_is_refused` covers only the length bound; no test asserts a refusal for a well-sized but malformed address.

## Steps to Reproduce

1. Boot the emulator.
2. Call StartPrimaryEmailUpdate with AccountId="210987654321" (not the caller's own account) and PrimaryEmail="notanemail".
3. Observe HTTP 200 with Status=PENDING; real AWS refuses with HTTP 400 ValidationException and records no update.
4. Call GetPrimaryEmailUpdateStatus with AccountId="210987654321" and observe the malformed address was accepted as a pending update (Status=PENDING, UpdatedAt set).
