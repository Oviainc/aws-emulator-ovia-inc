// SPDX-License-Identifier: CC0-1.0

//! How ACM words a constraint failure on its TAG surface, over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: a constraint clause about an input position that carries TAG METADATA — a tag
//! collection, or anything reached through one: a pair's key, a pair's value, an entry of
//! a key list — names that position and NOT the text submitted there
//! (`Value of the input at '<member path>' failed to satisfy constraint: <requirement>`),
//! which is the subject ACM states EVERY position with: a position off the tag surface
//! reads the same way, so no caller-supplied text reaches a message wherever it came
//! from.
//!
//! The tables below span what the rule generalises over rather than the one request a
//! trace recorded:
//!
//! * both halves of a pair (`tags.<n>.member.key`, `tags.<n>.member.value`), the
//!   collection itself (`tags`) and a key list's entries (`tagKeys`);
//! * every operation of this service that takes tags from a request — the generic
//!   resource pair (`TagResource` / `UntagResource`), the certificate pair
//!   (`AddTagsToCertificate` / `RemoveTagsFromCertificate`), and the two creates that
//!   carry a `TagList` (`RequestCertificate`, `CreateAcmeEndpoint`) — including
//!   `CreateAcmeEndpoint`'s QUALIFIED tag member `CertificateTags`
//!   (`smithy_query(query="operation", operation="CreateAcmeEndpoint")`), which no trace
//!   cites;
//! * every constraint a tag shape declares: the shared `@pattern`, `TagKey`'s and
//!   `TagValue`'s `@length` maxima, and `TagList`'s own maximum;
//! * the text classes the `@pattern` excludes, including one that is not renderable at
//!   all (an emoji, Unicode category `So`) and a control character — the values whose
//!   echo would land in a log;
//! * the rows OFF the tag surface, which state the position alike — an integer member
//!   and a string member of a nested structure.
//!
//! The envelope itself (`N validation error(s) detected: `), the unanchored `@pattern`
//! rendering and the absent-member subject have their own suite in
//! [`crate::validation_message_rendering`] and are not restated here.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    call, error_code, error_message, request_certificate, router, ACCOUNT, REGION,
};
use crate::resource_tagging::{create_endpoint, stored_tags, tag_resource};

/// How ACM names the subject of a clause at a position whose submitted text it
/// withholds.
const WITHHOLDING_SUBJECT: &str = "Value of the input at";

/// The `@pattern` both tag shapes declare, as a clause reports it: the expression
/// without the `^` / `$` the model anchors it with
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagKey,com.amazonaws.acm#TagValue")`).
const TAG_PATTERN: &str = r"([\p{L}\p{Z}\p{N}_.:/=+\-@]*)";

/// `TagKey`'s modelled maximum length (`len 1..128`).
const TAG_KEY_MAX: usize = 128;

/// `TagValue`'s modelled maximum length (`len ..256`).
const TAG_VALUE_MAX: usize = 256;

/// `TagList`'s and `TagKeyList`'s modelled maximum length (`len 1..50`).
const TAG_LIST_MAX: usize = 50;

/// Every text class the tag `@pattern` EXCLUDES, each paired with what it exercises: the
/// pattern admits letters (`\p{L}`), separators (`\p{Z}`), numbers (`\p{N}`) and the
/// punctuation set `_ . : / = + - @`, so none of these is admitted. The emoji and the
/// control character are the rows whose echo would be unrenderable.
fn out_of_charset_texts() -> Vec<(&'static str, String)> {
    vec![
        ("a number sign", "env#prod".to_string()),
        ("a percent sign", "env%prod".to_string()),
        ("curly braces", "env{prod}".to_string()),
        ("an emoji, Unicode category So", "status-\u{2705}-ok".to_string()),
        ("a currency symbol, Unicode category Sc", "cost-\u{20ac}-eur".to_string()),
        ("a control character", "env\u{0001}prod".to_string()),
        ("a newline", "env\nprod".to_string()),
    ]
}

/// One row: what it exercises, the operation, the request body, the member path the
/// clause names, the requirement it states, and the text the request submitted at that
/// position — which the clause must NOT carry.
struct Row {
    what: String,
    operation: &'static str,
    body: Value,
    path: &'static str,
    requirement: String,
    submitted: String,
}

/// The requirement a `@pattern` violation states.
fn pattern_requirement() -> String {
    format!("Member must satisfy regular expression pattern: {TAG_PATTERN}")
}

/// The requirement a `@length` maximum violation states.
fn max_length_requirement(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The whole single-clause envelope a row must answer with.
fn expected_message(row: &Row) -> String {
    format!(
        "1 validation error detected: {WITHHOLDING_SUBJECT} '{}' failed to satisfy constraint: {}",
        row.path, row.requirement
    )
}

/// A `CreateAcmeEndpoint` body carrying `tags` under the member `member`.
fn create_endpoint_body(member: &str, tags: Value) -> Value {
    let mut body = json!({
        "AuthorizationBehavior": "PRE_APPROVED",
        "Contact": "REQUIRED",
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
    });
    body[member] = tags;
    body
}

/// A `TagKeys`/`Tags` list of `count` distinct, legal entries.
fn legal_pairs(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("key{index:02}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// The rows the rule is asserted over. `endpoint` is a stored ACME endpoint's ARN and
/// `certificate` a stored certificate's.
fn rows(endpoint: &str, certificate: &str) -> Vec<Row> {
    let long_key = "k".repeat(TAG_KEY_MAX + 1);
    let long_value = "v".repeat(TAG_VALUE_MAX + 1);

    let mut rows = vec![
        // The generic resource pair: both halves of a pair, and the collection itself.
        Row {
            what: "a tag value outside the shared pattern on TagResource".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": "env", "Value": "prod#1" }],
            }),
            path: "tags.1.member.value",
            requirement: pattern_requirement(),
            submitted: "prod#1".to_string(),
        },
        Row {
            what: "a tag key outside the shared pattern on TagResource".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": "env#1", "Value": "prod" }],
            }),
            path: "tags.1.member.key",
            requirement: pattern_requirement(),
            submitted: "env#1".to_string(),
        },
        Row {
            what: "a tag value one character past TagValue's maximum".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": "env", "Value": long_value.clone() }],
            }),
            path: "tags.1.member.value",
            requirement: max_length_requirement(TAG_VALUE_MAX),
            submitted: long_value,
        },
        Row {
            what: "a tag key one character past TagKey's maximum".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": long_key.clone(), "Value": "prod" }],
            }),
            path: "tags.1.member.key",
            requirement: max_length_requirement(TAG_KEY_MAX),
            submitted: long_key,
        },
        Row {
            what: "one pair past TagList's maximum, a clause about the collection"
                .to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": endpoint,
                "Tags": legal_pairs(TAG_LIST_MAX + 1),
            }),
            path: "tags",
            requirement: max_length_requirement(TAG_LIST_MAX),
            // The walk has no text for a collection, so what it would state is its own
            // count summary rather than anything submitted.
            submitted: format!("{} items", TAG_LIST_MAX + 1),
        },
        // A key list's entry, whose clause is collapsed onto the collection with the
        // element shape's whole constraint set.
        Row {
            what: "a key to remove outside the shared pattern on UntagResource".to_string(),
            operation: "UntagResource",
            body: json!({ "ResourceArn": endpoint, "TagKeys": ["env#1"] }),
            path: "tagKeys",
            requirement: format!(
                "Member must satisfy constraint: [{}, Member must have length greater than or \
                 equal to 1, {}, Member must not be null]",
                max_length_requirement(TAG_KEY_MAX),
                pattern_requirement(),
            ),
            submitted: "env#1".to_string(),
        },
        // The certificate pair, which addresses its resource by a different member.
        Row {
            what: "a tag value outside the shared pattern on AddTagsToCertificate".to_string(),
            operation: "AddTagsToCertificate",
            body: json!({
                "CertificateArn": certificate,
                "Tags": [{ "Key": "env", "Value": "prod%1" }],
            }),
            path: "tags.1.member.value",
            requirement: pattern_requirement(),
            submitted: "prod%1".to_string(),
        },
        Row {
            what: "a tag key outside the shared pattern on RemoveTagsFromCertificate"
                .to_string(),
            operation: "RemoveTagsFromCertificate",
            body: json!({
                "CertificateArn": certificate,
                "Tags": [{ "Key": "env{1}" }],
            }),
            path: "tags.1.member.key",
            requirement: pattern_requirement(),
            submitted: "env{1}".to_string(),
        },
        // The creates that carry a `TagList`, including the QUALIFIED tag member.
        Row {
            what: "a tag value outside the shared pattern on RequestCertificate".to_string(),
            operation: "RequestCertificate",
            body: json!({
                "DomainName": "tagged.example.com",
                "ValidationMethod": "DNS",
                "Tags": [{ "Key": "env", "Value": "prod&1" }],
            }),
            path: "tags.1.member.value",
            requirement: pattern_requirement(),
            submitted: "prod&1".to_string(),
        },
        Row {
            what: "a tag key outside the shared pattern on CreateAcmeEndpoint".to_string(),
            operation: "CreateAcmeEndpoint",
            body: create_endpoint_body("Tags", json!([{ "Key": "env*1", "Value": "prod" }])),
            path: "tags.1.member.key",
            requirement: pattern_requirement(),
            submitted: "env*1".to_string(),
        },
        Row {
            what: "a value of the qualified tag member certificateTags".to_string(),
            operation: "CreateAcmeEndpoint",
            body: create_endpoint_body(
                "CertificateTags",
                json!([{ "Key": "env", "Value": "prod|1" }]),
            ),
            path: "certificateTags.1.member.value",
            requirement: pattern_requirement(),
            submitted: "prod|1".to_string(),
        },
        // The rows OFF the tag surface: they state the position the same way, so the
        // rule above is the service's one subject rather than the tag surface's own.
        Row {
            what: "an integer member off the tag surface, below its range minimum".to_string(),
            operation: "ListCertificates",
            body: json!({ "MaxItems": 0 }),
            path: "maxItems",
            requirement: "Member must have value greater than or equal to 1".to_string(),
            submitted: "0".to_string(),
        },
        Row {
            what: "a string member off the tag surface, outside its pattern".to_string(),
            operation: "CreateAcmeDomainValidation",
            body: json!({
                "AcmeEndpointArn": endpoint,
                "DomainName": "withheld.example.com",
                "PrevalidationOptions": { "DnsPrevalidation": { "HostedZoneId": "zone-1" } },
            }),
            path: "prevalidationOptions.dnsPrevalidation.hostedZoneId",
            requirement: "Member must satisfy regular expression pattern: Z[A-Z0-9]+".to_string(),
            submitted: "zone-1".to_string(),
        },
    ];

    // Every excluded text class, on the value and on the key of a generic tag pair, and
    // on the value of a certificate tag pair — the same clause whichever operation the
    // pair arrived on.
    for (class, text) in out_of_charset_texts() {
        rows.push(Row {
            what: format!("{class} in a tag value on TagResource"),
            operation: "TagResource",
            body: json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": "env", "Value": text.clone() }],
            }),
            path: "tags.1.member.value",
            requirement: pattern_requirement(),
            submitted: text.clone(),
        });
        rows.push(Row {
            what: format!("{class} in a tag key on TagResource"),
            operation: "TagResource",
            body: json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": text.clone(), "Value": "prod" }],
            }),
            path: "tags.1.member.key",
            requirement: pattern_requirement(),
            submitted: text.clone(),
        });
        rows.push(Row {
            what: format!("{class} in a tag value on AddTagsToCertificate"),
            operation: "AddTagsToCertificate",
            body: json!({
                "CertificateArn": certificate,
                "Tags": [{ "Key": "env", "Value": text.clone() }],
            }),
            path: "tags.1.member.value",
            requirement: pattern_requirement(),
            submitted: text,
        });
    }

    rows
}

/// RULE: a clause about a position on ACM's tag surface names the position and withholds
/// the text submitted there — as a clause about a position off that surface does too.
#[tokio::test]
async fn a_tag_surface_clause_names_the_position_and_withholds_the_text() {
    let router = router();
    let case = "the tag subject rule";
    let endpoint = create_endpoint(&router, case).await;
    // One legal pair first, so the "nothing was written" read below is a claim about a
    // NON-empty map: a refusal that wiped it would be caught too.
    tag_resource(
        &router,
        case,
        &endpoint,
        &json!([{ "Key": "env", "Value": "dev" }]),
    )
    .await;
    let certificate = request_certificate(
        &router,
        &json!({ "DomainName": "subject.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    for row in rows(&endpoint, &certificate) {
        let (status, response) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {:?} ({}): expected 400 ValidationException, got {status} {response}",
            row.what,
            row.operation
        );
        let stated = error_message(&response);
        assert_eq!(
            stated,
            expected_message(&row),
            "case {:?} ({}): expected the ACM constraint envelope, got {stated:?}",
            row.what,
            row.operation
        );
        assert!(
            !stated.contains(&row.submitted),
            "case {:?} ({}): the submitted text {:?} must not appear in the message, \
             got {stated:?}",
            row.what,
            row.operation,
            row.submitted
        );
    }

    // Every refused call above left the stored map exactly as the one legal pair left
    // it, read back through a different operation than the writes.
    let tags = stored_tags(&router, case, &endpoint).await;
    assert_eq!(
        tags,
        vec![("env".to_string(), "dev".to_string())],
        "expected the endpoint to keep its one legal pair through every refusal, got {tags:?}"
    );
}

/// RULE (the other half): an in-charset tag value is SERVED and round-trips byte for
/// byte, so the refusals above are the shape's doing rather than a handler refusing text
/// it cannot render.
///
/// The rows are the character classes the pattern admits outside ASCII — the scripts, the
/// non-ASCII separators and numbers — each read back through `ListTagsForResource`.
#[tokio::test]
async fn an_in_charset_tag_value_round_trips_unchanged() {
    let router = router();

    // (what it exercises, the key, the value)
    let rows: [(&str, &str, &str); 5] = [
        ("accented Latin, CJK and Greek in one value", "unicode", "café-テスト-Ω"),
        ("Cyrillic", "cyrillic", "продакшн"),
        // `\p{Z}` is every separator and `\p{N}` every number, not the ASCII ones alone.
        ("the non-ASCII separators the pattern admits", "separators", "team\u{2003}sre"),
        ("the non-ASCII numbers the pattern admits", "numbers", "tier\u{2167}"),
        ("the whole punctuation set the pattern admits", "punctuation", "j_k.l:m/n=o+p-q@r"),
    ];

    for (what, key, value) in rows {
        let arn = create_endpoint(&router, what).await;
        tag_resource(&router, what, &arn, &json!([{ "Key": key, "Value": value }])).await;
        let stored = stored_tags(&router, what, &arn).await;
        assert_eq!(
            stored,
            vec![(key.to_string(), value.to_string())],
            "case {what:?}: expected the value stored byte for byte, got {stored:?}"
        );
    }
}

/// RULE: the withholding subject is the tag surface's wherever that surface is reached,
/// so a refusal naming an ACM resource of this account's own Region states it too — the
/// ARN the row addresses is well formed, which keeps the clause the tag member's.
#[tokio::test]
async fn a_tag_clause_withholds_the_text_for_a_resource_that_does_not_exist() {
    let router = router();
    // A syntactically well-formed ARN of this account and Region with no record behind
    // it: the walk refuses the tag before any lookup, so the answer is the tag clause
    // rather than the not-found.
    let absent = format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/{}", "a".repeat(32));

    let (status, response) = call(
        &router,
        "TagResource",
        &json!({ "ResourceArn": absent, "Tags": [{ "Key": "env", "Value": "prod?1" }] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case an absent resource: expected the constraint refusal, got {status} {response}"
    );
    let stated = error_message(&response);
    assert_eq!(
        stated,
        format!(
            "1 validation error detected: {WITHHOLDING_SUBJECT} 'tags.1.member.value' failed to \
             satisfy constraint: {}",
            pattern_requirement()
        ),
        "case an absent resource: expected the withholding tag clause, got {stated:?}"
    );
}
