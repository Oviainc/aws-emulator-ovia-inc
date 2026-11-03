// SPDX-License-Identifier: CC0-1.0

//! `GetTaxRegistrationDocument`: resolving a tax document access token against
//! the registration documents this service holds, end to end over the restJson1
//! wire.
//!
//! The rule these tables assert is one: a `taxDocumentMetadata` is answered with
//! the download location of the document it names in the registration documents
//! this service holds, and metadata that names none of them is refused with the
//! modelled `ValidationException` (HTTP 400) carrying the published `errorCode`
//! for how the token failed — `MalformedToken` for text this service never
//! minted, `InvalidToken` for a token of its own that no longer locates a
//! document.
//!
//! The rows span the classes the rule generalises over rather than the one
//! recorded request: both shapes a `TaxRegistrationDocument` names its document
//! by (an uploaded file's name, an S3 object key), one document against the five
//! `VerificationDetails$taxRegistrationDocuments` admits, document names
//! carrying the path separator, non-ASCII names and a name far longer than the
//! 200-character cap the model puts on the reported URL, every form of
//! `destinationS3Location` (absent, a bucket alone, a bucket with a prefix, the
//! same prefix spelled with its trailing separator, the same prefix under a
//! different bucket), and each way a token can fail to name a held document.
//!
//! The harness (`emulator`, `call`, `field_list`, `make_validations_due`) lives
//! in `tax_registration_lifecycle.rs`; nothing here builds a second one.

use std::collections::BTreeSet;

use aws_smithy_types::base64;
use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ERROR_CODE_INVALID_TOKEN, ERROR_CODE_MALFORMED_TOKEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_registration_lifecycle::{
    call, emulator, emulator_with_engine, field_list, make_validations_due, Answer,
};

/// The `errorCode` values the model publishes for this service's
/// `ValidationException` (`smithy_query(query="shape",
/// shape="ValidationExceptionErrorCode")`): every refusal below must carry one
/// of them.
const PUBLISHED_ERROR_CODES: &[&str] = &[
    "MalformedToken",
    "ExpiredToken",
    "InvalidToken",
    "FieldValidationFailed",
    "MissingInput",
    "NonIndiaCustomerCanNotSetPAN",
    "GSTExistenceBlockSetPAN",
];

/// How this service words the refusal of a token that names no held document,
/// whichever way the token failed: one sentence for the whole class, naming no
/// value from the request. Written out here rather than read from the service's
/// own constant, so changing that wording cannot move both sides of the
/// assertion at once.
const TOKEN_REFUSED_MESSAGE: &str = "The specified token in the request is either malformed or \
                                     incorrect. Check your request and try again.";

/// Store a tax registration for `account_id` carrying `documents`, and answer
/// the `TaxDocumentMetadata` entries a read of that registration reports.
///
/// A plain German VAT entry, because the rule under test is about the DOCUMENTS
/// a registration carries and not about the jurisdiction it is held in; the
/// per-country requirements have their own suites.
pub(crate) async fn store_documents(
    router: &MultiServiceRouter,
    account_id: &str,
    documents: &Value,
) -> Vec<Value> {
    let put = call(
        router,
        "PutTaxRegistration",
        &json!({
            "accountId": account_id,
            "taxRegistrationEntry": {
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
                "verificationDetails": { "taxRegistrationDocuments": documents },
            },
        }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "setup write for {account_id} failed: {put:?}"
    );
    reported_metadatas(router, account_id).await
}

/// The `taxDocumentMetadatas` a read of `account_id`'s registration reports —
/// where a caller gets the metadata it submits here, as the model's note on
/// `taxDocumentAccessToken` says.
pub(crate) async fn reported_metadatas(router: &MultiServiceRouter, account_id: &str) -> Vec<Value> {
    let got = call(
        router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(got.status, 200, "reading {account_id} failed: {got:?}");
    got.body
        .pointer("/taxRegistration/taxDocumentMetadatas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Ask for one document's download location.
pub(crate) async fn get_document(
    router: &MultiServiceRouter,
    metadata: &Value,
    destination: Option<&Value>,
) -> Answer {
    let mut body = json!({ "taxDocumentMetadata": metadata });
    if let Some(destination) = destination {
        body["destinationS3Location"] = destination.clone();
    }
    call(router, "GetTaxRegistrationDocument", &body).await
}

/// One registration's documents: the account to store them under, the
/// `taxRegistrationDocuments` list, and the name each document is expected to
/// be known by.
struct DocumentRow {
    name: &'static str,
    account_id: &'static str,
    documents: Value,
    document_names: Vec<String>,
}

/// The document shapes the rule holds over.
///
/// A `TaxRegistrationDocument` names its document either by an uploaded file's
/// name or by its S3 object key (`smithy_query(query="shape",
/// shape="TaxRegistrationDocument")`), and
/// `VerificationDetails$taxRegistrationDocuments` admits one to five of them.
/// The rows differ in that shape, in how many documents one registration
/// carries, and in the names themselves: a plain file name, an object key
/// carrying the path separator, a non-ASCII name, and a name far longer than the
/// 200 characters the model allows the reported URL.
fn document_rows() -> Vec<DocumentRow> {
    let long_name = format!("{}.pdf", "vat-certificate-page-".repeat(20));
    vec![
        DocumentRow {
            name: "one uploaded file",
            account_id: "000000000101",
            documents: json!([
                { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
            ]),
            document_names: vec!["certificate.pdf".to_string()],
        },
        DocumentRow {
            name: "one S3 object key carrying the path separator",
            account_id: "000000000102",
            documents: json!([
                { "s3Location": { "bucket": "trn-docs", "key": "2026/de/vat-certificate.pdf" } },
            ]),
            document_names: vec!["2026/de/vat-certificate.pdf".to_string()],
        },
        DocumentRow {
            name: "the five documents the list admits, of both shapes",
            account_id: "000000000103",
            documents: json!([
                { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
                { "s3Location": { "bucket": "trn-docs", "key": "director-id.png" } },
                { "file": { "fileName": "articles.pdf", "fileContent": "d29ybGQ=" } },
                { "s3Location": { "bucket": "trn-docs", "key": "archive/2025/vat.pdf" } },
                { "file": { "fileName": "power-of-attorney.pdf", "fileContent": "YWdhaW4=" } },
            ]),
            document_names: vec![
                "certificate.pdf".to_string(),
                "director-id.png".to_string(),
                "articles.pdf".to_string(),
                "archive/2025/vat.pdf".to_string(),
                "power-of-attorney.pdf".to_string(),
            ],
        },
        DocumentRow {
            name: "a non-ASCII document name",
            account_id: "000000000104",
            documents: json!([
                { "file": { "fileName": "Umsatzsteuer-Zertifikat-Ö.pdf", "fileContent": "aGVsbG8=" } },
            ]),
            document_names: vec!["Umsatzsteuer-Zertifikat-Ö.pdf".to_string()],
        },
        DocumentRow {
            name: "a document name longer than the reported URL may be",
            account_id: "000000000105",
            documents: json!([
                { "file": { "fileName": long_name, "fileContent": "aGVsbG8=" } },
            ]),
            document_names: vec![long_name],
        },
    ]
}

/// The destinations the rule holds over: `destinationS3Location` is optional
/// (`smithy_query(query="operation", operation="GetTaxRegistrationDocument")`),
/// its `bucket` is "the name of your Amazon S3 bucket that you specify to
/// download your tax documents to" and its `prefix` is "the Amazon S3 object
/// prefix that you specify for your tax document file", so the rows are no
/// destination at all, a bucket alone, a bucket with a prefix, the same prefix
/// spelled with its trailing separator — which names the same place in the
/// bucket — and the same prefix under a different bucket.
///
/// The third element is the prefix the reported path is expected to carry:
/// `None` with a destination means the path is the bucket and the document's
/// name alone, and no destination at all means no path is reported.
fn destination_rows() -> Vec<(&'static str, Option<Value>, Option<&'static str>)> {
    vec![
        ("no destination", None, None),
        (
            "a bucket alone",
            Some(json!({ "bucket": "my-tax-documents" })),
            None,
        ),
        (
            "a bucket and a prefix",
            Some(json!({ "bucket": "my-tax-documents", "prefix": "tax/2026" })),
            Some("tax/2026"),
        ),
        (
            "the same prefix spelled with its trailing separator",
            Some(json!({ "bucket": "my-tax-documents", "prefix": "tax/2026/" })),
            Some("tax/2026"),
        ),
        (
            "the same prefix under a different bucket",
            Some(json!({ "bucket": "somewhere-else-entirely", "prefix": "tax/2026" })),
            Some("tax/2026"),
        ),
    ]
}

/// RULE: the metadata a stored registration reports for one of its documents is
/// answered with that document's download location — a URL within the modelled
/// `Url` constraints naming the document's own object, and, when the request
/// names a destination, the place in THAT location (its bucket, then the key
/// under its prefix) the document is asked to be downloaded to.
///
/// Asserted over every document shape and every destination form, and the same
/// question asked twice: a read reports what it reported before and leaves the
/// stored metadata alone.
#[tokio::test]
async fn a_held_documents_metadata_resolves_to_its_download_location() {
    let router = emulator();
    // Every URL this test is answered, so no two documents can share one.
    let mut urls: BTreeSet<String> = BTreeSet::new();

    for row in document_rows() {
        let metadatas = store_documents(&router, row.account_id, &row.documents).await;
        assert_eq!(
            metadatas.len(),
            row.document_names.len(),
            "case {:?}: expected one metadata per submitted document, got {metadatas:?}",
            row.name
        );

        for (index, metadata) in metadatas.iter().enumerate() {
            let document_name = metadata
                .get("taxDocumentName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                document_name, row.document_names[index],
                "case {:?}: expected document {index} to be named {:?}, got {metadata:?}",
                row.name, row.document_names[index]
            );
            // The form this service reports a held document's object under,
            // written out rather than computed from the handler's own helper, so
            // a bug there cannot move both sides of the assertion at once: the
            // account's tax-document bucket, then the registration revision (the
            // first write's, which is 1) and the document's position in it.
            let expected_url = format!(
                "https://aws-tax-documents-{}.s3.amazonaws.com/1/{index}",
                row.account_id
            );

            for (destination_case, destination, expected_prefix) in destination_rows() {
                let case = format!("{} / document {index} / {destination_case}", row.name);
                let answer = get_document(&router, metadata, destination.as_ref()).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected 200, got {answer:?}"
                );

                let url = answer
                    .get("presignedS3Url")
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    url, expected_url,
                    "case {case:?}: expected the document's own URL {expected_url:?}, got {:?}",
                    answer.body
                );
                // `Url` is `@length 1..200` with pattern `^https.*\S.*$`
                // (`smithy_query(query="operation",
                // operation="GetTaxRegistrationDocument")`), so the reported URL
                // stays inside those bounds however the document is named.
                assert!(
                    url.starts_with("https") && url.chars().any(|c| !c.is_whitespace()),
                    "case {case:?}: expected an https URL, got {url:?}"
                );
                assert!(
                    (1..=200).contains(&url.chars().count()),
                    "case {case:?}: expected a URL within the modelled 1..200 characters, got {} \
                     characters: {url:?}",
                    url.chars().count()
                );
                urls.insert(url);

                let reported_path = answer.get("destinationFilePath").as_str().map(str::to_string);
                // The path names the whole location the request gave: the bucket
                // it asked the document to be downloaded to, then the key under
                // the prefix. Written out rather than computed from the
                // handler's own helper, for the reason above.
                let expected_path = destination.as_ref().map(|destination| {
                    let bucket = destination
                        .get("bucket")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    match expected_prefix {
                        Some(prefix) => format!("s3://{bucket}/{prefix}/{document_name}"),
                        None => format!("s3://{bucket}/{document_name}"),
                    }
                });
                assert_eq!(
                    reported_path, expected_path,
                    "case {case:?}: expected destinationFilePath {expected_path:?}, got {:?}",
                    answer.body
                );
            }
        }

        // The reads changed nothing: the registration still reports exactly the
        // metadata it did before they ran.
        let after = reported_metadatas(&router, row.account_id).await;
        assert_eq!(
            after, metadatas,
            "case {:?}: reading a document must leave the stored metadata alone",
            row.name
        );
    }

    let documents: usize = document_rows()
        .iter()
        .map(|row| row.document_names.len())
        .sum();
    assert_eq!(
        urls.len(),
        documents,
        "each held document must have its own URL, got {urls:?}"
    );
}

/// Assert one answer is the refusal a metadata naming no held document gets:
/// HTTP 400, `ValidationException`, one of the published `errorCode` values (the
/// expected one), and a `fieldList` naming the access-token parameter. Answers
/// the message, which the caller checks does not vary with the token asked
/// about.
pub(crate) fn assert_refused(case: &str, answer: &Answer, expected_code: &str) -> String {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {answer:?}"
    );
    let code = answer.get("errorCode").as_str().unwrap_or_default();
    assert!(
        PUBLISHED_ERROR_CODES.contains(&code),
        "case {case:?}: expected a published errorCode, got {code:?}"
    );
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected errorCode {expected_code:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        field_list(answer),
        // The name this service reports the refused parameter under. OBSERVED,
        // and not the CONSTANT_CASE of the member carrying the token: the
        // recorded unit
        // `taxsettings.get-tax-registration-document-invalid-token-rejected`
        // names `TAX_DOC_ACCESS_TOKEN`. Written out rather than computed, for
        // the reason above.
        vec!["TAX_DOC_ACCESS_TOKEN".to_string()],
        "case {case:?}: the refusal must name the member it is about, got {:?}",
        answer.body
    );
    let message = answer
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, TOKEN_REFUSED_MESSAGE,
        "case {case:?}: expected the refusal this service words for the class, got {:?}",
        answer.body
    );
    message
}

/// RULE: a `taxDocumentMetadata` that names no document this service holds is
/// refused — `MalformedToken` for text this service never minted, `InvalidToken`
/// for a token of its own that locates nothing — and the refused read leaves the
/// stored registration exactly as it was.
#[tokio::test]
async fn metadata_that_names_no_held_document_is_refused() {
    let router = emulator();
    let account_id = "000000000201";
    let metadatas = store_documents(
        &router,
        account_id,
        &json!([
            { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
        ]),
    )
    .await;
    let held = metadatas
        .first()
        .cloned()
        .expect("the stored registration reports its document");
    let held_token = held
        .get("taxDocumentAccessToken")
        .and_then(Value::as_str)
        .expect("the metadata carries an access token")
        .to_string();

    // Each row is a metadata that names no held document, and the published code
    // it is refused with. The malformed rows are texts this service never
    // minted; the invalid rows are its own form naming nothing it holds.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "text that is not an encoded token at all",
            json!({ "taxDocumentAccessToken": "!!! not a token !!!", "taxDocumentName": "certificate.pdf" }),
            ERROR_CODE_MALFORMED_TOKEN,
        ),
        (
            "the empty token",
            json!({ "taxDocumentAccessToken": "", "taxDocumentName": "certificate.pdf" }),
            ERROR_CODE_MALFORMED_TOKEN,
        ),
        (
            "an encoded text carrying no coordinates",
            json!({
                "taxDocumentAccessToken": base64::encode("hello"),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_MALFORMED_TOKEN,
        ),
        (
            "an encoded text that stops before the document name",
            json!({
                "taxDocumentAccessToken": base64::encode(format!("{account_id}/1/0")),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_MALFORMED_TOKEN,
        ),
        (
            "an encoded text whose revision is not a number",
            json!({
                "taxDocumentAccessToken": base64::encode(format!("{account_id}/one/0/certificate.pdf")),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_MALFORMED_TOKEN,
        ),
        (
            "an encoded text whose position is not a number",
            json!({
                "taxDocumentAccessToken": base64::encode(format!("{account_id}/1/last/certificate.pdf")),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_MALFORMED_TOKEN,
        ),
        (
            "an account this store holds no registration for",
            json!({
                "taxDocumentAccessToken": base64::encode("999999999999/1/0/certificate.pdf"),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_INVALID_TOKEN,
        ),
        (
            "a revision the stored registration is not at",
            json!({
                "taxDocumentAccessToken": base64::encode(format!("{account_id}/9/0/certificate.pdf")),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_INVALID_TOKEN,
        ),
        (
            "a position past the documents the registration carries",
            json!({
                "taxDocumentAccessToken": base64::encode(format!("{account_id}/1/4/certificate.pdf")),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_INVALID_TOKEN,
        ),
        (
            "the held document's token under a different document name",
            json!({ "taxDocumentAccessToken": held_token, "taxDocumentName": "someone-elses.pdf" }),
            ERROR_CODE_INVALID_TOKEN,
        ),
    ];

    // The refusal is about the member, not about the value asked for, so one
    // message per published code is all these rows may produce — which also
    // means none of them echoes a submitted token back.
    let mut messages: BTreeSet<(String, String)> = BTreeSet::new();
    for (case, metadata, expected_code) in &rows {
        // Both destination forms, so a refusal cannot depend on whether the
        // request asked for a download.
        for (destination_case, destination, _) in destination_rows() {
            let case = format!("{case} / {destination_case}");
            let answer = get_document(&router, metadata, destination.as_ref()).await;
            let message = assert_refused(&case, &answer, expected_code);
            messages.insert(((*expected_code).to_string(), message));
        }
        // And the refused read left the registration alone: its metadata still
        // reads back, and the held document still resolves.
        assert_eq!(
            reported_metadatas(&router, account_id).await,
            metadatas,
            "case {case:?}: a refused read must leave the stored metadata alone"
        );
        let still = get_document(&router, &held, None).await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the held document must still resolve, got {still:?}"
        );
    }
    assert_eq!(
        messages.len(),
        2,
        "the refusal must not vary with the token asked about, got {messages:?}"
    );
}

/// RULE: a token names a document of ONE revision of ONE account's registration,
/// so rewriting that registration retires it — the write's new metadata resolves
/// and the previous token is refused — and deleting the registration retires
/// every token it had.
#[tokio::test]
async fn rewriting_or_deleting_a_registration_retires_its_tokens() {
    let (router, engine) = emulator_with_engine();
    let account_id = "000000000301";
    let first = store_documents(
        &router,
        account_id,
        &json!([
            { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
        ]),
    )
    .await;
    let first_metadata = first
        .first()
        .cloned()
        .expect("the first revision reports its document");
    let first_answer = get_document(&router, &first_metadata, None).await;
    assert_eq!(
        first_answer.status, 200,
        "the first revision's document must resolve, got {first_answer:?}"
    );
    let first_url = first_answer
        .get("presignedS3Url")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A second write needs the first registration's validation to have fallen
    // due — an outstanding TRN may not be replaced — which is state, never a
    // sleep.
    make_validations_due(&engine);
    let second = store_documents(
        &router,
        account_id,
        &json!([
            { "s3Location": { "bucket": "trn-docs", "key": "renewed/certificate.pdf" } },
        ]),
    )
    .await;
    let second_metadata = second
        .first()
        .cloned()
        .expect("the second revision reports its document");
    assert_ne!(
        second_metadata.get("taxDocumentAccessToken"),
        first_metadata.get("taxDocumentAccessToken"),
        "rewriting the registration must mint a new access token, got {second_metadata:?}"
    );

    let retired = get_document(&router, &first_metadata, None).await;
    assert_refused(
        "the previous revision's token",
        &retired,
        ERROR_CODE_INVALID_TOKEN,
    );

    let renewed = get_document(&router, &second_metadata, None).await;
    assert_eq!(
        renewed.status, 200,
        "the current revision's document must resolve, got {renewed:?}"
    );
    let renewed_url = renewed
        .get("presignedS3Url")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        renewed_url, first_url,
        "a new revision's document must have its own URL, got {renewed_url:?}"
    );

    let deleted = call(
        &router,
        "DeleteTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "the setup delete must succeed, got {deleted:?}"
    );
    for (case, metadata) in [
        ("the deleted registration's current token", &second_metadata),
        ("its retired token", &first_metadata),
    ] {
        let answer = get_document(&router, metadata, None).await;
        assert_refused(case, &answer, ERROR_CODE_INVALID_TOKEN);
    }
}
