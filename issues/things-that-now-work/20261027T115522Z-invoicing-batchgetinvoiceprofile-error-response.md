Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 16003bcc8481d509912583747e009ee504542034

# A refused input on the 11 implemented operations missing from FrontDoor drops the modelled reason and fieldList

_Filed: 2026-10-27T11:55:22Z_
_Operation: BatchGetInvoiceProfile | Class: error-response_

> _Note: this issue was identified against the code base at commit `16003bcc8481d509912583747e009ee504542034`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::service::FrontDoor` is what makes a boundary constraint refusal come back in this service's own modelled `ValidationException` (with `reason` and `fieldList`) instead of the framework's generic body, and its own doc states the rule for which operations it serves: "Only the operations this emulator IMPLEMENTS are served here". The list under that doc holds 9 entries (`[ServedOperation; 9]`): the five invoice-unit operations, `ListInvoiceSummaries`, and the three tag operations.

Eleven operations that this crate DOES implement are absent from it: `BatchGetInvoiceProfile` and the whole procurement-portal plane (`CreateProcurementPortalPreference`, `GetProcurementPortalPreference`, `PutProcurementPortalPreference`, `DeleteProcurementPortalPreference`, `UpdateProcurementPortalPreferenceStatus`, `SendProcurementPortalValidation`, `VerifyProcurementPortalValidation`, `ListProcurementPortalPreferences`, `ListProcurementPortals`, `ListProcurementPortalSuppliers`). A refused request for any of them therefore falls through to `Service::override_refusal`, which can only hand back a `Refused { code, message }` — so the body carries the message envelope alone, with `reason` null and no `fieldList`. That is exactly the client-visible defect `FrontDoor`'s own doc says it exists to remove ("A client deserializing that error gets a null `reason` and an empty field list, so it can neither branch on the reason nor report which member was rejected"), and the modelled `ValidationException` shape has both members, which `crate::service::validation::field_validation_failed` already fills for the nine served operations.

The same drift is written into the test suite: the two `BatchGetInvoiceProfile` rows in `validation_message_wording.rs` are introduced as "A SCAFFOLDED operation's refusal" and marked `.scaffolded()`, which asserts `fieldList` is ABSENT — but `BatchGetInvoiceProfile` is no longer a scaffold: its handler resolves linked accounts and computes `Profiles` from state. The behaviour is pinned by a reason that the handler itself contradicts.

Symptoms of one cause: every modelled-constraint refusal of those eleven operations (a bad `ProcurementPortalPreferenceArn` pattern, a `MaxResults` out of `@range`, an absent `@required` member, an account id outside `^\d{12}$`) answers without `reason`/`fieldList`, while the identical class of refusal on `CreateInvoiceUnit`/`GetInvoiceUnit`/`ListTagsForResource` answers with both.

## Evidence

Rule the code under it breaks — services/service-com-amazonaws-invoicing-invoicing/src/service.rs:154-157:
```
/// Only the operations this emulator IMPLEMENTS are served here. A scaffolded
/// operation answers HTTP 501 and asserts nothing about its behaviour, so restating
/// its refusals or its media type would claim a fidelity it does not have; each of
/// those is its own stub issue.
```
and the list that implements it — service.rs:167 `fn operations() -> [ServedOperation; 9] {` with entries naming only `CreateInvoiceUnit` (:171), `GetInvoiceUnit` (:182), `ListInvoiceUnits` (:193), `UpdateInvoiceUnit` (:204), `DeleteInvoiceUnit` (:215), `ListInvoiceSummaries` (:226), `ListTagsForResource` (:237), `TagResource` (:248), `UntagResource` (:259). No procurement-portal operation and no `BatchGetInvoiceProfile` entry exists.

Why the omission is observable — service.rs:135-147:
```
/// 1. A REFUSED INPUT is stated in this service's own modelled `ValidationException`
///    instead of the framework's generic refusal body. … A client deserializing that error gets a null `reason`
///    and an empty field list, so it can neither branch on the reason nor report which
///    member was rejected.
```
and the fallback seam those eleven operations actually reach — service.rs:414-425 `fn override_refusal(&self, refusal: &Refusal) -> Option<Refused>` → `validation::refused(refusal)`, whose return value carries two fields only (src/service/validation.rs:215-218):
```
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(&violations[..violations.len().min(MAX_VIOLATIONS)])?,
        }),
```
while the served path builds the full shape (src/service/validation.rs:175-205 `field_validation_failed` → `reason: Some(REASON_FIELD_VALIDATION_FAILED)`, `field_list: Some(...)`).

The operation is implemented, not a scaffold — src/ops/batch_get_invoice_profile.rs:157-208 (`handle` reads `store.linked_accounts()`, refuses an outsider 403 and maps `crate::state::invoice_profile` over the requested ids); no `//! SCAFFOLDED, NOT IMPLEMENTED:` header in the file.

The stale test reason — tests/all/validation_message_wording.rs:459-473:
```
        // A SCAFFOLDED operation's refusal is worded the same way: the framework
        // validates its modelled constraints before the 501, and this service words
        // what it refuses once.
        Case::new(
            "BatchGetInvoiceProfile",
            "an account id outside the account-id pattern",
…
        .scaffolded(),
```
and what `.scaffolded()` asserts, :534-539:
```
        if !case.modelled {
            // A scaffolded operation keeps the generic envelope: there are no modelled
            // members to carry the per-member detail in.
            assert!(
                answer.body.get("fieldList").is_none(),
```
Coverage for the modelled body exists only for the nine served operations — tests/all/wire_envelope_and_error_detail.rs:404-464 (`a_rejected_member_is_stated_as_a_field_validation_failure`, rows for `CreateInvoiceUnit`, `GetInvoiceUnit`, `UpdateInvoiceUnit`, `ListInvoiceUnits`, `ListTagsForResource`, `TagResource`) and :490-501 asserting `answer.member("reason")` and `answer.body["fieldList"]`; no procurement-portal or `BatchGetInvoiceProfile` row asserts either member.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.BatchGetInvoiceProfile` and body `{"AccountIds":["not-an-account"]}`.
3. Observe HTTP 400 `ValidationException` whose body carries `message` only: `reason` is absent (not `fieldValidationFailed`) and there is no `fieldList` entry naming `accountIds.1.member`.
4. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"COUPA","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":false,"PurchaseOrderRetrievalEnabled":false}` (the `@required` `Contacts` absent): same 400 with no `reason` and no `fieldList`.
5. POST / with `X-Amz-Target: Invoicing.CreateInvoiceUnit` and body `{"Name":"U","InvoiceReceiver":"not-an-account-id","TaxInheritanceDisabled":false,"Rule":{"LinkedAccounts":["210987654321"]}}` — the same constraint class answers with `reason: "fieldValidationFailed"` and a `fieldList` entry naming `invoiceReceiver`, which is the body the eleven unserved operations never produce.
