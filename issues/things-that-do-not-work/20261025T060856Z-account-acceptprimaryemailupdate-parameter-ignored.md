Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# Otp is never minted, stored or compared, so any well-formed six-character value confirms a primary email update

_Filed: 2026-10-25T06:08:56Z_
_Operation: AcceptPrimaryEmailUpdate | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AcceptPrimaryEmailUpdate` takes a `@required` `Otp` member that the model documents as "The OTP code sent to the PrimaryEmail specified on the StartPrimaryEmailUpdate API call". The handler reads `input.otp` nowhere: after the pending check and the address-match check it calls `update.accept(now)` unconditionally, so every value inside `^[a-zA-Z0-9]{6}$` confirms the update and the documented confirmation step is a no-op. Nothing is stored for it either — `StartPrimaryEmailUpdate` never mints a passcode (`PrimaryEmailUpdate::started` records only `requested_email`, `status`, `updated_at`, `completes_at`), so the input member is dropped on the floor rather than persisted and checked.

This contradicts the crate's own treatment of the identical "one-time passcode delivered out of band" rule for the phone number: `SendPhoneNumberVerification` mints a passcode through the existing `crate::state::new_otp()` helper, stores it on the account, and `VerifyPhoneNumber` refuses a submission that is not the outstanding one with `crate::state::incorrect_passcode()`. `new_otp()`/`OTP_LENGTH` exist in `state.rs` precisely so a minted passcode is submittable, and the primary-email path is the one path that needs them and does not call them. The handler justifies the gap with "this emulator has no mailbox a caller could read it from", but the phone flow has no handset either and is implemented by holding the passcode in the store (its own test reads it out of the store "the way the phone's owner reads it off a text message"). One of the two sibling flows is wrong, and the one that drops a `@required`, documented input member is this one.

## Evidence

Model (smithy_query doc, AcceptPrimaryEmailUpdateRequest$Otp): "The OTP code sent to the <code>PrimaryEmail</code> specified on the <code>StartPrimaryEmailUpdate</code> API call."

The handler never looks at it — services/service-com-amazonaws-account-account/src/ops/accept_primary_email_update.rs:256-263:
```
        // The passcode itself: Account Management mails it to the proposed
        // address, and this emulator has no mailbox a caller could read it
        // from, so it cannot hold a particular passcode a caller would have to
        // present — and it invents none to compare against. What the model DOES
        // state about `Otp` is its form (`^[a-zA-Z0-9]{6}$`), which the
        // boundary refuses anything outside of; a passcode of that form
        // confirms the update.
        update.accept(now);
```
Nothing is minted at start — src/ops/start_primary_email_update.rs:209:
```
        let update = crate::state::PrimaryEmailUpdate::started(input.primary_email, now);
```
and `PrimaryEmailUpdate` (src/state.rs:536-560) carries `requested_email`, `status`, `updated_at`, `completes_at` and no passcode.

The sibling flow in the SAME crate does mint, store and compare — src/state.rs:92-96:
```
/// How many characters a one-time passcode has: `Otp` is modelled
/// `^[a-zA-Z0-9]{6}$` (`smithy_query(query="operation",
/// operation="VerifyPhoneNumber")`), so a passcode this service mints has to be
/// six alphanumerics for the caller to be able to submit it at all.
pub const OTP_LENGTH: usize = 6;
```
src/ops/send_phone_number_verification.rs:221-224:
```
        let verification =
            crate::state::PhoneNumberVerification::sent(phone_number, crate::state::new_otp(), now);
```
src/ops/verify_phone_number.rs:231-237:
```
        if input.otp != outstanding {
            ...
            return Err(VerifyPhoneNumberErrorModeled::ValidationException(
                crate::state::incorrect_passcode(),
```
`grep` over the crate shows `new_otp` has exactly one caller (src/ops/send_phone_number_verification.rs:222) and `outstanding_otp` exactly one (src/ops/verify_phone_number.rs:221) — the email path calls neither.

Test coverage: tests/all/primary_email_update_acceptance.rs:26-28 states the rule it pins — "`Otp` is constrained `^[a-zA-Z0-9]{6}$`: anything outside that form is refused with HTTP 400 and consumes nothing, and every passcode of that form is admitted" — and :500-509 asserts every well-formed passcode is admitted; its stated reason is only the modelled pattern, not a reason for ignoring the value. The phone suite shows the alternative is already implemented and testable: tests/all/phone_number_verification.rs:29-33 "the test reads the outstanding passcode out of the emulator's own store, the way the phone's owner reads it off a text message".

## Steps to Reproduce

1. POST /startPrimaryEmailUpdate with {"AccountId":"123456789012","PrimaryEmail":"ada@example.com"} → 200, Status PENDING. No passcode is recorded anywhere in the store (inspect `Account::primary_email_update`: it has no passcode field).
2. POST /acceptPrimaryEmailUpdate with {"AccountId":"123456789012","PrimaryEmail":"ada@example.com","Otp":"zzzzzz"} — an arbitrary value nothing ever sent anywhere → 200, Status ACCEPTED.
3. Repeat with "Otp":"000000", "Otp":"AAAAAA", any six alphanumerics on a fresh account: each one accepts the update, so the member has no effect on the outcome.
4. Compare the phone path: POST /sendPhoneNumberVerification → 200 PENDING, then POST /verifyPhoneNumber with {"Otp":"zzzzzz"} → 400 ValidationException "the submitted one-time passcode is incorrect", because that flow mints and stores the passcode it later compares.
