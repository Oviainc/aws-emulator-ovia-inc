// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the error code S3 answers a REFUSED request with.
//!
//! The rule under test: a refusal carries one of S3's own error codes in its
//! `<Error><Code>` document, never the framework's generic `ValidationException`, and
//! WHICH code depends on where the offending value came from.
//!
//! * A value that fails a modelled constraint inside the request's XML DOCUMENT is a
//!   document that "did not validate against our published schema": `MalformedXML`, 400.
//! * A value that fails one in a HEADER, a QUERY parameter or a URI label is
//!   `InvalidArgument`, 400.
//! * A body carrying a character the XML 1.0 character set forbids is not well-formed XML
//!   at all, so it is `MalformedXML` before any handler runs — while the same character in
//!   an object key, which travels in the URI and through no document, is accepted.
//!
//! [`the_request_document_catalog_matches_the_model`] re-derives the member list that rule
//! keys on from the model itself.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_s3_amazons3::service::XML_DOCUMENT_PAYLOAD_MEMBERS;

use super::bucket_lifecycle::{create_bucket, router, send, send_bytes, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// One row of the refusal table: the case name, the request to send, and the error code
/// and status it must be refused with.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
    code: &'static str,
}

/// This service's Smithy model, parsed.
fn model() -> Value {
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3-amazons3.json"
    );
    serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON")
}

/// A member name as a validation path spells it: lowerCamel, the casing
/// `aws_smithy_schema_server::constraints::path::member` applies.
fn lower_camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

#[test]
fn the_request_document_catalog_matches_the_model() {
    // Every `@httpPayload` member of an operation INPUT whose target is a structured
    // shape — a structure, union, list or map, i.e. a body that is a whole XML document
    // rather than raw bytes — read from the model rather than from a second hand-written
    // list.
    let model = model();
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    let input_shapes: std::collections::BTreeSet<&str> = shapes
        .iter()
        .filter(|(_, shape)| shape["type"] == "operation")
        .filter_map(|(_, shape)| shape["input"]["target"].as_str())
        .collect();

    let mut derived: Vec<String> = Vec::new();
    for input in input_shapes {
        let Some(members) = shapes.get(input).and_then(|shape| shape["members"].as_object()) else {
            continue;
        };
        for (name, member) in members {
            if member["traits"].get("smithy.api#httpPayload").is_none() {
                continue;
            }
            let Some(target) = member["target"].as_str() else {
                continue;
            };
            let kind = shapes
                .get(target)
                .and_then(|shape| shape["type"].as_str())
                .unwrap_or_default();
            if matches!(kind, "structure" | "union" | "list" | "map" | "document") {
                derived.push(lower_camel(name));
            }
        }
    }
    derived.sort();
    derived.dedup();

    let mut declared: Vec<String> = XML_DOCUMENT_PAYLOAD_MEMBERS
        .iter()
        .map(|member| (*member).to_string())
        .collect();
    declared.sort();
    assert_eq!(
        declared, derived,
        "XML_DOCUMENT_PAYLOAD_MEMBERS must name exactly the structured @httpPayload input \
         members the model binds; declared {declared:?}, the model binds {derived:?}"
    );
}

/// The rows: one refused request per class the rule covers, each naming a DIFFERENT
/// operation, a different kind of constraint and a different HTTP binding, so the table
/// spans what the rule generalises over rather than repeating one recorded call.
fn rows(bucket: &str) -> Vec<Row> {
    vec![
        // An enum member bound to a HEADER.
        Row {
            case: "an x-amz-tagging-directive outside TaggingDirective",
            method: "PUT",
            uri: format!("/{bucket}/copied"),
            headers: vec![
                ("x-amz-copy-source", "refusal-codes/source"),
                ("x-amz-tagging-directive", "MERGE"),
            ],
            body: String::new(),
            code: "InvalidArgument",
        },
        // An enum member bound to a header whose target is a LIST of enums, so the
        // violation path names a list element rather than the member itself.
        Row {
            case: "an x-amz-object-attributes element outside ObjectAttributes",
            method: "GET",
            uri: format!("/{bucket}/source?attributes"),
            headers: vec![("x-amz-object-attributes", "NotARealAttribute")],
            body: String::new(),
            code: "InvalidArgument",
        },
        Row {
            case: "an x-amz-object-ownership outside ObjectOwnership",
            method: "PUT",
            uri: "/refusal-codes-ownership".to_string(),
            headers: vec![("x-amz-object-ownership", "SomebodyElse")],
            body: String::new(),
            code: "InvalidArgument",
        },
        // A @range member bound to a QUERY parameter, above its published maximum.
        Row {
            case: "a max-buckets above the published maximum",
            method: "GET",
            uri: "/?x-id=ListBuckets&max-buckets=10001".to_string(),
            headers: Vec::new(),
            body: String::new(),
            code: "InvalidArgument",
        },
        // The same bound at the other end: below the published minimum.
        Row {
            case: "a max-buckets below the published minimum",
            method: "GET",
            uri: "/?x-id=ListBuckets&max-buckets=0".to_string(),
            headers: Vec::new(),
            body: String::new(),
            code: "InvalidArgument",
        },
        // An enum member inside the request DOCUMENT.
        Row {
            case: "a VersioningConfiguration Status outside BucketVersioningStatus",
            method: "PUT",
            uri: format!("/{bucket}?versioning"),
            headers: Vec::new(),
            body: format!(
                "<VersioningConfiguration xmlns=\"{XMLNS}\"><Status>Disabled</Status>\
                 </VersioningConfiguration>"
            ),
            code: "MalformedXML",
        },
        // An enum member NESTED two levels inside the request document.
        Row {
            case: "an InventoryConfiguration Schedule Frequency outside InventoryFrequency",
            method: "PUT",
            uri: format!("/{bucket}?inventory&id=refused"),
            headers: Vec::new(),
            body: format!(
                "<InventoryConfiguration xmlns=\"{XMLNS}\"><Id>refused</Id>\
                 <IsEnabled>true</IsEnabled><IncludedObjectVersions>All\
                 </IncludedObjectVersions><Schedule><Frequency>Hourly</Frequency></Schedule>\
                 <Destination><S3BucketDestination><Bucket>arn:aws:s3:::other</Bucket>\
                 <Format>CSV</Format></S3BucketDestination></Destination>\
                 </InventoryConfiguration>"
            ),
            code: "MalformedXML",
        },
        // A REQUIRED member absent from the request document.
        Row {
            case: "an OwnershipControls payload naming no Rule",
            method: "PUT",
            uri: format!("/{bucket}?ownershipControls"),
            headers: Vec::new(),
            body: format!("<OwnershipControls xmlns=\"{XMLNS}\"></OwnershipControls>"),
            code: "MalformedXML",
        },
    ]
}

#[tokio::test]
async fn a_refused_request_carries_the_s3_code_for_where_the_value_came_from() {
    let router = router();
    let bucket = "refusal-codes";
    create_bucket(&router, bucket, None, None).await;
    let stored = send_bytes(
        &router,
        "PUT",
        &format!("/{bucket}/source"),
        &[("content-type", "text/plain")],
        b"body",
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "the source object must be stored first, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );

    for row in rows(bucket) {
        let answer = send(&router, row.method, &row.uri, &row.headers, &row.body).await;
        let code = answer.error_code();
        assert_eq!(
            code, row.code,
            "case {:?}: expected the S3 error code {:?}, got {code:?} with status {} and body \
             {:?}",
            row.case,
            row.code,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.status, 400,
            "case {:?}: an S3 refusal of a bad request is HTTP 400, got {}",
            row.case, answer.status
        );
        assert_ne!(
            code, "ValidationException",
            "case {:?}: a refusal must never carry the framework's generic code",
            row.case
        );
    }
}

/// `POST /<bucket>?delete` — `DeleteObjects`, with the `Delete` document as the body.
async fn delete_objects(router: &MultiServiceRouter, bucket: &str, keys: &[&str]) -> Answer {
    let mut body = format!("<Delete xmlns=\"{XMLNS}\">");
    for key in keys {
        body.push_str(&format!("<Object><Key>{key}</Key></Object>"));
    }
    body.push_str("</Delete>");
    send(router, "POST", &format!("/{bucket}?delete"), &[], &body).await
}

#[tokio::test]
async fn a_document_carrying_a_character_xml_forbids_is_malformed_xml() {
    let router = router();
    let bucket = "refusal-codes-charset";
    create_bucket(&router, bucket, None, None).await;

    // Every row is a code point outside the XML 1.0 `Char` production (W3C XML 1.0, §2.2):
    // the C0 controls other than tab, line feed and carriage return, at both ends of that
    // range, plus the two non-characters at the end of the BMP.
    for (case, forbidden) in [
        ("NUL", '\u{0}'),
        ("BEL", '\u{7}'),
        ("vertical tab", '\u{b}'),
        ("form feed", '\u{c}'),
        ("SO", '\u{e}'),
        ("unit separator", '\u{1f}'),
        ("U+FFFE", '\u{fffe}'),
    ] {
        let key = format!("key{forbidden}name");
        let answer = delete_objects(&router, bucket, &[&key]).await;
        assert_eq!(
            answer.error_code(),
            "MalformedXML",
            "case {case:?}: a Delete document carrying {forbidden:?} is not well-formed XML, \
             got {:?} with status {} and body {:?}",
            answer.error_code(),
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {}",
            answer.status
        );
    }

    // Characters the XML character set DOES allow travel through the same document
    // untouched: tab, line feed and carriage return are `Char`, and so is an astral-plane
    // code point.
    for (case, allowed) in [
        ("tab", '\u{9}'),
        ("line feed", '\u{a}'),
        ("an astral-plane code point", '\u{1f600}'),
    ] {
        let key = format!("allowed{allowed}key");
        let answer = delete_objects(&router, bucket, &[&key]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: {allowed:?} is a character XML allows, so the delete must be \
             served, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The same forbidden character in an object KEY, which travels in the request URI and
    // through no document at all, is accepted: the rule is about the XML charset, not about
    // the character. The URI carries it percent-encoded, as every client sends a control
    // character in a path.
    let stored = send_bytes(
        &router,
        "PUT",
        &format!("/{bucket}/vertical%0Btab"),
        &[("content-type", "text/plain")],
        b"bytes",
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "a PUT whose KEY carries the same character is served, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );
}
