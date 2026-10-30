// SPDX-License-Identifier: CC0-1.0

//! The validity window a `taxDocumentAccessToken` has, end to end over the
//! restJson1 wire.
//!
//! The rule these tables assert is one: a token this service minted resolves
//! while it is inside the window the model publishes for it and is refused once
//! that window has closed, with the modelled `ValidationException` (HTTP 400)
//! carrying the published `ExpiredToken` code — whatever else about the
//! registration the token names is unchanged. The model states the window on the
//! member that carries the token (`smithy_query(query="shape",
//! shape="TaxDocumentMetadata")`, member `taxDocumentAccessToken`): "This token
//! is valid for 24 hours."
//!
//! The rows span what the rule generalises over: where in the window the call
//! falls (the window the write started, most of it still to run, its last
//! minutes, the instant it closes, and a second, an hour, a day and a week past
//! it), how the document is named (an uploaded file's name, an S3 object key
//! carrying the path separator) and how many documents one registration carries
//! (one against the five `VerificationDetails$taxRegistrationDocuments` admits,
//! whose tokens one write mints together), each asked with and without a
//! `destinationS3Location`. A second table pins which refusal a token gets when
//! more than one way of failing applies, and a third that the window a write
//! starts is the published 24 hours.
//!
//! Never a sleep: the instant a window closes is state, exactly as the instant a
//! TRN validation falls due is.
//!
//! The harness (`emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs` and the document readers (`store_documents`,
//! `get_document`, `reported_metadatas`, `assert_refused`) in
//! `tax_registration_document.rs`; nothing here builds a second one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_smithy_types::base64;
use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, ERROR_CODE_EXPIRED_TOKEN, ERROR_CODE_INVALID_TOKEN,
    ERROR_CODE_MALFORMED_TOKEN, TAX_DOCUMENT_TOKEN_VALIDITY,
};

use crate::tax_registration_document::{
    assert_refused, get_document, reported_metadatas, store_documents,
};

/// How this service words the refusal of a token that cannot be resolved,
/// whichever way it failed — the one sentence for the whole class, the published
/// `errorCode` telling the ways apart. Written out rather than read from the
/// service's own constant, so changing that wording cannot move both sides of an
/// assertion at once.
const TOKEN_REFUSED_MESSAGE: &str = "The specified token in the request is either malformed or \
                                     incorrect. Check your request and try again.";
use crate::tax_registration_lifecycle::{call, emulator_with_engine, make_validations_due};

/// Where in its validity window a call for a document falls.
#[derive(Clone, Copy, Debug)]
enum Window {
    /// The window the write itself started — the record is left exactly as the
    /// write stored it.
    AsWritten,
    /// This much of the window is still to run at the call.
    Open(Duration),
    /// The window closed this long before the call.
    Closed(Duration),
}

/// Put every stored registration's document-token window where `window` says,
/// the way the clock would have.
///
/// A monotonic instant as far back as a row asks for may not exist on a host
/// that booted more recently, in which case the earliest one it has stands in —
/// still a window that closed before the call, which is what such a row is
/// about.
fn set_token_window(engine: &Arc<engine::InMemoryEngine>, window: Window) {
    let now = Instant::now();
    let closes_at = match window {
        Window::AsWritten => return,
        Window::Open(remaining) => now + remaining,
        Window::Closed(ago) => now.checked_sub(ago).unwrap_or(now),
    };
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    for stored in store.registrations.values_mut() {
        stored.document_tokens_expire_at = Some(closes_at);
    }
}

/// The window rows: where in the window the call falls, and whether the token
/// still resolves there.
///
/// The published window is [`TAX_DOCUMENT_TOKEN_VALIDITY`] — the model's own 24
/// hours — so the rows carry both of its ends (the whole window still to run,
/// and the instant it closes) and spans past it.
fn window_rows() -> Vec<(&'static str, Window, bool)> {
    vec![
        ("the window the write started", Window::AsWritten, true),
        (
            "the whole published window still to run",
            Window::Open(TAX_DOCUMENT_TOKEN_VALIDITY),
            true,
        ),
        (
            "an hour of the window still to run",
            Window::Open(Duration::from_secs(3_600)),
            true,
        ),
        (
            "the last minutes of the window",
            Window::Open(Duration::from_secs(300)),
            true,
        ),
        (
            "the instant the window closes",
            Window::Closed(Duration::ZERO),
            false,
        ),
        (
            "a second past the window",
            Window::Closed(Duration::from_secs(1)),
            false,
        ),
        (
            "an hour past the window",
            Window::Closed(Duration::from_secs(3_600)),
            false,
        ),
        (
            "a further day past the window",
            Window::Closed(TAX_DOCUMENT_TOKEN_VALIDITY),
            false,
        ),
        (
            "a week past the window",
            Window::Closed(Duration::from_secs(7 * 24 * 60 * 60)),
            false,
        ),
    ]
}

/// The document shapes the rule holds over: how a `TaxRegistrationDocument`
/// names its document (an uploaded file's name, an S3 object key, which carries
/// the path separator) and how many of them one registration carries — one
/// against the five `VerificationDetails$taxRegistrationDocuments` admits, all
/// minted by the one write and so all inside the one window.
fn document_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one uploaded file",
            json!([{ "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } }]),
        ),
        (
            "one S3 object key",
            json!([{ "s3Location": { "bucket": "trn-docs", "key": "2026/de/vat.pdf" } }]),
        ),
        (
            "the five documents the list admits, of both shapes",
            json!([
                { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
                { "s3Location": { "bucket": "trn-docs", "key": "director-id.png" } },
                { "file": { "fileName": "articles.pdf", "fileContent": "d29ybGQ=" } },
                { "s3Location": { "bucket": "trn-docs", "key": "archive/2025/vat.pdf" } },
                { "file": { "fileName": "power-of-attorney.pdf", "fileContent": "YWdhaW4=" } },
            ]),
        ),
    ]
}

/// The destinations each row is asked with, so the window cannot depend on
/// whether the request asked for a download.
fn destinations() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no destination", None),
        (
            "a bucket and a prefix",
            Some(json!({ "bucket": "my-tax-documents", "prefix": "tax/2026" })),
        ),
    ]
}

/// RULE: a `taxDocumentAccessToken` resolves to its document while it is inside
/// the window the model publishes for it, and once that window has closed the
/// same metadata is refused `ExpiredToken` — the document it names being held
/// still, and the stored registration being left exactly as it was.
#[tokio::test]
async fn a_token_resolves_inside_its_window_and_is_refused_past_it() {
    for (document_case, documents) in document_rows() {
        for (window_case, window, resolves) in window_rows() {
            let (router, engine) = emulator_with_engine();
            let account_id = "000000000401";
            let metadatas = store_documents(&router, account_id, &documents).await;
            assert!(
                !metadatas.is_empty(),
                "case {document_case:?}: the stored registration must report its documents"
            );
            set_token_window(&engine, window);

            for (index, metadata) in metadatas.iter().enumerate() {
                for (destination_case, destination) in destinations() {
                    let case = format!(
                        "{document_case} / document {index} / {window_case} / {destination_case}"
                    );
                    let answer = get_document(&router, metadata, destination.as_ref()).await;
                    if resolves {
                        assert_eq!(
                            answer.status, 200,
                            "case {case:?}: a token inside its window must resolve, got {answer:?}"
                        );
                        let url = answer.get("presignedS3Url").as_str().unwrap_or_default();
                        assert!(
                            url.starts_with("https"),
                            "case {case:?}: expected the document's URL, got {url:?}"
                        );
                    } else {
                        let message = assert_refused(&case, &answer, ERROR_CODE_EXPIRED_TOKEN);
                        // The refusal states the one sentence this service
                        // words for the token class, written out here rather
                        // than read from the service's own constant so that
                        // changing that wording cannot move both sides of the
                        // assertion at once.
                        assert_eq!(
                            message, TOKEN_REFUSED_MESSAGE,
                            "case {case:?}: expected the refusal this service words for the \
                             class, got {:?}",
                            answer.body
                        );
                    }
                }
            }

            // The call changed nothing: the registration still reports exactly
            // the metadata it did, whichever side of the window it was asked on.
            assert_eq!(
                reported_metadatas(&router, account_id).await,
                metadatas,
                "case {document_case:?} / {window_case:?}: resolving or refusing a token must \
                 leave the stored metadata alone"
            );

            // And asking again is answered the same way, so no read consumed the
            // window.
            let again = get_document(&router, &metadatas[0], None).await;
            let expected_status = if resolves { 200 } else { 400 };
            assert_eq!(
                again.status, expected_status,
                "case {document_case:?} / {window_case:?}: the second read must be answered like \
                 the first, got {again:?}"
            );
        }
    }
}

/// RULE: the window a write starts is the one the model publishes — 24 hours
/// from that write — so the instant the record's tokens stop being valid is the
/// write instant plus that span and nothing else.
#[tokio::test]
async fn the_window_a_write_starts_is_the_published_twenty_four_hours() {
    // The span the model states on `TaxDocumentMetadata$taxDocumentAccessToken`:
    // "This token is valid for 24 hours."
    assert_eq!(
        TAX_DOCUMENT_TOKEN_VALIDITY,
        Duration::from_secs(24 * 60 * 60),
        "case \"the published span\": expected the 24 hours the model publishes, got {:?}",
        TAX_DOCUMENT_TOKEN_VALIDITY
    );

    let (router, engine) = emulator_with_engine();
    let account_id = "000000000402";
    let before = Instant::now();
    store_documents(
        &router,
        account_id,
        &json!([{ "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } }]),
    )
    .await;
    let after = Instant::now();

    let closes_at = {
        let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
        let store = state.lock();
        store
            .registrations
            .get(account_id)
            .expect("the write stored the registration")
            .document_tokens_expire_at
            .expect("the write recorded the window its tokens are valid for")
    };
    assert!(
        closes_at >= before + TAX_DOCUMENT_TOKEN_VALIDITY
            && closes_at <= after + TAX_DOCUMENT_TOKEN_VALIDITY,
        "case \"the window a write starts\": expected the write instant plus the published span, \
         got an instant {:?} past the call",
        closes_at.saturating_duration_since(before)
    );
}

/// RULE: each way a token can fail to resolve carries its own published code, so
/// a token that fails more than one way is refused for the one that is about the
/// token itself rather than for its age: text this service never minted is
/// `MalformedToken`, its own token naming nothing it holds is `InvalidToken`,
/// and only a token that still locates a held document is judged against its
/// window.
#[tokio::test]
async fn the_refusal_names_how_the_token_failed() {
    let (router, engine) = emulator_with_engine();
    let account_id = "000000000403";
    let metadatas = store_documents(
        &router,
        account_id,
        &json!([{ "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } }]),
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

    // Every row is asked with the window closed, so the code each is refused
    // with is the one its own way of failing publishes.
    set_token_window(&engine, Window::Closed(Duration::from_secs(1)));
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "text this service never minted",
            json!({ "taxDocumentAccessToken": "!!! not a token !!!", "taxDocumentName": "certificate.pdf" }),
            ERROR_CODE_MALFORMED_TOKEN,
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
            "an account this store holds no registration for",
            json!({
                "taxDocumentAccessToken": base64::encode("999999999999/1/0/certificate.pdf"),
                "taxDocumentName": "certificate.pdf",
            }),
            ERROR_CODE_INVALID_TOKEN,
        ),
        (
            "the held document's token under another document's name",
            json!({ "taxDocumentAccessToken": held_token, "taxDocumentName": "someone-elses.pdf" }),
            ERROR_CODE_INVALID_TOKEN,
        ),
        (
            "the held document's own metadata",
            held.clone(),
            ERROR_CODE_EXPIRED_TOKEN,
        ),
    ];
    for (case, metadata, expected_code) in &rows {
        let answer = get_document(&router, metadata, None).await;
        let message = assert_refused(case, &answer, expected_code);
        // Every way of failing is told the same sentence, so none of these
        // refusals echoes a submitted token back or varies with it.
        assert_eq!(
            message, TOKEN_REFUSED_MESSAGE,
            "case {case:?}: expected the refusal this service words for the class, got {:?}",
            answer.body
        );
    }

    // A rewrite mints tokens of its own, so the new write's window is its own:
    // the token it reports resolves even though the window of the record it
    // replaced had closed, and the token that record had is refused for being
    // retired rather than for its age.
    make_validations_due(&engine);
    let renewed = store_documents(
        &router,
        account_id,
        &json!([{ "s3Location": { "bucket": "trn-docs", "key": "renewed/certificate.pdf" } }]),
    )
    .await;
    let renewed_metadata = renewed
        .first()
        .cloned()
        .expect("the second revision reports its document");
    let answer = get_document(&router, &renewed_metadata, None).await;
    assert_eq!(
        answer.status, 200,
        "case \"the rewrite's own token\": a new write's token must resolve, got {answer:?}"
    );
    let retired = get_document(&router, &held, None).await;
    assert_refused(
        "the replaced revision's token, past its own window",
        &retired,
        ERROR_CODE_INVALID_TOKEN,
    );

    // The registration itself is untouched by any of it: the record still reads
    // back, reporting the current write's metadata.
    let got = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(
        got.status, 200,
        "case \"the registration after the refusals\": the record must still read back, got \
         {got:?}"
    );
    assert_eq!(
        reported_metadatas(&router, account_id).await,
        renewed,
        "case \"the registration after the refusals\": the read must report the current write's \
         metadata"
    );
}
