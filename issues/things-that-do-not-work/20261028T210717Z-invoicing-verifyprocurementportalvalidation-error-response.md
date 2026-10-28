Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# A fresh validation code starts with its attempt budget already spent, so the refusal names an attempt past the budget and prescribes a remedy it does not honour

_Filed: 2026-10-28T21:07:17Z_
_Operation: VerifyProcurementPortalValidation | Class: error-response_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The refused-code message is built from a per-preference counter that `SendProcurementPortalValidation` never resets, so issuing a fresh code does not give that code a fresh attempt budget.

`VerifyProcurementPortalValidation` increments `StoredProcurementPortalPreference::failed_verification_attempts` on every refused submission and renders the refusal as "Attempt {n} of 3 failed", switching to a second sentence once `n >= 3`: "Request a new validation code with SendProcurementPortalValidation." `SendProcurementPortalValidation` is the operation that sentence names, and its handler does exactly one write — it replaces `stored.validation` with a newly minted code — leaving `failed_verification_attempts` untouched.

So a client that follows the instruction the service just gave it gets a brand-new code whose budget is already reported as spent: the next wrong submission answers "Attempt 4 of 3 failed. Request a new validation code with SendProcurementPortalValidation." — an attempt number outside the budget it is stated "of", and the same remedy the caller has already applied, forever. The handler's own doc says that number is "what a client driving the OTP flow reads the remaining attempts off", and after one exhausted round that reading is permanently wrong for every later code.

The existing stated reason for not clamping the number ("never clamped to the budget, which would report an attempt that did not fail") is about clamping the display of attempts made against ONE issued code; it says nothing about whether a newly issued code inherits the previous code's spent budget. No test covers a send after the budget is exhausted.

## Evidence

The budget, and what the handler says a client reads off it — services/service-com-amazonaws-invoicing-invoicing/src/ops/verify_procurement_portal_validation.rs:135-144:
```
/// How many submissions of a validation code for one preference the refusal counts
/// against: the budget AWS's recorded refusal states ("Attempt 1 of 3 failed"), which is
/// what a client driving the OTP flow reads the remaining attempts off.
///
/// It is a REPORTING budget: nothing in this service's model or in the recorded bodies
/// says the service stops accepting submissions once it is spent, so no submission is
/// refused for exceeding it and the attempt the refusal names is the true count of failed
/// attempts — never clamped to the budget, which would report an attempt that did not
/// fail.
const MAX_VERIFICATION_ATTEMPTS: u32 = 3;
```

The message, which names the remedy once the budget is spent — verify_procurement_portal_validation.rs:155-168:
```
fn invalid_code_message(failed_attempts: u32) -> String {
    let budget = MAX_VERIFICATION_ATTEMPTS;
    if failed_attempts < budget {
        format!(
            "The code you entered is invalid. Attempt {failed_attempts} of {budget} failed. \
             Please try again."
        )
    } else {
        format!(
            "The code you entered is invalid. Attempt {failed_attempts} of {budget} failed. \
             Request a new validation code with SendProcurementPortalValidation."
        )
    }
}
```
and the only writer of that counter — verify_procurement_portal_validation.rs:276-277:
```
                stored.failed_verification_attempts =
                    stored.failed_verification_attempts.saturating_add(1);
```

The sibling handler the message names, which issues the fresh code and writes nothing else — services/service-com-amazonaws-invoicing-invoicing/src/ops/send_procurement_portal_validation.rs:225-228 and :251-255:
```
        // Issuing the code IS this operation's work, so this call writes the validation
        // state of the preference: the code it mints becomes the outstanding one and any
        // code an earlier send left is retired, which is what a client asking for a fresh
        // code gets.
```
```
        stored.validation = Some(crate::state::IssuedPortalValidation {
            code,
            issued_at,
            verified_at: None,
        });
```
— `stored.failed_verification_attempts` appears nowhere in this handler, so "what a client asking for a fresh code gets" is a fresh code with the previous code's budget still spent.

The counter's own definition, which ties it to what the refusal names — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:552-554:
```
    /// How many submissions of a validation code for this preference have been REFUSED:
    /// the failed attempts of `VerifyProcurementPortalValidation`, which its refusal
    /// names ("attempt N of the budget").
```

Test coverage: `tests/all/refused_procurement_portal_writes.rs:884-912` repeats submissions "past the attempt budget" against ONE issued code and asserts `invalid_code_sentence(attempt)` for attempt 1..=N, and `tests/all/procurement_portal_error_message_key.rs:569-583` pins that the count is per preference ("case another preference's first attempt: the count is per preference"). Neither sends a SECOND validation code after the budget is exhausted, and `tests/all/portal_validation_code_issue.rs:364-410` (`a_completed_validation_can_be_started_again`) re-sends but asserts only the issued code, never the attempt number — so nothing pins the behaviour this issue describes.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}]}`; note the returned `ProcurementPortalPreferenceArn`.
3. POST / with `X-Amz-Target: Invoicing.SendProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>"}`.
4. POST / with `X-Amz-Target: Invoicing.VerifyProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>","Code":"not-a-code"}` three times. Observe HTTP 400 with messages "Attempt 1 of 3 failed. Please try again.", "Attempt 2 of 3 failed. Please try again.", then "Attempt 3 of 3 failed. Request a new validation code with SendProcurementPortalValidation."
5. Follow that instruction: POST / with `X-Amz-Target: Invoicing.SendProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>"}` again. A fresh code is issued and the previous one is retired.
6. POST / with `X-Amz-Target: Invoicing.VerifyProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>","Code":"not-a-code"}` once more.
7. Observe HTTP 400 with message "The code you entered is invalid. Attempt 4 of 3 failed. Request a new validation code with SendProcurementPortalValidation." — the first failed submission against the NEW code is reported as attempt 4 of a 3-attempt budget, and the refusal repeats the remedy the caller just applied. Expected: the freshly issued code is refused as "Attempt 1 of 3 failed. Please try again.", so the number a client "reads the remaining attempts off" stays inside the budget it is stated against.
