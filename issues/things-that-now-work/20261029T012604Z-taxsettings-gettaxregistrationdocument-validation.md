Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0b0649dc1b525c86072f831fd9c0b574c3cd2f61

# The documented 24-hour validity of taxDocumentAccessToken is never enforced, so an arbitrarily old token still resolves and ExpiredToken is unreachable

_Filed: 2026-10-29T01:26:04Z_
_Operation: GetTaxRegistrationDocument | Class: validation_

> _Note: this issue was identified against the code base at commit `0b0649dc1b525c86072f831fd9c0b574c3cd2f61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states two ways a `taxDocumentAccessToken` stops being valid: a rewrite of the registration, and age. The crate implements the first and cites the model for it, while the second is simply absent — `held_tax_document` compares the account, the registration revision, the token text and the document name, and nothing about when the token was minted. A token obtained days earlier from a registration that has not been rewritten is therefore still answered 200 with a download location.

As a consequence the model's `ValidationExceptionErrorCode` value `ExpiredToken` — which the generated value set in this crate carries, and which the crate's own tests list as one of the codes every refusal of this class must come from — can never be produced by any path in the service: `state.rs` defines constants for `MalformedToken`, `InvalidToken`, `FieldValidationFailed`, `MissingInput`, `NonIndiaCustomerCanNotSetPAN` and `GSTExistenceBlockSetPAN`, and none for `ExpiredToken`.

Nothing in the crate records when a token was minted: `StoredTaxRegistration` holds `validation_due_at` (an `Instant`, cleared once the TRN validation settles) and `revision`, and no wall-clock instant for the registration write, so there is nothing for the age check to read.

## Evidence

Spec sentence — `smithy_query(query="shape", shape="TaxDocumentMetadata")`, member `taxDocumentAccessToken`: "If you update your tax registration, the existing `taxDocumentAccessToken` won't be valid. To get the latest token, call the `GetTaxRegistration` or `ListTaxRegistrations` API operation. **This token is valid for 24 hours.**"

The crate claims to implement that invalidation — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:670-679` (doc of `held_tax_document`): "the token locates one document of ONE revision of ONE account's registration, so a token minted for a registration that has since been rewritten (a later revision) or deleted names nothing — the invalidation the model documents for `taxDocumentAccessToken`."

The check it actually performs — `src/state.rs:688-702`:
```
        let named = decode_document_access_token(&metadata.tax_document_access_token)?;
        let held = self
            .registrations
            .get(&named.account_id)
            .filter(|stored| stored.revision == named.revision)
            .and_then(|stored| stored.tax_document_metadatas.get(named.index))
            .is_some_and(|document| {
                document.tax_document_access_token == metadata.tax_document_access_token
                    && document.tax_document_name == metadata.tax_document_name
            });
```
No term involving time. The token's own payload carries no timestamp either — `src/state.rs:1405-1407`:
```
fn document_access_token(account_id: &str, revision: u64, index: usize, name: &str) -> String {
    base64::encode(format!("{account_id}/{revision}/{index}/{name}"))
}
```
and `StoredTaxRegistration` (`src/state.rs:1034-1071`) stores no wall-clock write time.

`ExoiredToken` unreachable — `src/state.rs:330-348` declares every `ValidationExceptionErrorCode` constant this service uses and the enum comment names the full published set ("`MalformedToken | ExpiredToken | InvalidToken | FieldValidationFailed | MissingInput | NonIndiaCustomerCanNotSetPAN | GSTExistenceBlockSetPAN`") while no constant or call site exists for `ExpiredToken`; the generated set is in `src/ops/types/shared_types.rs:495`. The handler's two refusals are only `MalformedToken` (`src/state.rs:1457-1464`) and `InvalidToken` (`src/state.rs:1488-1494`), and `src/state.rs:1431-1433` says "both refusals below state it and the published `errorCode` is what tells the two apart" — the third one the model publishes is not among them.

Test coverage: `tests/all/tax_registration_document.rs:41-53` lists `"ExpiredToken"` among `PUBLISHED_ERROR_CODES` ("every refusal below must carry one of them") but no row ages a token, and `tests/all/tax_registration_document.rs:9-13` enumerates only the malformed/no-longer-locates cases — so no test pins a token staying valid forever, and no comment in the crate states a reason for omitting the 24-hour window.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE136695976","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"},"verificationDetails":{"taxRegistrationDocuments":[{"file":{"fileName":"cert.pdf","fileContent":"<base64>"}}]}}}.
2. POST /GetTaxRegistration with {"accountId":"000000000001"} and keep taxRegistration.taxDocumentMetadatas[0].
3. Leave the registration untouched for more than 24 hours (or advance the emulator clock past that window).
4. POST /GetTaxRegistrationDocument with {"taxDocumentMetadata": <the kept metadata>}. Observed: 200 with a presignedS3Url. Expected per the modelled sentence "This token is valid for 24 hours": the token is no longer valid and the call is refused 400 ValidationException with the published ExpiredToken code — a code no path in this crate can produce today.
