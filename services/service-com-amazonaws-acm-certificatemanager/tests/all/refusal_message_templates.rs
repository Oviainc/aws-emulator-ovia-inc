// SPDX-License-Identifier: CC0-1.0

//! How ACM words three refusals whose template is known, over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! Three rules, each asserted over a TABLE rather than the one request a trace
//! happened to record:
//!
//! 1. A constraint clause names THE POSITION in the input that failed and nothing the
//!    request submitted there — `Value of the input at '<member path>' failed to satisfy
//!    constraint: <requirement>` — at every position: a resource identifier (a `…Arn`
//!    member), a request token (a `…Token` member), a plain scalar, a value the
//!    validating walk holds no submitted text for (a collection, whose only rendering is
//!    its element count, and a value past the walk's own echo cap), and a clause
//!    collapsed onto a collection member. The rows span
//!    the constraint kinds (`@length` minimum and maximum, `@pattern`, `@range`), the
//!    member depth (a top-level member, a member of a structure under a UNION member, a
//!    collection element) and the value forms (empty, short, at a bound, past a bound,
//!    far past the echo cap).
//! 2. A tag whose key or value begins with the `aws:` prefix AWS reserves is refused
//!    with a fixed template naming the offending HALF of the pair and quoting its text
//!    (`Invalid Tag Key: "<key>". AWS internal tags cannot be changed with this API.`),
//!    at every operation that takes tags from a request.
//! 3. A request that would leave a resource carrying more than the modelled `TagList`
//!    maximum of user tags is refused with the tagging layer's constant sentence for
//!    that quota (`The resultant tag set must not have more than 50 user tags.`),
//!    whichever modelled error the operation carries it on.
//!
//! The envelope itself (`N validation error(s) detected: `), the member paths, the
//! unanchored `@pattern` rendering and the absent-member subject have their own suite
//! in [`crate::validation_message_rendering`] and are not restated here.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the path the emulator
//! serves real traffic on, and every rejected write is read back through a DIFFERENT
//! operation, so a refusal that quietly wrote cannot pass.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    call, error_code, error_message, request_certificate, router, ACCOUNT, REGION,
};
use crate::resource_tagging::{create_endpoint, stored_tags};

/// The modelled `TagList` maximum, which is the per-resource user-tag quota
/// (`smithy_query(query="operation", operation="TagResource")` reports `Tags` as
/// `len 1..50`).
const TAG_LIST_MAX: usize = 50;

/// `HostedZoneId`'s modelled maximum length
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#HostedZoneId")`: `len 1..32`,
/// `@pattern ^Z[A-Z0-9]+$`).
const HOSTED_ZONE_ID_MAX: usize = 32;

/// `DomainName`'s modelled maximum length
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#DomainNameString")`, reached
/// through `smithy_query(query="operation", operation="CreateAcmeDomainValidation")`:
/// `len 1..253`).
const DOMAIN_NAME_MAX: usize = 253;

/// The `@pattern` of `com.amazonaws.acm#HostedZoneId`, unanchored, as a clause reports
/// it.
const HOSTED_ZONE_ID_PATTERN: &str = "Z[A-Z0-9]+";

/// How a clause reads at a position whose value ACM withholds.
const WITHHOLDING_SUBJECT: &str = "Value of the input at";

/// How the validating walk renders a value it has no text for: a list's element count
/// (`<n> items`) and, past the echo cap, a prefix plus `... [<n> bytes]`. Neither is
/// anything the caller submitted, so neither may appear in a message.
const WALK_RENDERINGS: [&str; 2] = [" items", " bytes]"];

/// One subject row: what it exercises, the operation, the request body, the member path
/// the clause names, the requirements it states in order, and the text the request
/// submitted there — which no clause may quote.
struct SubjectCase<'a> {
    what: &'a str,
    operation: &'a str,
    body: Value,
    path: &'a str,
    requirements: Vec<String>,
    submitted: Option<String>,
}

/// A syntactically well-formed ACM certificate ARN, for a row whose subject is some
/// OTHER member.
fn well_formed_certificate_arn() -> String {
    format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/12345678-1234-1234-1234-123456789012")
}

/// The AWS envelope over one clause per requirement, all of them about `subject`.
fn envelope(subject: &str, requirements: &[String]) -> String {
    let clauses: Vec<String> = requirements
        .iter()
        .map(|requirement| format!("{subject} failed to satisfy constraint: {requirement}"))
        .collect();
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        ),
    }
}

/// A `CreateAcmeDomainValidation` body naming `endpoint`, with `options` as its
/// `PrevalidationOptions.DnsPrevalidation`.
fn prevalidation_body(endpoint: &str, domain: &str, options: Value) -> Value {
    json!({
        "AcmeEndpointArn": endpoint,
        "DomainName": domain,
        "PrevalidationOptions": { "DnsPrevalidation": options },
    })
}

/// The rows the subject rule is asserted over.
fn subject_cases(endpoint: &str) -> Vec<SubjectCase<'static>> {
    let certificate_arn = well_formed_certificate_arn();
    // A value satisfying `@pattern ^Z[A-Z0-9]+$` but one character past the maximum.
    let too_long_zone = format!("Z{}", "A".repeat(HOSTED_ZONE_ID_MAX));
    // A single-label domain name far past `DomainName`'s maximum AND past the walk's
    // 256-byte echo cap, so the walk holds only an elision of it.
    let huge_domain = "a".repeat(DOMAIN_NAME_MAX * 4);
    let min_length = |min: i64| format!("Member must have length greater than or equal to {min}");
    let max_length = |max: i64| format!("Member must have length less than or equal to {max}");
    let pattern = |expression: &str| {
        format!("Member must satisfy regular expression pattern: {expression}")
    };

    vec![
        // The position the issue recorded: a plain string member of a structure under a
        // UNION member, submitted EMPTY, which trips both of its constraints — so the
        // row also pins that no clause of a many-clause envelope quotes the value.
        SubjectCase {
            what: "an empty hostedZoneId, under its minimum length and outside its pattern",
            operation: "CreateAcmeDomainValidation",
            body: prevalidation_body(endpoint, "empty.example.com", json!({ "HostedZoneId": "" })),
            path: "prevalidationOptions.dnsPrevalidation.hostedZoneId",
            requirements: vec![min_length(1), pattern(HOSTED_ZONE_ID_PATTERN)],
            submitted: None,
        },
        SubjectCase {
            what: "a hostedZoneId of a legal length but outside its pattern",
            operation: "CreateAcmeDomainValidation",
            body: prevalidation_body(
                endpoint,
                "lower.example.com",
                json!({ "HostedZoneId": "zone-1" }),
            ),
            path: "prevalidationOptions.dnsPrevalidation.hostedZoneId",
            requirements: vec![pattern(HOSTED_ZONE_ID_PATTERN)],
            submitted: Some("zone-1".to_string()),
        },
        SubjectCase {
            what: "a hostedZoneId one character past its maximum length",
            operation: "CreateAcmeDomainValidation",
            body: prevalidation_body(
                endpoint,
                "long.example.com",
                json!({ "HostedZoneId": too_long_zone.clone() }),
            ),
            path: "prevalidationOptions.dnsPrevalidation.hostedZoneId",
            requirements: vec![max_length(HOSTED_ZONE_ID_MAX as i64)],
            submitted: Some(too_long_zone),
        },
        // A different operation, a different constraint kind: an integer member.
        SubjectCase {
            what: "an integer member below its range minimum",
            operation: "ListCertificates",
            body: json!({ "MaxItems": 0 }),
            path: "maxItems",
            requirements: vec!["Member must have value greater than or equal to 1".to_string()],
            submitted: Some("0".to_string()),
        },
        // A collection ELEMENT: the clause is reported at the COLLECTION, and states the
        // element shape's whole constraint set (the recorded `tagKeys` envelope in
        // [`crate::pending_validation_and_tag_envelopes`]).
        SubjectCase {
            what: "a list element outside its own pattern",
            operation: "UntagResource",
            body: json!({ "ResourceArn": certificate_arn, "TagKeys": ["{nope}"] }),
            path: "tagKeys",
            requirements: vec!["Member must satisfy constraint: [Member must have length less \
                                than or equal to 128, Member must have length greater than or \
                                equal to 1, Member must satisfy regular expression pattern: \
                                ([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*), Member must not be null]"
                .to_string()],
            submitted: Some("{nope}".to_string()),
        },
        // A resource identifier, on a THIRD ARN shape beside the `Arn` and `PcaArn` the
        // other suite covers.
        SubjectCase {
            what: "a malformed acmeEndpointArn",
            operation: "CreateAcmeDomainValidation",
            body: prevalidation_body(
                "arn:aws:acm:us-west-2:000000000000:not-an-endpoint",
                "arn.example.com",
                json!({ "HostedZoneId": "Z2FDTNDATAQYW2" }),
            ),
            path: "acmeEndpointArn",
            requirements: vec![pattern(
                "arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+",
            )],
            submitted: None,
        },
        // A request token.
        SubjectCase {
            what: "an idempotencyToken outside its pattern",
            operation: "PutAccountConfiguration",
            body: json!({ "IdempotencyToken": "token-with-dashes" }),
            path: "idempotencyToken",
            requirements: vec![pattern("\\w+")],
            submitted: Some("token-with-dashes".to_string()),
        },
        // A COLLECTION, whose only rendering is its element count.
        SubjectCase {
            what: "an empty list, under the collection's own minimum length",
            operation: "UntagResource",
            body: json!({ "ResourceArn": certificate_arn, "TagKeys": [] }),
            path: "tagKeys",
            requirements: vec![min_length(1)],
            submitted: None,
        },
        // A value far past the walk's echo cap, which holds only an elision of it.
        SubjectCase {
            what: "a domain name far past the walk's echo cap",
            operation: "CreateAcmeDomainValidation",
            body: prevalidation_body(
                endpoint,
                &huge_domain,
                json!({ "HostedZoneId": "Z2FDTNDATAQYW2" }),
            ),
            path: "domainName",
            requirements: vec![max_length(DOMAIN_NAME_MAX as i64)],
            submitted: Some(huge_domain),
        },
    ]
}

/// RULE: a constraint clause names the position that failed and the requirement it
/// failed, and never the value the request submitted there — nor any rendering the
/// validating walk holds in its place.
#[tokio::test]
async fn a_clause_names_the_position_and_never_the_submitted_value() {
    let router = router();
    let endpoint = create_endpoint(&router, "the subject rule").await;

    for case in subject_cases(&endpoint) {
        let SubjectCase {
            what,
            operation,
            body,
            path,
            requirements,
            submitted,
        } = case;
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {what:?} ({operation}): expected 400 ValidationException, got {status} \
             {response}"
        );
        let stated = error_message(&response);
        assert_eq!(
            stated,
            envelope(&format!("{WITHHOLDING_SUBJECT} '{path}'"), &requirements),
            "case {what:?} ({operation}): expected the ACM constraint envelope, got {stated:?}"
        );
        // The clause states no rendering of the value at all: not the text submitted
        // (where there is any to confuse it with), not the walk's count summary, and
        // not its elision marker.
        if let Some(submitted) = submitted.filter(|text| !text.is_empty()) {
            assert!(
                !stated.contains(&submitted),
                "case {what:?} ({operation}): the submitted value {submitted:?} must not be \
                 stated, got {stated:?}"
            );
        }
        for rendering in WALK_RENDERINGS {
            assert!(
                !stated.contains(rendering),
                "case {what:?} ({operation}): a withheld value must not be stated as the \
                 walk's own rendering {rendering:?}, got {stated:?}"
            );
        }
    }
}

/// One reserved-tag row: what it exercises, the operation, the request members beside
/// the resource ARN, the modelled error code, and the half and text the message names.
type ReservedCase<'a> = (&'a str, &'a str, Value, &'a str, &'a str, &'a str);

/// RULE: a tag whose key or value begins with the `aws:` prefix AWS reserves is refused
/// with a fixed template naming the offending half of the pair and quoting its text.
///
/// The rows span both halves of a pair, several reserved keys (the bare prefix, a
/// service-qualified key, a key the issue did not cite), a reserved pair beside an
/// allowed one, the key-addressed removal path that carries no value at all, and the
/// four operations that take tags from a request — each with the error code that
/// operation models for it.
#[tokio::test]
async fn a_reserved_tag_is_refused_with_the_quoted_offending_half() {
    let router = router();
    let endpoint = create_endpoint(&router, "reserved tags").await;
    let certificate = request_certificate(
        &router,
        &json!({ "DomainName": "reserved.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    let rows: Vec<ReservedCase<'static>> = vec![
        (
            "a reserved key on AddTagsToCertificate",
            "AddTagsToCertificate",
            json!({
                "CertificateArn": certificate,
                "Tags": [{ "Key": "aws:reserved", "Value": "nope" }],
            }),
            "InvalidTagException",
            "Key",
            "aws:reserved",
        ),
        (
            "the bare reserved prefix as a key",
            "AddTagsToCertificate",
            json!({
                "CertificateArn": certificate,
                "Tags": [{ "Key": "aws:", "Value": "nope" }],
            }),
            "InvalidTagException",
            "Key",
            "aws:",
        ),
        (
            "a service-qualified reserved key, beside an allowed pair",
            "AddTagsToCertificate",
            json!({
                "CertificateArn": certificate,
                "Tags": [
                    { "Key": "allowed", "Value": "yes" },
                    { "Key": "aws:cloudformation:stack-name", "Value": "stack" },
                ],
            }),
            "InvalidTagException",
            "Key",
            "aws:cloudformation:stack-name",
        ),
        (
            "a reserved VALUE on AddTagsToCertificate",
            "AddTagsToCertificate",
            json!({
                "CertificateArn": certificate,
                "Tags": [{ "Key": "owner", "Value": "aws:internal" }],
            }),
            "InvalidTagException",
            "Value",
            "aws:internal",
        ),
        (
            "a reserved key on RemoveTagsFromCertificate",
            "RemoveTagsFromCertificate",
            json!({
                "CertificateArn": certificate,
                "Tags": [{ "Key": "aws:reserved" }],
            }),
            "InvalidTagException",
            "Key",
            "aws:reserved",
        ),
        (
            "a reserved key on TagResource",
            "TagResource",
            json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": "aws:owner", "Value": "nope" }],
            }),
            "ValidationException",
            "Key",
            "aws:owner",
        ),
        (
            "a reserved value on TagResource",
            "TagResource",
            json!({
                "ResourceArn": endpoint,
                "Tags": [{ "Key": "stage", "Value": "aws:prod" }],
            }),
            "ValidationException",
            "Value",
            "aws:prod",
        ),
        (
            "a reserved key on the key-addressed removal path",
            "UntagResource",
            json!({ "ResourceArn": endpoint, "TagKeys": ["env", "aws:owner"] }),
            "ValidationException",
            "Key",
            "aws:owner",
        ),
    ];

    for (what, operation, body, code, half, offending) in rows {
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, code),
            "case {what:?}: expected 400 {code}, got {status} {response}"
        );
        let stated = error_message(&response);
        let expected = format!(
            "Invalid Tag {half}: \"{offending}\". AWS internal tags cannot be changed with this \
             API."
        );
        assert_eq!(
            stated, expected,
            "case {what:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // No refused call wrote: the endpoint still carries nothing.
    let tags = stored_tags(&router, "reserved tags", &endpoint).await;
    assert!(
        tags.is_empty(),
        "expected every reserved-tag refusal to leave the endpoint untagged, got {tags:?}"
    );
}

/// RULE: a request that would leave a resource carrying more than the modelled
/// `TagList` maximum of user tags is refused with the tagging layer's constant sentence
/// for that quota, whichever modelled error the operation carries it on — and the
/// sentence states the quota alone, never the count the request would have reached.
///
/// The rows are the two tag-writing paths, which model DIFFERENT errors for the one
/// quota: the generic resource path (`ServiceQuotaExceededException`) and the
/// certificate path (`TooManyTagsException`). Each is driven to the bound first, so the
/// refusal is the quota rather than the request's own `@length` bound.
#[tokio::test]
async fn the_tag_quota_refusal_states_the_quota_sentence() {
    let router = router();
    let expected =
        format!("The resultant tag set must not have more than {TAG_LIST_MAX} user tags.");

    let endpoint = create_endpoint(&router, "the tag quota").await;
    let certificate = request_certificate(
        &router,
        &json!({ "DomainName": "quota.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let at_bound: Value = Value::Array(
        (0..TAG_LIST_MAX)
            .map(|index| json!({ "Key": format!("key{index:02}"), "Value": "v" }))
            .collect(),
    );
    let one_more = json!([{ "Key": "one-too-many", "Value": "v" }]);

    // (what, the operation, the body that fills the quota, the body that exceeds it,
    //  the code that operation models for the quota)
    let rows: Vec<(&str, &str, Value, Value, &str)> = vec![
        (
            "the generic resource path",
            "TagResource",
            json!({ "ResourceArn": endpoint, "Tags": at_bound }),
            json!({ "ResourceArn": endpoint, "Tags": one_more }),
            "ServiceQuotaExceededException",
        ),
        (
            "the certificate path",
            "AddTagsToCertificate",
            json!({ "CertificateArn": certificate, "Tags": at_bound }),
            json!({ "CertificateArn": certificate, "Tags": one_more }),
            "TooManyTagsException",
        ),
    ];

    for (what, operation, fill, exceed, code) in rows {
        let (status, response) = call(&router, operation, &fill).await;
        assert_eq!(
            status, 200,
            "case {what:?}: filling the quota must succeed, got {status} {response}"
        );
        let (status, response) = call(&router, operation, &exceed).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, code),
            "case {what:?}: expected 400 {code}, got {status} {response}"
        );
        let stated = error_message(&response);
        assert_eq!(
            stated, expected,
            "case {what:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The refused writes left the resource at the bound, so the sentence is about a
    // quota that really was enforced.
    let tags = stored_tags(&router, "the tag quota", &endpoint).await;
    assert_eq!(
        tags.len(),
        TAG_LIST_MAX,
        "expected the endpoint to stay at the {TAG_LIST_MAX}-tag bound, got {} tags",
        tags.len()
    );
}
