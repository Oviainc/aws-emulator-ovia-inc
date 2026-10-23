// SPDX-License-Identifier: CC0-1.0

//! How a request this service's MODELLED CONSTRAINTS refuse is stated on the wire.
//!
//! The rule under test: a request that breaks a constraint the model declares is answered
//! `HTTP 400` `ValidationException` whose detail text — carried in the `Message` member,
//! which is how every error shape of this `awsJson1_1` model spells it — states ONE clause
//! per violation,
//!
//! ```text
//! Value of input '<member path>' failed to satisfy constraint: <requirement>
//! ```
//!
//! with several clauses joined by `"; "` in the REVERSE of the order the input shape
//! declares the members they name, and a `@pattern` requirement quoted as the service
//! publishes the expression — without the `^…$` pair the model wraps it in. The clause
//! names the member and the constraint it broke and does not quote the rejected value
//! back.
//!
//! The table spans the class the rule generalises over rather than the one recorded
//! request: every constraint class the model puts on this service's inputs (`@pattern`,
//! `@length` at its minimum, `@range` at both published bounds, and an absent `@required`
//! member), the same member refused from several operations, operations whose refusal
//! names TWO members, and the same two members submitted in both body orders — so the
//! message can only come from the rule and not from the request's own order. Each quoted
//! pattern is the member's modelled one, named in the constant that holds it
//! (`smithy_query(query="operation",operation="DescribeInstance,DeleteInstance,ListTagsForResource,TagResource,ListInstances",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the boundary validator, the
//! service's own refusal wording and the `awsJson1_1` error envelope are all exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};

use crate::ssoadmin_permission_sets::{
    call, call_raw, error_code, router, TEST_REGION, UNKNOWN_INSTANCE_ARN,
};

/// The `InstanceArn` pattern, as the model declares it (`InstanceArn`, used by every
/// operation's `InstanceArn` member).
const INSTANCE_ARN_PATTERN: &str =
    r"arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}";

/// The `TaggableResourceArn` pattern, as the model declares it (`ListTagsForResource` /
/// `TagResource`'s `ResourceArn`).
const TAGGABLE_RESOURCE_ARN_PATTERN: &str = r"arn:aws(-[a-z]{1,5}){0,3}:sso::((:instance/(sso)?ins-[a-zA-Z0-9-.]{16})|(:permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16})|(\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16})|(\d{12}:trustedTokenIssuer/(sso)?ins-[a-zA-Z0-9-.]{16}/tti-[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}))";

/// The `Token` pattern, as the model declares it (every `NextToken` member).
const TOKEN_PATTERN: &str = r"[-a-zA-Z0-9+=/_]*";

/// A `ResourceArn` the `TaggableResourceArn` pattern admits, so a row can refuse a member
/// OTHER than the resource ARN.
const TAGGABLE_INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-0000000000000000";

/// One clause of a refusal, for the member at `path` and the requirement it broke.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value of input '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement clause for a `@pattern`.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// One row: an operation, the exact request body to send, and the `Message` the refusal
/// must carry.
struct Case {
    label: &'static str,
    operation: &'static str,
    body: &'static str,
    message: fn() -> String,
}

/// The rows the rule is asserted over.
const CASES: &[Case] = &[
    Case {
        label: "a malformed InstanceArn on a read",
        operation: "DescribeInstance",
        body: r#"{"InstanceArn":"arn:aws:sso:::instance/not-a-valid-id"}"#,
        message: || clause("instanceArn", &pattern_requirement(INSTANCE_ARN_PATTERN)),
    },
    Case {
        // The same member, the same clause, from a DELETE: the wording belongs to the
        // service's validator, not to one operation.
        label: "a malformed InstanceArn on a delete",
        operation: "DeleteInstance",
        body: r#"{"InstanceArn":"arn:aws:sso:::instance/not-a-valid-id"}"#,
        message: || clause("instanceArn", &pattern_requirement(INSTANCE_ARN_PATTERN)),
    },
    Case {
        label: "a malformed InstanceArn in another partition's spelling",
        operation: "DescribeInstance",
        body: r#"{"InstanceArn":"arn:aws-us-gov:sso:::instance/ssoins-000"}"#,
        message: || clause("instanceArn", &pattern_requirement(INSTANCE_ARN_PATTERN)),
    },
    Case {
        // TWO members refused by one request: the clauses come in the reverse of the
        // order `ListTagsForResourceRequest` declares them (`InstanceArn`, `ResourceArn`).
        label: "two malformed ARNs, the resource ARN written first",
        operation: "ListTagsForResource",
        body: r#"{"ResourceArn":"not-an-arn","InstanceArn":"not-an-arn"}"#,
        message: || {
            format!(
                "{}; {}",
                clause(
                    "resourceArn",
                    &pattern_requirement(TAGGABLE_RESOURCE_ARN_PATTERN)
                ),
                clause("instanceArn", &pattern_requirement(INSTANCE_ARN_PATTERN))
            )
        },
    },
    Case {
        // The SAME message from the same two members written the other way round: the
        // order is the model's, so the request's own order cannot change it.
        label: "two malformed ARNs, the instance ARN written first",
        operation: "ListTagsForResource",
        body: r#"{"InstanceArn":"not-an-arn","ResourceArn":"not-an-arn"}"#,
        message: || {
            format!(
                "{}; {}",
                clause(
                    "resourceArn",
                    &pattern_requirement(TAGGABLE_RESOURCE_ARN_PATTERN)
                ),
                clause("instanceArn", &pattern_requirement(INSTANCE_ARN_PATTERN))
            )
        },
    },
    Case {
        // A write refusing the same two members, with its own required members satisfied.
        label: "two malformed ARNs on a tag write",
        operation: "TagResource",
        body: r#"{"ResourceArn":"not-an-arn","InstanceArn":"not-an-arn","Tags":[{"Key":"k","Value":"v"}]}"#,
        message: || {
            format!(
                "{}; {}",
                clause(
                    "resourceArn",
                    &pattern_requirement(TAGGABLE_RESOURCE_ARN_PATTERN)
                ),
                clause("instanceArn", &pattern_requirement(INSTANCE_ARN_PATTERN))
            )
        },
    },
    Case {
        // `@length` and `@pattern` on ONE member: the walk checks the length first, so
        // the pattern clause is stated first.
        label: "an empty InstanceArn, short AND malformed",
        operation: "DescribeInstance",
        body: r#"{"InstanceArn":""}"#,
        message: || {
            format!(
                "{}; {}",
                clause("instanceArn", &pattern_requirement(INSTANCE_ARN_PATTERN)),
                clause(
                    "instanceArn",
                    "Member must have length greater than or equal to 10"
                )
            )
        },
    },
    Case {
        label: "an absent required member",
        operation: "DescribeInstance",
        body: "{}",
        message: || clause("instanceArn", "Member must not be null"),
    },
    Case {
        label: "an absent required member on a tag write",
        operation: "TagResource",
        body: r#"{"ResourceArn":"arn:aws:sso:::instance/ssoins-0000000000000000"}"#,
        message: || clause("tags", "Member must not be null"),
    },
    Case {
        label: "a page size past the published maximum",
        operation: "ListInstances",
        body: r#"{"MaxResults":101}"#,
        message: || {
            clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            )
        },
    },
    Case {
        label: "a page size below the published minimum",
        operation: "ListInstances",
        body: r#"{"MaxResults":0}"#,
        message: || {
            clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            )
        },
    },
    Case {
        // A member the issue never cited, whose pattern is not an ARN at all: the same
        // rule quotes it the same way.
        label: "a pagination token outside its pattern",
        operation: "ListTagsForResource",
        body: r#"{"ResourceArn":"arn:aws:sso:::instance/ssoins-0000000000000000","NextToken":"not a token!"}"#,
        message: || clause("nextToken", &pattern_requirement(TOKEN_PATTERN)),
    },
    Case {
        label: "a pagination token outside its pattern on a list",
        operation: "ListInstances",
        body: r#"{"NextToken":"not a token!"}"#,
        message: || clause("nextToken", &pattern_requirement(TOKEN_PATTERN)),
    },
];

/// The rule, over the whole table: 400 `ValidationException`, one clause per violation in
/// the `Message` member, naming the member and the requirement it broke.
#[tokio::test]
async fn a_refused_request_states_every_member_and_the_constraint_it_broke() {
    let router = router();

    for case in CASES {
        let (status, body) = call_raw(&router, TEST_REGION, case.operation, case.body).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for {}, got {status} {body}",
            case.label, case.body
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: expected ValidationException, got {body}",
            case.label
        );
        let expected = (case.message)();
        assert_eq!(
            body["Message"].as_str(),
            Some(expected.as_str()),
            "case {:?}: expected Message {expected:?}, got {body}",
            case.label
        );
        assert_eq!(
            body["message"],
            Value::Null,
            "case {:?}: the detail text must travel in ONE member, got {body}",
            case.label
        );
    }
}

/// Each published bound of a `@range` is INSIDE it: the refusal is for the value past the
/// bound, never for the bound itself.
#[tokio::test]
async fn a_page_size_at_either_published_bound_is_accepted() {
    let router = router();

    // `MaxResults` is `@range(min: 1, max: 100)`; each bound and one past it.
    for (case, max_results, expected_status) in [
        ("the minimum", 1, 200),
        ("the maximum", 100, 200),
        ("one below the minimum", 0, 400),
        ("one past the maximum", 101, 400),
    ] {
        let (status, body) = call(&router, "ListInstances", &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults {max_results}, \
             got {status} {body}"
        );
        if expected_status == 400 {
            let expected = clause(
                "maxResults",
                if max_results < 1 {
                    "Member must have value greater than or equal to 1"
                } else {
                    "Member must have value less than or equal to 100"
                },
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected Message {expected:?}, got {body}"
            );
        }
    }
}

/// Whatever produced it, an error body reports its detail text in `Message` and never
/// beside it: the modelled failures a handler returns, and the bodies the framework
/// renders itself.
#[tokio::test]
async fn every_error_body_reports_its_detail_text_in_the_message_member() {
    let router = router();

    // (case, operation, body) — one error per channel this service answers on: the
    // boundary refusal, a modelled failure from a handler, and the scaffold's own signal
    // for an operation nobody has implemented yet.
    let rows: [(&str, &str, Value); 3] = [
        (
            "the boundary validator's refusal",
            "DescribeInstance",
            json!({"InstanceArn": "not-an-arn"}),
        ),
        (
            "a modelled failure a handler returns",
            "DescribeInstance",
            json!({"InstanceArn": UNKNOWN_INSTANCE_ARN}),
        ),
        (
            "an operation that is still scaffolded",
            "ListTagsForResource",
            json!({"ResourceArn": TAGGABLE_INSTANCE_ARN}),
        ),
    ];

    for (case, operation, body) in rows {
        let (status, answer) = call(&router, operation, &body).await;
        assert!(
            status >= 400,
            "case {case:?}: expected a failure, got {status} {answer}"
        );
        assert!(
            answer["Message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected the detail text in Message, got {answer}"
        );
        assert_eq!(
            answer["message"],
            Value::Null,
            "case {case:?}: the detail text must travel in ONE member, got {answer}"
        );
    }
}

/// A success is not an error body: nothing about it is rewritten.
#[tokio::test]
async fn a_successful_answer_is_untouched() {
    let router = router();

    let (status, body) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {body}");
    assert!(
        body["Instances"].is_array(),
        "a success must answer with the modelled members, got {body}"
    );
    assert_eq!(
        body["Message"],
        Value::Null,
        "a success carries no detail text, got {body}"
    );
}
