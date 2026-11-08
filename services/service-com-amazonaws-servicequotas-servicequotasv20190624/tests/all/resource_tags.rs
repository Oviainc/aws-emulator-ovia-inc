// SPDX-License-Identifier: CC0-1.0

//! How Service Quotas WORDS the refusal of a `ResourceARN` it will not accept as a
//! value — the message a tagging call gets back, as opposed to the tag-set behaviour
//! (`tags.rs`) and the status-and-code envelope (`error_envelope.rs`).
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, and each assertion reads the whole body: the `__type` AND the
//! `Message` member.
//!
//! The rule, asserted over a TABLE of inputs rather than the one recorded case: this
//! service runs THREE validators, and which one answers is a property of the MEMBER,
//! not of the operation or of the value (`src/service.rs`:
//! `FRONT_END_VALIDATED_MEMBERS`, `SERVICE_OWN_VALIDATED_MEMBERS`).
//!
//! * `ResourceARN` is checked by the service's own application layer, which states ONE
//!   sentence for an illegal value and names neither the member nor the rule —
//!   recorded for an ARN outside its `@pattern`, and asserted here for every
//!   constraint that member carries AND for every value that layer itself refuses (a
//!   string inside the modelled pattern and length that is still not an ARN of one of
//!   this service's applied quotas), on all three tagging operations;
//! * an ABSENT `ResourceARN` is not an illegal value and keeps the member-and-rule
//!   clause, since there was no value for that layer to check;
//! * every other member keeps its own validator's wording: the modelled
//!   `IllegalArgumentException` with the member-and-rule clause, or
//!   `ValidationException` with the request validator's `Value at '<path>'` clause for
//!   the members the front end owns.
//!
//! The harness is the crate's shared one: `quota_workflow`'s `Client` (the same
//! `MultiServiceRouter` every suite speaks to) and `error_envelope`'s `error_of`, which
//! reads the code and whichever message member the answering validator wrote — so this
//! suite adds assertions, not a third client.

use serde_json::{json, Value};

use super::error_envelope::error_of;
use super::quota_workflow::{client, emulator, Client, ACCOUNT};

/// Service Quotas speaks `awsJson1_1`, so every modelled client error goes out on the
/// status of its FAULT (see
/// `service_com_amazonaws_servicequotas_servicequotasv20190624::scaffolded::wire_http_status`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The recorded wire answer for a `ResourceARN` this service will not accept as a
/// value (corpus unit
/// `service-quotas.list-tags-for-resource-invalid-arn-pattern-rejected`): the ARN is
/// checked by the service's application layer, which names neither the member nor the
/// constraint.
const ILLEGAL_VALUE_MESSAGE: &str =
    "The request failed because you provided an illegal value for a parameter.";

/// `AmazonResourceName` is `@length(min: 1, max: 1011)` with the `@pattern`
/// `^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$`
/// (`smithy_query(query="shape", shape="AmazonResourceName")`).
const ARN_MAX_LENGTH: usize = 1011;

/// A caller on an emulator of its own, in `region`.
fn caller(region: &str) -> Client {
    client(&emulator(), region)
}

/// Dispatch one call that must be refused and report its status, error code and the
/// message member the answering validator wrote.
async fn refused(client: &Client, operation: &str, body: Value) -> (u16, String, Option<String>) {
    let (status, parsed) = client.call(operation, &body).await;
    let (code, message) = error_of(&parsed);
    (status, code, message)
}

/// Every tagging call that carries a `ResourceARN`, for one value of it: the rule is a
/// property of the member, so it is asserted on all three operations.
fn tagging_calls(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "ListTagsForResource",
            json!({"ResourceARN": resource_arn}),
        ),
        (
            "TagResource",
            json!({"ResourceARN": resource_arn, "Tags": [{"Key": "Environment", "Value": "prod"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["Environment"]}),
        ),
    ]
}

/// A `ResourceARN` this service will not accept as a value is refused with the
/// modelled `IllegalArgumentException` carrying the service's OWN illegal-value
/// sentence — whichever constraint the value broke, on every tagging operation.
#[tokio::test]
async fn an_illegal_resource_arn_is_refused_in_the_services_own_words() {
    let client = caller("us-east-1");
    let prefix = format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:vpc/L-");
    let past_max = format!("{prefix}{}", "A".repeat(ARN_MAX_LENGTH + 1 - prefix.len()));

    // Rows: one per way the value breaks a constraint the member carries — its
    // `@pattern` in three forms, its `@length` minimum, and its `@length` maximum.
    let rows: Vec<(&str, String)> = vec![
        ("not an ARN at all", "not-an-arn".to_string()),
        (
            "an ARN with no account segment",
            "arn:aws:servicequotas:us-east-1::vpc/L-F678F1CE".to_string(),
        ),
        (
            "an account id that is not twelve digits",
            "arn:aws:servicequotas:us-east-1:1234:vpc/L-F678F1CE".to_string(),
        ),
        (
            "a single character, at the length minimum but outside the pattern",
            "x".to_string(),
        ),
        ("the empty string, below the length minimum", String::new()),
        ("one character past the length maximum", past_max),
    ];

    for (why, resource_arn) in rows {
        for (operation, body) in tagging_calls(&resource_arn) {
            let case = format!("{why} ({operation})");
            let (status, code, message) = refused(&client, operation, body).await;
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {case}: expected HTTP {CLIENT_ERROR_STATUS}, got {status} with {code:?}"
            );
            assert!(
                code.contains("IllegalArgumentException"),
                "case {case}: expected IllegalArgumentException, got {code:?} with {message:?}"
            );
            assert_eq!(
                message.as_deref(),
                Some(ILLEGAL_VALUE_MESSAGE),
                "case {case}: expected the service's own illegal-value sentence"
            );
        }
    }
}

/// A `ResourceARN` that satisfies every constraint the MODEL declares but is still not
/// a value this service will accept — it is not an ARN of one of its own applied quotas
/// — is refused in exactly the same words: the validator is a property of the member, so
/// the VALUE never decides the wording.
#[tokio::test]
async fn a_pattern_passing_resource_arn_that_is_not_a_quota_arn_is_refused_in_the_same_words() {
    let client = caller("us-east-1");

    // Rows: one per way a value inside `AmazonResourceName`'s `@pattern`
    // `^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$` and its length bounds is still not an
    // applied-quota ARN of this service — another service's `arnNamespace`, a namespace
    // spelled in another case, a segment count this service cannot read, and four
    // resource parts that are not `<ServiceCode>/<QuotaCode>`.
    let rows: Vec<(&str, String)> = vec![
        (
            "another service's arnNamespace",
            format!("arn:aws:s3:us-east-1:{ACCOUNT}:bucket/x"),
        ),
        (
            "this service's arnNamespace in another case",
            format!("arn:aws:ServiceQuotas:us-east-1:{ACCOUNT}:vpc/L-F678F1CE"),
        ),
        (
            "one segment short of an ARN this service can read",
            format!("arn:aws:servicequotas:{ACCOUNT}:vpc/L-F678F1CE"),
        ),
        (
            "a resource part carrying no quota code",
            format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:notaquota"),
        ),
        (
            "a resource part whose service code is empty",
            format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:/L-F678F1CE"),
        ),
        (
            "a resource part whose quota code is empty",
            format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:vpc/"),
        ),
        (
            "a resource part carrying a third slash-separated segment",
            format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:vpc/L-F678F1CE/extra"),
        ),
    ];

    for (why, resource_arn) in rows {
        for (operation, body) in tagging_calls(&resource_arn) {
            let case = format!("{why} ({operation})");
            let (status, code, message) = refused(&client, operation, body).await;
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {case}: expected HTTP {CLIENT_ERROR_STATUS}, got {status} with {code:?}"
            );
            assert!(
                code.contains("IllegalArgumentException"),
                "case {case}: expected IllegalArgumentException, got {code:?} with {message:?}"
            );
            assert_eq!(
                message.as_deref(),
                Some(ILLEGAL_VALUE_MESSAGE),
                "case {case}: expected the service's own illegal-value sentence, got {message:?}"
            );
            let message = message.unwrap_or_default();
            assert!(
                !message.contains("ResourceARN") && !message.contains(resource_arn.as_str()),
                "case {case}: the sentence names neither the member nor the value, got {message:?}"
            );
        }
    }
}

/// An ABSENT `ResourceARN` is not an illegal value: there was no value for the
/// service's own layer to check, so it keeps the member-and-rule clause every other
/// absent required member of this service is refused with.
#[tokio::test]
async fn an_absent_resource_arn_keeps_the_member_and_rule_clause() {
    let client = caller("us-east-1");

    // The same three operations, each with `ResourceARN` left out entirely.
    let rows: Vec<(&str, Value)> = vec![
        ("ListTagsForResource", json!({})),
        (
            "TagResource",
            json!({"Tags": [{"Key": "Environment", "Value": "prod"}]}),
        ),
        ("UntagResource", json!({"TagKeys": ["Environment"]})),
    ];

    for (operation, body) in rows {
        let case = format!("an absent ResourceARN ({operation})");
        let (status, code, message) = refused(&client, operation, body).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case}: expected HTTP {CLIENT_ERROR_STATUS}, got {status} with {code:?}"
        );
        assert!(
            code.contains("IllegalArgumentException"),
            "case {case}: expected IllegalArgumentException, got {code:?} with {message:?}"
        );
        assert_eq!(
            message.as_deref(),
            Some("ResourceARN failed to satisfy constraint: Member must not be null"),
            "case {case}: expected the member-and-rule clause"
        );
    }
}

/// A `ResourceARN` that IS a legal value is past that layer, so what answers is the
/// resource lookup: a different error shape, and a message built from the request
/// rather than the illegal-value sentence.
#[tokio::test]
async fn a_legal_resource_arn_that_names_nothing_is_answered_by_the_lookup() {
    let client = caller("us-east-1");
    let prefix = format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:vpc/L-");
    let at_max = format!("{prefix}{}", "A".repeat(ARN_MAX_LENGTH - prefix.len()));
    assert_eq!(
        at_max.len(),
        ARN_MAX_LENGTH,
        "case the length maximum: expected a {ARN_MAX_LENGTH}-character ARN, got {}",
        at_max.len()
    );

    // Rows: legal ARNs — the pattern holds and the length is within both bounds, one
    // of them exactly AT the maximum — that name no quota of this account. WHICH
    // lookups answer that way is `tags.rs`'s table; what is asserted here is the
    // message, so one ordinary row plus the row at the bound is enough.
    let rows: Vec<(&str, String)> = vec![
        (
            "a quota code the catalog does not hold",
            format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:vpc/L-00000000"),
        ),
        ("a quota code at the ARN length maximum", at_max),
    ];

    for (why, resource_arn) in rows {
        for (operation, body) in tagging_calls(&resource_arn) {
            let case = format!("{why} ({operation})");
            let (status, code, message) = refused(&client, operation, body).await;
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {case}: expected HTTP {CLIENT_ERROR_STATUS}, got {status} with {code:?}"
            );
            assert!(
                code.contains("NoSuchResourceException"),
                "case {case}: expected NoSuchResourceException, got {code:?} with {message:?}"
            );
            assert_ne!(
                message.as_deref(),
                Some(ILLEGAL_VALUE_MESSAGE),
                "case {case}: expected a message about the resource lookup"
            );
            assert!(
                message.as_deref().is_some_and(|text| !text.is_empty()),
                "case {case}: expected a non-empty Message member, got {message:?}"
            );
        }
    }
}

/// The split is per MEMBER: a member this service does not check itself keeps its own
/// validator's wording, so the illegal-value sentence never spreads to the rest of the
/// service.
#[tokio::test]
async fn every_other_member_keeps_its_own_validators_wording() {
    let client = caller("us-east-1");

    // Rows: the operation, a request with one member outside its constraints, the
    // error code that member's validator answers with, and the opening of the clause
    // it carries. The members and their constraints are the model's
    // (`smithy_query(query="operation", operation="GetServiceQuota,ListServiceQuotas,\
    // GetRequestedServiceQuotaChange")`); which validator owns which member is
    // recorded behaviour, listed in `src/service.rs`.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a quota code outside its pattern",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "1-bad code"}),
            "IllegalArgumentException",
            "QuotaCode failed to satisfy constraint: ",
        ),
        (
            "a service code outside its pattern",
            "ListServiceQuotas",
            json!({"ServiceCode": "1-not-a-service"}),
            "IllegalArgumentException",
            "ServiceCode failed to satisfy constraint: ",
        ),
        (
            "a page size outside its range",
            "ListServices",
            json!({"MaxResults": 0}),
            "IllegalArgumentException",
            "MaxResults failed to satisfy constraint: ",
        ),
        (
            // A member the request-validation FRONT END owns: another code, and
            // another clause — inside AWS's count envelope.
            "a request id outside its pattern",
            "GetRequestedServiceQuotaChange",
            json!({"RequestId": "-leading-dash"}),
            "ValidationException",
            "1 validation error detected: Value at 'requestId' failed to satisfy constraint: ",
        ),
    ];

    for (why, operation, body, expected_code, clause) in rows {
        let case = format!("{why} ({operation})");
        let (status, code, message) = refused(&client, operation, body).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case}: expected HTTP {CLIENT_ERROR_STATUS}, got {status} with {code:?}"
        );
        assert!(
            code.contains(expected_code),
            "case {case}: expected {expected_code}, got {code:?} with {message:?}"
        );
        let message = message.unwrap_or_else(|| {
            panic!("case {case}: expected a message member, got none");
        });
        assert!(
            message.starts_with(clause),
            "case {case}: expected a message starting {clause:?}, got {message:?}"
        );
        assert_ne!(
            message, ILLEGAL_VALUE_MESSAGE,
            "case {case}: the illegal-value sentence belongs to the resource ARN alone"
        );
    }
}
