// SPDX-License-Identifier: CC0-1.0

//! The destination a `GetTaxRegistrationDocument` request names, end to end over
//! the restJson1 wire.
//!
//! The rule these tables assert is one: the path reported for a request that
//! names a destination is computed from the WHOLE location the request gave —
//! the `bucket` the document is asked to be downloaded to, then the object key
//! it occupies under the `prefix` — so two requests differing only in the
//! location they name are answered different paths, while the document's own
//! download URL, which names where the service keeps the document, does not move
//! with the destination at all.
//!
//! The rows span the location forms the rule generalises over rather than one
//! recorded request: buckets at each published length bound and one past it,
//! dotted and hyphenated bucket labels, prefixes absent, flat, nested, spelled
//! with their trailing separator, at the published length bound and one past it,
//! and a location with no bucket at all.
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs` and
//! the document setup (`store_documents`, `get_document`) in
//! `tax_registration_document.rs`; nothing here builds a second one.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::tax_registration_document::{get_document, store_documents};
use crate::tax_registration_lifecycle::emulator;

/// The document every case below asks for.
const DOCUMENT_NAME: &str = "certificate.pdf";

/// One location a request may name, and the object key the document is expected
/// to occupy under it.
struct LocationRow {
    name: &'static str,
    bucket: String,
    prefix: Option<String>,
    expected_key: String,
}

/// The locations the rule holds over.
///
/// `DestinationS3Location$bucket` is `@required`, `@length 3..63` and admits
/// dot-separated lowercase labels; `prefix` is `@length 0..512`
/// (`smithy_query(query="shape", shape="DestinationS3Location")`). The rows are
/// a plain bucket, the same prefix under another bucket, a bucket at each
/// published length bound, a dotted bucket, a hyphenated-and-numbered bucket,
/// and prefixes that are absent, flat, nested, spelled with the trailing
/// separator, bare separator, and at the published length bound.
fn location_rows() -> Vec<LocationRow> {
    // Each published bound of `bucket`'s 3..63 length, built from label
    // characters the member's pattern admits.
    let shortest_bucket = "abc".to_string();
    let longest_bucket = format!("b{}", "a".repeat(62));
    let longest_prefix = "p".repeat(512);
    vec![
        LocationRow {
            name: "a bucket alone",
            bucket: "my-tax-documents".to_string(),
            prefix: None,
            expected_key: DOCUMENT_NAME.to_string(),
        },
        LocationRow {
            name: "another bucket, no prefix",
            bucket: "somewhere-else-entirely".to_string(),
            prefix: None,
            expected_key: DOCUMENT_NAME.to_string(),
        },
        LocationRow {
            name: "a bucket at the shortest published length",
            bucket: shortest_bucket,
            prefix: None,
            expected_key: DOCUMENT_NAME.to_string(),
        },
        LocationRow {
            name: "a bucket at the longest published length",
            bucket: longest_bucket,
            prefix: None,
            expected_key: DOCUMENT_NAME.to_string(),
        },
        LocationRow {
            name: "a dotted bucket label",
            bucket: "my.tax.documents".to_string(),
            prefix: Some("tax".to_string()),
            expected_key: format!("tax/{DOCUMENT_NAME}"),
        },
        LocationRow {
            name: "a hyphenated and numbered bucket",
            bucket: "acme-trn-2026".to_string(),
            prefix: Some("tax/2026".to_string()),
            expected_key: format!("tax/2026/{DOCUMENT_NAME}"),
        },
        LocationRow {
            name: "the same prefix under a different bucket",
            bucket: "acme-trn-archive".to_string(),
            prefix: Some("tax/2026".to_string()),
            expected_key: format!("tax/2026/{DOCUMENT_NAME}"),
        },
        LocationRow {
            name: "a deeply nested prefix",
            bucket: "acme-trn-2026".to_string(),
            prefix: Some("tax/2026/de/vat/certificates".to_string()),
            expected_key: format!("tax/2026/de/vat/certificates/{DOCUMENT_NAME}"),
        },
        LocationRow {
            name: "the nested prefix spelled with its trailing separator",
            bucket: "acme-trn-2026".to_string(),
            prefix: Some("tax/2026/de/vat/certificates/".to_string()),
            expected_key: format!("tax/2026/de/vat/certificates/{DOCUMENT_NAME}"),
        },
        LocationRow {
            name: "a prefix that is the separator alone",
            bucket: "acme-trn-2026".to_string(),
            prefix: Some("/".to_string()),
            expected_key: DOCUMENT_NAME.to_string(),
        },
        LocationRow {
            name: "a prefix at the longest published length",
            bucket: "acme-trn-2026".to_string(),
            prefix: Some(longest_prefix.clone()),
            expected_key: format!("{longest_prefix}/{DOCUMENT_NAME}"),
        },
    ]
}

/// The JSON a `LocationRow` is submitted as.
fn location_body(row: &LocationRow) -> Value {
    let mut location = json!({ "bucket": row.bucket });
    if let Some(prefix) = &row.prefix {
        location["prefix"] = json!(prefix);
    }
    location
}

/// RULE: a request naming a destination is answered the path the document is to
/// be downloaded to in THAT location — the bucket the request gave, then the key
/// under the prefix it asked for — so a request that names a different bucket is
/// answered a different path, and the document's own download URL does not vary
/// with the destination.
#[tokio::test]
async fn the_reported_path_names_the_location_the_request_gave() {
    let router = emulator();
    let account_id = "000000000401";
    let metadatas = store_documents(
        &router,
        account_id,
        &json!([
            { "file": { "fileName": DOCUMENT_NAME, "fileContent": "aGVsbG8=" } },
        ]),
    )
    .await;
    let metadata = metadatas
        .first()
        .cloned()
        .expect("the stored registration reports its document");

    // What the same question is answered when it names no destination: there is
    // no path to report, and the URL below must not move with the destination.
    let without = get_document(&router, &metadata, None).await;
    assert_eq!(
        without.status, 200,
        "a request naming no destination must be answered, got {without:?}"
    );
    assert_eq!(
        without.get("destinationFilePath"),
        &Value::Null,
        "a request naming no destination asked for no download, so no path may be \
         reported, got {:?}",
        without.body
    );
    let document_url = without
        .get("presignedS3Url")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Every path this test is answered, by the location that was asked for, so
    // two locations cannot be answered one path.
    let mut paths: BTreeMap<String, String> = BTreeMap::new();

    for row in location_rows() {
        let case = row.name;
        let answer = get_document(&router, &metadata, Some(&location_body(&row))).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );

        let reported = answer
            .get("destinationFilePath")
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The path AWS writes to an object in an S3 bucket: the bucket, then the
        // key under it. Written out rather than computed from the handler's own
        // helper, so a bug there cannot move both sides of the assertion.
        let expected = format!("s3://{}/{}", row.bucket, row.expected_key);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected destinationFilePath {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            reported.contains(&row.bucket),
            "case {case:?}: the reported path must name the bucket {:?} the request gave, got \
             {reported:?}",
            row.bucket
        );
        assert!(
            reported.ends_with(DOCUMENT_NAME),
            "case {case:?}: the reported path must end at the document's own name, got \
             {reported:?}"
        );

        // Where the document itself is belongs to the service, not to the
        // destination the request asked for, so it is the same answer every
        // time.
        let url = answer
            .get("presignedS3Url")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            url, document_url,
            "case {case:?}: the document's own URL must not vary with the destination, got {:?}",
            answer.body
        );

        // The location the row names, with its prefix normalised the way a
        // trailing separator is: two rows that name the SAME place in the same
        // bucket are expected to be answered the same path, and no other pair
        // is.
        let location = format!(
            "{}|{}",
            row.bucket,
            row.prefix.unwrap_or_default().trim_end_matches('/')
        );
        if let Some(previous) = paths.insert(reported.clone(), location.clone()) {
            assert_eq!(
                previous, location,
                "case {case:?}: location {location:?} was answered the same path {reported:?} as \
                 location {previous:?}"
            );
        }
    }

    // The two rows that differ only in their bucket, and the two that differ
    // only in their prefix, were each answered their own path.
    assert_eq!(
        paths.len(),
        location_rows()
            .iter()
            .map(|row| format!(
                "{}|{}",
                row.bucket,
                row.prefix.clone().unwrap_or_default().trim_end_matches('/')
            ))
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        "each distinct location must have its own path, got {paths:?}"
    );
}

/// RULE: a `destinationS3Location` outside the constraints the model publishes
/// for it is refused (HTTP 400 `ValidationException`) rather than answered a
/// path — `bucket` is required and bounded at 3..63 characters, `prefix` at 512
/// (`smithy_query(query="shape", shape="DestinationS3Location")`).
#[tokio::test]
async fn a_destination_outside_the_published_constraints_is_refused() {
    let router = emulator();
    let account_id = "000000000402";
    let metadatas = store_documents(
        &router,
        account_id,
        &json!([
            { "file": { "fileName": DOCUMENT_NAME, "fileContent": "aGVsbG8=" } },
        ]),
    )
    .await;
    let metadata = metadatas
        .first()
        .cloned()
        .expect("the stored registration reports its document");

    // Each row is one past a published bound of the location, or the omission of
    // its one required member.
    let rows: Vec<(&str, Value)> = vec![
        ("a location naming no bucket", json!({ "prefix": "tax" })),
        (
            "a bucket one character short of the shortest published length",
            json!({ "bucket": "ab" }),
        ),
        (
            "a bucket one character past the longest published length",
            json!({ "bucket": format!("b{}", "a".repeat(63)) }),
        ),
        (
            "a prefix one character past the longest published length",
            json!({ "bucket": "acme-trn-2026", "prefix": "p".repeat(513) }),
        ),
    ];

    for (case, destination) in rows {
        let answer = get_document(&router, &metadata, Some(&destination)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.get("destinationFilePath"),
            &Value::Null,
            "case {case:?}: a refused request must report no path, got {:?}",
            answer.body
        );
    }

    // The refusals changed nothing: a location the model admits is still
    // answered its own path.
    let answer = get_document(
        &router,
        &metadata,
        Some(&json!({ "bucket": "acme-trn-2026", "prefix": "tax" })),
    )
    .await;
    assert_eq!(
        answer
            .get("destinationFilePath")
            .as_str()
            .unwrap_or_default(),
        format!("s3://acme-trn-2026/tax/{DOCUMENT_NAME}"),
        "an admitted location must still be answered its own path, got {answer:?}"
    );
}
