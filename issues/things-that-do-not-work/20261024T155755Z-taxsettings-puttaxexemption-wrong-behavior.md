Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# PutTaxExemption answers 404 for any account that has no stored tax registration, so an exemption cannot be submitted for a plain billing-family member account

_Filed: 2026-10-24T15:57:55Z_
_Operation: PutTaxExemption | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: PutTaxExemption "adds the tax exemption for a single account or all accounts listed in a consolidated billing family". Membership in a consolidated billing family is not conditioned on the member account holding a tax registration — a brand-new member account with no TRN at all is a perfectly normal target for an exemption submission, and the real service answers 200 with a `caseId` (the support case that carries the certificate for review) and records the exemption against every account the request named (1..=550 of them).

Actual: the handler pre-screens every requested account with `store.reports_exemptions_for(account_id)`, which is true only when the id equals the emulator's own account id, or the store already holds a tax REGISTRATION for that id, or the store already holds an EXEMPTION for it. Any other 12-digit account id makes the handler return a call-level `ResourceNotFoundException` (HTTP 404) with the message "Account <id> is not in your consolidated billing family, so it has no tax exemptions to report", and nothing is stored for ANY of the accounts the call named — the submission is refused whole.

Observable consequence: the only way to grant an exemption to a sibling account in this emulator is to first call PutTaxRegistration for that account, an API call the real service does not require and that has nothing to do with exemptions. A client that does `PutTaxExemption(accountIds=["111111111111","222222222222"], ...)` against a fresh emulator gets 404 where AWS returns 200 + caseId. The emulator cannot verify family membership (it models no organization), so conditioning the refusal on "this account happens to have a TRN" invents a precondition rather than approximating family membership: an unknown-but-well-formed account id is indistinguishable from a family member here, and the batch-read side of this same predicate is already known to be over-strict.

This also makes the 404 branch unreachable for the legitimate case it is meant for (an id truly outside the family) while firing on the common case (a family member without a TRN).

## Evidence

Spec: `smithy_query(query="operation", operation="PutTaxExemption")` — input `accountIds` (AccountIds, @length 1..=550, each a 12-digit AccountId), `authority`, `exemptionType`, `exemptionCertificate`; output `caseId`; errors include ResourceNotFoundException (404). The operation documentation describes adding the exemption for "a single account or all accounts listed in a consolidated billing family" — it states no requirement that the target account already hold a tax registration, and the model has no operation that enrolls an account in a billing family, so no request sequence can make an arbitrary member account "known" except an unrelated PutTaxRegistration.

Handler: `src/ops/put_tax_exemption.rs` `handle()` lines ~214-221 — `for account_id in &input.account_ids { if !store.reports_exemptions_for(account_id) { return Err(... no_exemptions_for_account(account_id) ...) } }`, executed before `store_exemptions(..)`, so the whole submission is refused. The predicate is `src/state.rs` `reports_exemptions_for()` lines ~362-366: `account_id == ::engine::emulator_account_id() || self.registrations.contains_key(account_id) || self.exemptions.contains_key(account_id)`.

Tests: `tests/all/tax_exemptions.rs::a_submission_for_an_account_outside_the_billing_family_is_refused` (~line 1123) pins the 404, and `hold_tax_settings_for()` (~line 777) exists purely to write a TRN for each account before a PutTaxExemption test can succeed — i.e. the suite encodes the invented precondition rather than testing the documented path. No test covers a submission for an account that holds no tax registration succeeding.

## Steps to Reproduce

1. Start the emulator with empty taxsettings state.
2. POST /PutTaxExemption with {"accountIds":["111111111111"],"authority":{"country":"US"},"exemptionType":"Reseller","exemptionCertificate":{"documentName":"cert.pdf","documentFile":"<base64 bytes>"}} using an exemptionType description that GetTaxExemptionTypes publishes.
3. Observe HTTP 404 ResourceNotFoundException ("Account 111111111111 is not in your consolidated billing family...") instead of 200 with a caseId; nothing is recorded.
4. POST /PutTaxRegistration for accountId 111111111111 with a complete taxRegistrationEntry, then repeat step 2 — now the identical exemption request answers 200 with a caseId, showing the submission's outcome depends on an unrelated tax-registration write.
5. POST /PutTaxExemption with {"accountIds":["<emulator own account>","111111111111"]} (one readable, one not) and observe the whole call 404: the exemption is not recorded even for the readable account.
