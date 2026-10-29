// SPDX-License-Identifier: CC0-1.0

//! CloudFront's resource-policy surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: the surface attaches a resource policy to ONE
//! CloudFront resource type ([`RESOURCE_POLICY_RESOURCE_TYPE`]) and addresses that policy
//! only by the ARN of the resource it is attached to, so
//!
//! * a `ResourceArn` that is no ARN at all — a value that does not open with `arn:` — is
//!   refused on its FORM: HTTP 400 `InvalidArgument` with the fixed sentence
//!   [`MALFORMED_ARN_MESSAGE`], since the value cannot be read as an ARN in the first
//!   place;
//! * a `ResourceArn` that IS an ARN but names anything else — an ARN of another service, an
//!   ARN of another CloudFront resource type, an ARN carrying no resource id — is refused on
//!   the resource TYPE it names: HTTP 400 `InvalidArgument`, decided before anything is
//!   written or looked up and whether or not a resource of that type exists. The CONDITION
//!   is one for both operations; the SENTENCE is the one each puts on the wire — the write
//!   names the resource type it wants ([`INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE`]),
//!   the read demands a usable ARN ([`INVALID_RESOURCE_POLICY_ARN_MESSAGE`]);
//! * an ARN the surface does accept round-trips: a read answers with the document the last
//!   write persisted under that ARN, byte for byte, and with that ARN;
//! * an accepted ARN carrying no policy is `EntityNotFound` (404) — the EXISTENCE answer,
//!   which is what distinguishes it from the parameter refusal above.
//!
//! The rows differ along what the rule generalises over: the resource type the ARN names
//! (every type this service mints an ARN for, and two the surface would never see), the
//! ARN's partition, account and region segments, whether a resource of that type exists,
//! the form and length of the resource id, the content and size of the policy document
//! (including text that only survives XML escaping), how many times the same ARN is
//! written, and which of the two operations asks.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    vpc_origin_arn, INVALID_RESOURCE_POLICY_ARN_MESSAGE,
    INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE, MALFORMED_ARN_MESSAGE,
    RESOURCE_POLICY_NOT_FOUND_MESSAGE, RESOURCE_POLICY_RESOURCE_TYPE, TAGGABLE_RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, create_distribution, emulator, Answer, ACCOUNT_ID, API_VERSION};
use crate::distribution_tenant_lifecycle::escape;

/// The CloudFront resource types that are NOT the one the resource-policy surface attaches
/// a policy to, beyond the taggable ones [`TAGGABLE_RESOURCE_TYPES`] already names.
///
/// Read off the model: each is a resource this service addresses by its own route
/// (`/2020-05-31/cache-policy/{Id}`, `/2020-05-31/origin-request-policy/{Id}`,
/// `/2020-05-31/response-headers-policy/{Id}`, `/2020-05-31/public-key/{Id}`,
/// `/2020-05-31/origin-access-control/{Id}` and
/// `/2020-05-31/continuous-deployment-policy/{Id}`), so its ARN spells that type the same
/// way (`smithy_query(query="operation", operation="GetCachePolicy,…")`).
pub(crate) const OTHER_RESOURCE_TYPES: [&str; 6] = [
    "cache-policy",
    "continuous-deployment-policy",
    "origin-access-control",
    "origin-request-policy",
    "public-key",
    "response-headers-policy",
];

/// The error message the answer states.
pub(crate) fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

/// The policy document the answer reports, or `""` when it carries none.
pub(crate) fn reported_document(answer: &Answer) -> &str {
    answer.root().find_text("PolicyDocument").unwrap_or_default()
}

/// The resource ARN the answer reports, or `""` when it carries none.
fn reported_arn(answer: &Answer) -> &str {
    answer.root().find_text("ResourceArn").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/put-resource-policy`, attaching `document` to `arn`.
pub(crate) async fn put_resource_policy(
    router: &MultiServiceRouter,
    arn: &str,
    document: &str,
) -> Answer {
    let body = format!(
        "<PutResourcePolicyRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <ResourceArn>{}</ResourceArn>\
         <PolicyDocument>{}</PolicyDocument>\
         </PutResourcePolicyRequest>",
        escape(arn),
        escape(document)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/put-resource-policy"),
        Some(&body),
        None,
    )
    .await
}

/// `POST /2020-05-31/get-resource-policy`, asking about `arn`.
pub(crate) async fn get_resource_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    let body = format!(
        "<GetResourcePolicyRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <ResourceArn>{}</ResourceArn>\
         </GetResourcePolicyRequest>",
        escape(arn)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/get-resource-policy"),
        Some(&body),
        None,
    )
    .await
}

/// A minimal `DistributionConfig`, so one row of the refusal table can name a CloudFront
/// resource that EXISTS and is of a type the surface does not attach policies to.
fn distribution_config(reference: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{reference}</Comment><Enabled>true</Enabled></DistributionConfig>"
    )
}

/// A resource policy document, as a caller would write one.
pub(crate) fn policy(statement: &str) -> String {
    format!(
        "{{\"Version\":\"2008-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::111122223333:root\"}},\
         \"Action\":\"cloudfront:*\",\"Sid\":\"{statement}\"}}]}}"
    )
}

// ---------------------------------------------------------------------------
// The parameter gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_no_vpc_origin_is_an_invalid_argument() {
    let router = emulator();

    // The row the rule has to hold for even though the resource EXISTS: a live
    // distribution, read back through its own ARN.
    let created = create_distribution(&router, &distribution_config("ref-resource-policy")).await;
    assert_eq!(
        created.status, 201,
        "the distribution must be created for its row to be about its TYPE, got {} ({})",
        created.status, created.raw
    );
    let distribution_arn = created
        .root()
        .find_text("ARN")
        .unwrap_or_default()
        .to_string();
    assert!(
        distribution_arn.contains(":distribution/"),
        "expected a distribution ARN to name the existing resource, got {distribution_arn:?}"
    );

    // Every other resource type this service mints an ARN for, each with an ARN of the
    // same shape as the accepted one — so what refuses each row is the TYPE it names.
    let mut rows: Vec<(String, String)> = TAGGABLE_RESOURCE_TYPES
        .iter()
        .chain(OTHER_RESOURCE_TYPES.iter())
        .map(|kind| {
            (
                format!("an ARN naming a {kind}"),
                format!("arn:aws:cloudfront::{ACCOUNT_ID}:{kind}/absent-resource"),
            )
        })
        .collect();
    rows.extend([
        (
            "the ARN of a distribution that exists".to_string(),
            distribution_arn.clone(),
        ),
        (
            "a bare ARN prefix".to_string(),
            "arn:".to_string(),
        ),
        (
            "an ARN carrying no resource part".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:"),
        ),
        (
            "an ARN of another service naming the accepted type".to_string(),
            format!("arn:aws:ec2:us-east-1:{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "an ARN with no partition".to_string(),
            format!("arn::cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "an ARN whose resource type merely starts with the accepted one".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}-group/vo-1"),
        ),
        (
            "an ARN naming the accepted type with no id".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}"),
        ),
        (
            "an ARN naming the accepted type with an empty id".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/"),
        ),
    ]);

    for (case, arn) in &rows {
        // The CONDITION is one for both operations; the SENTENCE each answers it with is the
        // one that operation puts on the wire — the write names the resource type it wants,
        // the read demands a usable ARN.
        let answers = [
            (
                "PutResourcePolicy",
                put_resource_policy(&router, arn, &policy("gate")).await,
                INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE,
            ),
            (
                "GetResourcePolicy",
                get_resource_policy(&router, arn).await,
                INVALID_RESOURCE_POLICY_ARN_MESSAGE,
            ),
        ];
        for (operation, answer, expected) in answers {
            assert_eq!(
                answer.status, 400,
                "case {case}/{operation}: expected HTTP 400, got {} ({})",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}/{operation}: expected InvalidArgument, got {:?} ({})",
                answer.error_code(),
                answer.raw
            );
            assert_eq!(
                message(&answer),
                expected,
                "case {case}/{operation}: expected message {expected:?}, got {:?}",
                message(&answer)
            );
        }
    }

    // The refusals wrote nothing: an accepted ARN still carries no policy, so no refused
    // write landed under any key.
    let accepted = vpc_origin_arn("vo-after-the-refusals");
    let read = get_resource_policy(&router, &accepted).await;
    assert_eq!(
        read.status, 404,
        "case after the refusals: expected HTTP 404, got {} ({})",
        read.status, read.raw
    );
}

// ---------------------------------------------------------------------------
// A value that is no ARN at all
// ---------------------------------------------------------------------------

/// A `ResourceArn` that does not open with `arn:` cannot be READ as an ARN, so both
/// operations of the surface refuse it on its FORM: HTTP 400 `InvalidArgument` with
/// [`MALFORMED_ARN_MESSAGE`], which states what the parameter has to be rather than the
/// resource type it names. Nothing is written by the refusal.
///
/// The rows span the forms a value that is no ARN takes: empty, a bare word, the resource
/// part alone, the accepted ARN with its prefix missing or misspelled (including the one
/// case difference, since the prefix is case-sensitive), a URL, and a value that merely
/// CONTAINS an ARN.
#[tokio::test]
async fn a_resource_arn_that_is_no_arn_is_refused_on_its_form() {
    let router = emulator();
    let accepted = vpc_origin_arn("vo-form");

    let rows: [(&str, String); 8] = [
        ("the empty string", String::new()),
        ("a bare word", "not-an-arn".to_string()),
        (
            "the resource part alone",
            format!("{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "the accepted ARN with the prefix stripped",
            accepted
                .strip_prefix("arn:")
                .unwrap_or_default()
                .to_string(),
        ),
        (
            "the accepted ARN with the prefix in upper case",
            format!("ARN:{}", accepted.strip_prefix("arn:").unwrap_or_default()),
        ),
        (
            "the accepted ARN with the prefix misspelled",
            format!("arn{}", accepted.strip_prefix("arn:").unwrap_or_default()),
        ),
        (
            "a URL rather than an ARN",
            format!("https://cloudfront.amazonaws.com/{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "a value that merely contains an ARN",
            format!("resource={accepted}"),
        ),
    ];

    for (case, value) in &rows {
        let answers = [
            (
                "PutResourcePolicy",
                put_resource_policy(&router, value, &policy("form")).await,
            ),
            (
                "GetResourcePolicy",
                get_resource_policy(&router, value).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, 400,
                "case {case}/{operation}: expected HTTP 400 for {value:?}, got {} ({})",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}/{operation}: expected InvalidArgument for {value:?}, got {:?} ({})",
                answer.error_code(),
                answer.raw
            );
            assert_eq!(
                message(&answer),
                MALFORMED_ARN_MESSAGE,
                "case {case}/{operation}: expected message {MALFORMED_ARN_MESSAGE:?}, got {:?}",
                message(&answer)
            );
        }
    }

    // The refusals wrote nothing under any key the surface does address.
    let read = get_resource_policy(&router, &accepted).await;
    assert_eq!(
        read.status, 404,
        "case after the form refusals: expected HTTP 404, got {} ({})",
        read.status, read.raw
    );
}

// ---------------------------------------------------------------------------
// The absent policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_accepted_arn_carrying_no_policy_is_entity_not_found() {
    let router = emulator();

    // Rows differing along the ARN's segments and the form of the resource id: each is an
    // ARN the surface accepts, none of them has ever been written.
    let rows = [
        (
            "the ARN this service mints",
            vpc_origin_arn("vo-never-written"),
        ),
        (
            "an id in the other partition",
            format!("arn:aws-cn:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-cn"),
        ),
        (
            "an ARN carrying a region segment",
            format!(
                "arn:aws:cloudfront:us-east-1:{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-regional"
            ),
        ),
        (
            "another account's resource",
            format!("arn:aws:cloudfront::111122223333:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-other"),
        ),
    ];

    for (case, arn) in &rows {
        let answer = get_resource_policy(&router, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            message(&answer),
            RESOURCE_POLICY_NOT_FOUND_MESSAGE,
            "case {case}: expected message {RESOURCE_POLICY_NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&answer)
        );
    }
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_policy_written_under_an_accepted_arn_reads_back() {
    let router = emulator();

    // Rows differing along the form and length of the resource id, the ARN's partition,
    // region and account segments, and the content and size of the document — including
    // text that only survives XML escaping and a document far larger than the others.
    let long_id = format!("vo-{}", "0123456789".repeat(6));
    let escaping = "{\"Version\":\"2008-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                    \"Condition\":{\"StringLike\":{\"aws:SourceArn\":\"<a & b> 'c' \\\"d\\\"\"}}}]}";
    let bulky = format!(
        "{{\"Version\":\"2008-10-17\",\"Statement\":[{}]}}",
        (0..40)
            .map(|index| format!(
                "{{\"Effect\":\"Allow\",\"Sid\":\"statement{index}\",\"Action\":\"cloudfront:*\"}}"
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the ARN this service mints",
            vpc_origin_arn("vo_abc123"),
            policy("minted"),
        ),
        (
            "an upper-case id",
            vpc_origin_arn("VO-UPPER-CASE-1"),
            policy("upper"),
        ),
        ("a one-character id", vpc_origin_arn("1"), policy("short")),
        (
            "an id of sixty-three characters",
            vpc_origin_arn(&long_id),
            policy("long-id"),
        ),
        (
            "the other partition",
            format!("arn:aws-cn:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-cn"),
            policy("partition"),
        ),
        (
            "a region segment and another account",
            format!(
                "arn:aws:cloudfront:us-east-1:111122223333:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-regional"
            ),
            policy("regional"),
        ),
        (
            "a document carrying text that needs escaping",
            vpc_origin_arn("vo-escaping"),
            escaping.to_string(),
        ),
        (
            "a document of forty statements",
            vpc_origin_arn("vo-bulky"),
            bulky.clone(),
        ),
        (
            "an empty JSON object",
            vpc_origin_arn("vo-empty-object"),
            "{}".to_string(),
        ),
    ];

    for (case, arn, document) in &rows {
        let written = put_resource_policy(&router, arn, document).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected HTTP 200 from the write, got {} ({})",
            written.status, written.raw
        );
        assert_eq!(
            reported_arn(&written),
            arn.as_str(),
            "case {case}: expected the write to report {arn:?}, got {:?}",
            reported_arn(&written)
        );

        let read = get_resource_policy(&router, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the read, got {} ({})",
            read.status, read.raw
        );
        assert_eq!(
            reported_document(&read),
            document.as_str(),
            "case {case}: expected the document {document:?}, got {:?}",
            reported_document(&read)
        );
        assert_eq!(
            reported_arn(&read),
            arn.as_str(),
            "case {case}: expected the read to report {arn:?}, got {:?}",
            reported_arn(&read)
        );
    }

    // Every row is still readable after every other row was written: a policy is keyed by
    // the ARN it was attached to, so no write displaced another resource's policy.
    for (case, arn, document) in &rows {
        let read = get_resource_policy(&router, arn).await;
        assert_eq!(
            reported_document(&read),
            document.as_str(),
            "case {case} re-read: expected the document {document:?}, got {:?}",
            reported_document(&read)
        );
    }

    // A read is a REPORT: asking twice answers the same, and asking never consumes the
    // policy.
    let (_, first_arn, first_document) = &rows[0];
    let again = get_resource_policy(&router, first_arn).await;
    assert_eq!(
        reported_document(&again),
        first_document.as_str(),
        "case the first row read a third time: expected the document {first_document:?}, got {:?}",
        reported_document(&again)
    );

    // An ARN no write named carries no policy, even though other ARNs do.
    let unwritten = vpc_origin_arn("vo-unwritten");
    let absent = get_resource_policy(&router, &unwritten).await;
    assert_eq!(
        absent.status, 404,
        "case an ARN no write named: expected HTTP 404, got {} ({})",
        absent.status, absent.raw
    );
    assert_eq!(
        absent.error_code(),
        "EntityNotFound",
        "case an ARN no write named: expected EntityNotFound, got {:?} ({})",
        absent.error_code(),
        absent.raw
    );
}

#[tokio::test]
async fn a_second_write_supersedes_the_policy_the_resource_carried() {
    let router = emulator();
    let arn = vpc_origin_arn("vo-superseded");

    // The write is a PUT: each of these documents replaces the one before it, so the read
    // after each write answers with that write's document and nothing accumulates.
    let documents = [
        policy("first"),
        policy("second"),
        "{}".to_string(),
        policy("last"),
    ];
    for (index, document) in documents.iter().enumerate() {
        let written = put_resource_policy(&router, &arn, document).await;
        assert_eq!(
            written.status, 200,
            "case write {index}: expected HTTP 200, got {} ({})",
            written.status, written.raw
        );
        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            reported_document(&read),
            document.as_str(),
            "case write {index}: expected the document {document:?}, got {:?}",
            reported_document(&read)
        );
    }

    // And a refused write leaves the policy the resource already carried untouched: the
    // parameter gate decides before anything is written.
    let refused = put_resource_policy(
        &router,
        &format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E1REFUSED"),
        &policy("refused"),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case the refused write: expected HTTP 400, got {} ({})",
        refused.status, refused.raw
    );
    let read = get_resource_policy(&router, &arn).await;
    let expected = documents.last().expect("the document table is not empty");
    assert_eq!(
        reported_document(&read),
        expected.as_str(),
        "case after the refused write: expected the document {expected:?}, got {:?}",
        reported_document(&read)
    );
}
