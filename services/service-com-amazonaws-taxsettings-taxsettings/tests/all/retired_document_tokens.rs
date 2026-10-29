// SPDX-License-Identifier: CC0-1.0

//! What becomes of a tax document access token once the registration it was
//! minted for leaves the store, end to end over the restJson1 wire.
//!
//! The rule under test: a `taxDocumentAccessToken` locates ONE document of ONE
//! write of ONE account's tax registration, so once that write's record is gone
//! from the store the token names nothing EVER AGAIN — a later write for the
//! same account mints its own tokens and its own document locations, and the
//! tokens the removed registration had stay refused with the modelled
//! `ValidationException` (HTTP 400) carrying `InvalidToken`. The model states the
//! invalidation on `TaxDocumentMetadata$taxDocumentAccessToken`
//! (`smithy_query(query="shape", shape="TaxDocumentMetadata")`): an existing
//! token "won't be valid" once the registration changes, and the latest one
//! comes from `GetTaxRegistration` or `ListTaxRegistrations`.
//!
//! The rows span what the rule generalises over: how the document is named (an
//! uploaded file's name, an S3 object key carrying the path separator, two
//! documents sharing one name), how many documents one registration carries (one
//! against the five `VerificationDetails$taxRegistrationDocuments` admits), how
//! many times the registration is written before it is removed, and which delete
//! path removes it (`DeleteTaxRegistration`, `BatchDeleteTaxRegistration`). A
//! second table drives several delete-and-re-register cycles over one account, so
//! the rule is asserted over a history rather than over one removal.
//!
//! The harness (`emulator_with_engine`, `call`, `make_validations_due`) lives in
//! `tax_registration_lifecycle.rs` and the document readers
//! (`store_documents`, `get_document`, `reported_metadatas`, `assert_refused`)
//! in `tax_registration_document.rs`; nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::ERROR_CODE_INVALID_TOKEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_registration_document::{
    assert_refused, get_document, reported_metadatas, store_documents,
};
use crate::tax_registration_lifecycle::{call, emulator_with_engine, make_validations_due};

/// How a stored tax registration leaves the store. Both operations remove a
/// record, so the rule must hold whichever one a client used.
#[derive(Clone, Copy, Debug)]
enum DeletePath {
    /// `DeleteTaxRegistration`, which addresses one account.
    Single,
    /// `BatchDeleteTaxRegistration`, which addresses a list of them.
    Batch,
}

/// Remove `account_id`'s registration through `path`, asserting the call
/// succeeded — the removal is this suite's setup, not its subject.
async fn delete_registration(
    router: &MultiServiceRouter,
    account_id: &str,
    path: DeletePath,
    case: &str,
) {
    let answer = match path {
        DeletePath::Single => {
            call(
                router,
                "DeleteTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await
        }
        DeletePath::Batch => {
            call(
                router,
                "BatchDeleteTaxRegistration",
                &json!({ "accountIds": [account_id] }),
            )
            .await
        }
    };
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the setup delete through {path:?} must succeed, got {answer:?}"
    );
    let errors = answer.get("errors").as_array().cloned().unwrap_or_default();
    assert!(
        errors.is_empty(),
        "case {case:?}: the setup delete through {path:?} must report no per-account error, got \
         {errors:?}"
    );
}

/// The `taxDocumentAccessToken` a reported metadata carries.
fn token_of(metadata: &Value) -> String {
    metadata
        .get("taxDocumentAccessToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The URL one `GetTaxRegistrationDocument` answer reports for a held document.
fn url_of(answer: &crate::tax_registration_lifecycle::Answer) -> String {
    answer
        .get("presignedS3Url")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One account's history: the documents each of its registrations carries, how
/// many times the registration is written before it is removed, and the delete
/// path that removes it.
struct Row {
    name: &'static str,
    account_id: &'static str,
    documents: Value,
    writes_before_delete: usize,
    delete_path: DeletePath,
}

/// The histories the rule holds over.
///
/// A `TaxRegistrationDocument` names its document either by an uploaded file's
/// name or by its S3 object key, and
/// `VerificationDetails$taxRegistrationDocuments` admits one to five of them
/// (`smithy_query(query="shape", shape="VerificationDetails")`), so the rows
/// differ in that shape, in how many documents a registration carries, in
/// whether two of them share one name, in how many writes precede the removal,
/// and in which operation removes the record.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "one uploaded file, removed by DeleteTaxRegistration",
            account_id: "000000000401",
            documents: json!([
                { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
            ]),
            writes_before_delete: 1,
            delete_path: DeletePath::Single,
        },
        Row {
            name: "one S3 object key, removed by BatchDeleteTaxRegistration",
            account_id: "000000000402",
            documents: json!([
                { "s3Location": { "bucket": "trn-docs", "key": "2026/de/vat-certificate.pdf" } },
            ]),
            writes_before_delete: 1,
            delete_path: DeletePath::Batch,
        },
        Row {
            name: "two documents sharing one name, rewritten before the removal",
            account_id: "000000000403",
            documents: json!([
                { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
                { "file": { "fileName": "certificate.pdf", "fileContent": "d29ybGQ=" } },
            ]),
            writes_before_delete: 2,
            delete_path: DeletePath::Single,
        },
        Row {
            name: "the five documents the list admits, of both shapes, removed in a batch",
            account_id: "000000000404",
            documents: json!([
                { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
                { "s3Location": { "bucket": "trn-docs", "key": "director-id.png" } },
                { "file": { "fileName": "articles.pdf", "fileContent": "d29ybGQ=" } },
                { "s3Location": { "bucket": "trn-docs", "key": "archive/2025/vat.pdf" } },
                { "file": { "fileName": "power-of-attorney.pdf", "fileContent": "YWdhaW4=" } },
            ]),
            writes_before_delete: 2,
            delete_path: DeletePath::Batch,
        },
        Row {
            name: "a non-ASCII document name, written three times before the removal",
            account_id: "000000000405",
            documents: json!([
                { "file": { "fileName": "Umsatzsteuer-Zertifikat-Ö.pdf", "fileContent": "aGVsbG8=" } },
            ]),
            writes_before_delete: 3,
            delete_path: DeletePath::Single,
        },
    ]
}

/// Write `documents` as `account_id`'s registration and answer the metadata the
/// read reports, asserting every document of the record just written resolves.
/// A write needs the previous registration's TRN validation to have fallen due —
/// an outstanding TRN may not be replaced — which is state, never a sleep.
async fn write_and_resolve(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    account_id: &str,
    documents: &Value,
    case: &str,
) -> Vec<(Value, String)> {
    make_validations_due(engine);
    let metadatas = store_documents(router, account_id, documents).await;
    let expected = documents.as_array().map_or(0, Vec::len);
    assert_eq!(
        metadatas.len(),
        expected,
        "case {case:?}: expected one metadata per submitted document, got {metadatas:?}"
    );
    let mut resolved = Vec::new();
    for (index, metadata) in metadatas.iter().enumerate() {
        let answer = get_document(router, metadata, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: document {index} of the stored registration must resolve, got \
             {answer:?}"
        );
        resolved.push((metadata.clone(), url_of(&answer)));
    }
    resolved
}

/// RULE: removing a tax registration retires every document access token it had
/// for good — re-registering the account with the very same documents mints
/// tokens and document locations of its own, the retired tokens stay refused with
/// `InvalidToken`, and the read reports only the new ones.
#[tokio::test]
async fn re_registering_an_account_does_not_revive_a_deleted_registrations_tokens() {
    for row in rows() {
        // One emulator per row, so no row observes another's writes.
        let (router, engine) = emulator_with_engine();
        // Every token and every document location this account has ever been
        // answered, so a repeat of either is caught wherever it happens.
        let mut tokens: BTreeSet<String> = BTreeSet::new();
        let mut urls: BTreeSet<String> = BTreeSet::new();
        let mut minted = 0usize;
        // The metadata of every registration written before the removal.
        let mut retired: Vec<Value> = Vec::new();

        for write in 0..row.writes_before_delete {
            let case = format!("{} / write {write}", row.name);
            for (metadata, url) in
                write_and_resolve(&router, &engine, row.account_id, &row.documents, &case).await
            {
                tokens.insert(token_of(&metadata));
                urls.insert(url);
                minted += 1;
                retired.push(metadata);
            }
        }

        delete_registration(&router, row.account_id, row.delete_path, row.name).await;

        // While the account holds nothing, every token it ever had names
        // nothing.
        for (index, metadata) in retired.iter().enumerate() {
            let answer = get_document(&router, metadata, None).await;
            assert_refused(
                &format!("{} / token {index} while the account holds nothing", row.name),
                &answer,
                ERROR_CODE_INVALID_TOKEN,
            );
        }

        // The account is registered again with the same documents, at the same
        // positions, under the same names.
        let case = format!("{} / after the re-registration", row.name);
        let renewed =
            write_and_resolve(&router, &engine, row.account_id, &row.documents, &case).await;
        for (index, (metadata, url)) in renewed.iter().enumerate() {
            let token = token_of(metadata);
            assert!(
                !tokens.contains(&token),
                "case {case:?}: document {index} of the new registration must not be given a \
                 retired token, got {token:?}"
            );
            assert!(
                !urls.contains(url),
                "case {case:?}: document {index} of the new registration must have its own \
                 location, got {url:?}"
            );
            tokens.insert(token);
            urls.insert(url.clone());
            minted += 1;
        }

        // And the retired tokens are refused exactly as before, now that the
        // account holds a registration again: the refusal is about the token's
        // own registration, not about whether the account has one.
        for (index, metadata) in retired.iter().enumerate() {
            let answer = get_document(&router, metadata, None).await;
            assert_refused(
                &format!("{} / retired token {index} after the re-registration", row.name),
                &answer,
                ERROR_CODE_INVALID_TOKEN,
            );
        }

        // The read reports the new registration's metadata and nothing retired,
        // so a client can tell the two registrations' documents apart.
        let reported = reported_metadatas(&router, row.account_id).await;
        let expected: Vec<Value> = renewed
            .iter()
            .map(|(metadata, _)| metadata.clone())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the read must report the new registration's metadata, got {reported:?}"
        );
        assert_eq!(
            tokens.len(),
            minted,
            "case {:?}: every document ever stored must have its own access token, got {tokens:?}",
            row.name
        );
        assert_eq!(
            urls.len(),
            minted,
            "case {:?}: every document ever stored must have its own location, got {urls:?}",
            row.name
        );
    }
}

/// How many delete-and-re-register cycles the history table drives, and with how
/// many documents each cycle's registration is written. Each cycle is one more
/// chance for a token to be minted twice.
const CYCLES: &[(&str, usize)] = &[
    ("one document per cycle", 1),
    ("three documents per cycle", 3),
];

/// RULE, over a history: however many times an account's registration is removed
/// and written again, no document access token and no document location is ever
/// handed out twice, and every token of every removed registration stays refused.
#[tokio::test]
async fn repeated_removal_and_re_registration_never_repeats_a_token() {
    for (case, documents_per_cycle) in CYCLES {
        let (router, engine) = emulator_with_engine();
        let account_id = "000000000406";
        let documents = Value::Array(
            (0..*documents_per_cycle)
                .map(|index| {
                    json!({
                        "file": {
                            "fileName": format!("certificate-{index}.pdf"),
                            "fileContent": "aGVsbG8=",
                        },
                    })
                })
                .collect(),
        );
        let mut tokens: BTreeSet<String> = BTreeSet::new();
        let mut urls: BTreeSet<String> = BTreeSet::new();
        let mut retired: Vec<Value> = Vec::new();

        for cycle in 0..4 {
            let case = format!("{case} / cycle {cycle}");
            let written =
                write_and_resolve(&router, &engine, account_id, &documents, &case).await;
            for (index, (metadata, url)) in written.iter().enumerate() {
                let token = token_of(metadata);
                assert!(
                    tokens.insert(token.clone()),
                    "case {case:?}: document {index} was given a token this account already had, \
                     got {token:?}"
                );
                assert!(
                    urls.insert(url.clone()),
                    "case {case:?}: document {index} was given a location this account already \
                     had, got {url:?}"
                );
            }

            // Everything earlier in this account's history is still refused.
            for (index, metadata) in retired.iter().enumerate() {
                let answer = get_document(&router, metadata, None).await;
                assert_refused(
                    &format!("{case} / token {index} of an earlier registration"),
                    &answer,
                    ERROR_CODE_INVALID_TOKEN,
                );
            }

            delete_registration(&router, account_id, DeletePath::Single, &case).await;
            retired.extend(written.into_iter().map(|(metadata, _)| metadata));
        }

        assert_eq!(
            tokens.len(),
            retired.len(),
            "case {case:?}: every document of every cycle must have its own token, got {tokens:?}"
        );
    }
}
