Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6946203d9f2a769569cfa156ac7c42bcf6c300bc

# A tax document access token retired by DeleteTaxRegistration resolves again after the account is re-registered, because the revision counter restarts at 1

_Filed: 2026-10-29T08:31:09Z_
_Operation: GetTaxRegistrationDocument | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6946203d9f2a769569cfa156ac7c42bcf6c300bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredTaxRegistration::revision` is the only thing that retires a `taxDocumentAccessToken`: the token encodes `{accountId}/{revision}/{index}/{name}` and `held_tax_document` accepts it only when the stored record's revision equals the one the token carries. But the revision is read exclusively from the record that is still in the store — `self.registrations.get_mut(account_id)` — so `DeleteTaxRegistration` (and `BatchDeleteTaxRegistration`), which REMOVE the map entry, also discard the counter. The next `PutTaxRegistration` for that account therefore starts again at revision 1 and re-mints byte-for-byte the same access tokens the deleted registration had.

Consequence: a token minted for the FIRST registration of an account, which the crate (and the model) say stopped naming anything when that registration was deleted, starts resolving again as soon as the account is re-registered with a document of the same name at the same position — `GetTaxRegistrationDocument` answers 200 with a `presignedS3Url` for a document of a DIFFERENT registration instead of the `InvalidToken` ValidationException the crate raises for a token that no longer locates a document. The same reset also makes `GetTaxRegistration` report, for the new registration, tokens identical to the retired ones, so a client cannot tell the two registrations' documents apart.

The root cause is in `state.rs::store_registration`; the wrong ANSWER is served by `GetTaxRegistrationDocument`, the one operation that resolves a token.

## Evidence

Model (`smithy_query(query="shape", shape="TaxDocumentMetadata")`, member `taxDocumentAccessToken`): "If you update your tax registration, the existing `taxDocumentAccessToken` won't be valid. To get the latest token, call the `GetTaxRegistration` or `ListTaxRegistrations` API operation."

The crate states the same rule and claims deletion is covered — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:673-677`:
```
/// The metadata must name a document this service HOLDS: the token locates
/// one document of ONE revision of ONE account's registration, so a token
/// minted for a registration that has since been rewritten (a later
/// revision) or deleted names nothing — the invalidation the model documents
/// for `taxDocumentAccessToken`.
```
and `src/state.rs:1063-1066`:
```
/// How many times `PutTaxRegistration` has written this account's TRN. The
/// document access tokens below are derived from it, because the model
/// documents that updating a registration invalidates the previous tokens.
```

But the counter lives only inside the stored record and the revision of a NEW record is always 1 — `src/state.rs:646-666`:
```
let previous_revision = match self.registrations.get_mut(account_id) {
    Some(stored) => { ... Some(stored.revision) }
    None => None,
};
...
let revision = previous_revision.map_or(1, |revision| revision.saturating_add(1));
```
and the delete path destroys that record outright — `src/ops/delete_tax_registration.rs:172`: `store.registrations.remove(&account_id);` (same in `src/ops/batch_delete_tax_registration.rs:191`). So after a delete the account's next write is revision 1 again, and `document_access_token(account_id, revision, index, name)` (`src/state.rs:1405-1407`) produces the identical string, which `held_tax_document`'s check accepts (`src/state.rs:689-697`: `.filter(|stored| stored.revision == named.revision)` plus whole-token equality).

Test coverage: `tests/all/tax_registration_document.rs:528-531` states the rule but never re-registers after the delete —
```
/// RULE: a token names a document of ONE revision of ONE account's registration,
/// so rewriting that registration retires it — the write's new metadata resolves
/// and the previous token is refused — and deleting the registration retires
/// every token it had.
```
Its delete case (lines 603-619) asserts the refusal only while the account holds NO registration, so the resurrection after a re-register is untested.

## Steps to Reproduce

1. PutTaxRegistration for accountId 000000000301 with `verificationDetails.taxRegistrationDocuments = [{ "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } }]`.
2. GetTaxRegistration for that account and save the single `taxDocumentMetadatas[0]` (token T1, name "certificate.pdf"). GetTaxRegistrationDocument with T1 answers 200.
3. Let the TRN validation fall due (or use the suite's `make_validations_due`), then call DeleteTaxRegistration for that account — 200, the record is removed. GetTaxRegistrationDocument with T1 is now refused `ValidationException` / `InvalidToken`, as the crate's rule says.
4. PutTaxRegistration again for the same account with the same document file name "certificate.pdf".
5. Call GetTaxRegistrationDocument with the OLD metadata T1: expected the same `InvalidToken` refusal (the token belonged to a registration that was deleted), actual HTTP 200 with a `presignedS3Url`, because the new record was stored with revision 1 and minted the identical token.
6. Also compare `GetTaxRegistration`'s reported `taxDocumentMetadatas[0].taxDocumentAccessToken` before the delete and after the re-register: they are equal, though `revision` is documented as "how many times PutTaxRegistration has written this account's TRN" (two writes).
