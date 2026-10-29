Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 4373217f8f4cfab6c9129dcbd57530e9c7b55ce8

# GetTaxRegistrationDocument drops destinationS3Location.bucket, so the response is identical whatever bucket the caller names

_Filed: 2026-10-29T06:00:29Z_
_Operation: GetTaxRegistrationDocument | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `4373217f8f4cfab6c9129dcbd57530e9c7b55ce8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DestinationS3Location$bucket` is the one `@required` member of the location a request names, and the model states what it is for: the operation "Downloads your tax documents to the Amazon S3 bucket that you specify in your request", the member is "The name of your Amazon S3 bucket that you specify to download your tax documents to", and the response member derived from it is "The file path of the Amazon S3 bucket where you want to download your tax document to".

The handler never reads that member. `destinationFilePath` is built from `prefix` and the document's own name only (`destination_file_path`), and `presignedS3Url` is built from the SERVICE's own bucket (`crate::state::document_bucket`, `aws-tax-documents-{accountId}`), which the crate documents as "never in the caller's bucket". A grep of the whole crate shows `bucket` is referenced only in the generated input struct and in the service's own `document_bucket` helper — the submitted value is never read, never stored and never echoed.

Consequence: two requests that differ only in `destinationS3Location.bucket` are answered byte-identically, so the caller cannot tell from the response which bucket (if any) was honoured, and the documented delivery "to the Amazon S3 bucket that you specify" leaves no trace anywhere in the service's state or in its answer. The one member the model makes mandatory for naming a destination is inert, while the optional `prefix` beside it IS honoured — so the handler half-implements the location it was given.

## Evidence

Model (smithy_query query="doc" shape="GetTaxRegistrationDocument"):
> "Downloads your tax documents to the Amazon S3 bucket that you specify in your request."

Model (smithy_query query="shape" shape="DestinationS3Location"):
> "bucket": {"target": "S3BucketName", "required": true, ... "doc": "<p>The name of your Amazon S3 bucket that you specify to download your tax documents to.</p>"}

Model (`GetTaxRegistrationDocumentResponse$destinationFilePath`):
> "The file path of the Amazon S3 bucket where you want to download your tax document to."

The member exists on the deserialized input — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/types/get_tax_registration_document_types.rs:21-26`:
```
#[smithy_macros::smithy(shape = "com.amazonaws.taxsettings#DestinationS3Location")]
pub struct DestinationS3Location {
    pub bucket: String,
    pub prefix: Option<String>,
}
```

The handler reads only `prefix` — `src/ops/get_tax_registration_document.rs:178-185`:
```
        Ok(GetTaxRegistrationDocumentOutput {
            destination_file_path: input
                .destination_s3_location
                .as_ref()
                .map(|destination| destination_file_path(destination, &document.name)),
            presigned_s3_url: Some(crate::state::document_download_url(&document)),
        })
```
and `src/ops/get_tax_registration_document.rs:196-206`:
```
fn destination_file_path(destination: &DestinationS3Location, name: &str) -> String {
    match destination
        .prefix
        .as_deref()
        .map(|prefix| prefix.trim_end_matches('/'))
        .filter(|prefix| !prefix.is_empty())
    {
        Some(prefix) => format!("{prefix}/{name}"),
        None => name.to_string(),
    }
}
```
`bucket` appears nowhere in that path. The URL that IS reported names the service's own bucket instead — `src/state.rs:1496-1505`:
```
/// The Amazon S3 bucket a reader reports an account's tax documents from.
/// ... they live in a bucket of its own named from
/// the account they belong to — never in the caller's bucket, which a request
/// names only to be downloaded INTO.
pub fn document_bucket(account_id: &str) -> String {
    format!("aws-tax-documents-{account_id}")
}
```
and `src/state.rs:1521-1528` builds `presignedS3Url` from `document_bucket(&document.account_id)`.

The handler's own header asserts the reported path is in the caller's bucket, which the computed value does not state — `src/ops/get_tax_registration_document.rs:13-17`:
```
//! * `destinationFilePath` — reported only when the request names a
//!   `destinationS3Location`: the object key the document occupies in THAT
//!   bucket, which is the file path the request asked it to be downloaded to.
```

Test coverage: `tests/all/tax_registration_document.rs` exercises this operation (token refusals at :347-364, published `errorCode` set at :41) but no test varies `destinationS3Location.bucket` or asserts anything about it, so nothing pins the current behaviour with a stated reason.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE136695976","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"},"verificationDetails":{"taxRegistrationDocuments":[{"file":{"fileName":"trn.pdf","fileContent":"aGVsbG8="}}]}}}
2. POST /GetTaxRegistration with {"accountId":"000000000001"} and copy taxRegistration.taxDocumentMetadatas[0]
3. POST /GetTaxRegistrationDocument with {"taxDocumentMetadata":<that metadata>,"destinationS3Location":{"bucket":"my-first-bucket","prefix":"tax/2024"}} — note destinationFilePath is "tax/2024/trn.pdf" and presignedS3Url points at aws-tax-documents-000000000001
4. Repeat the same call with {"bucket":"a-completely-different-bucket","prefix":"tax/2024"} — the response body is identical: the required bucket member changed nothing, and no state records that a download to either bucket was requested
