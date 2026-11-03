// SPDX-License-Identifier: CC0-1.0

//! The tag surface's `@pattern` dialect over the whole FAMILY and over values that miss the
//! expression INSIDE it, end to end over the wire (the same `MultiServiceRouter` the
//! emulator boots, through the harness in `canary_lifecycle`).
//!
//! `pattern_requirement_wording.rs` owns the rule itself — which expression a refusal names
//! depends on whether the OPERATION declares `ValidationException` in its own modelled error
//! set — and pins it over both families, every binding and values that break the declared
//! expression at its ENDS. This suite extends the published half of it along two axes that
//! one does not reach:
//!
//! 1. **The whole family.** All three tagging operations — `TagResource`, `UntagResource` and
//!    the reader `ListTagsForResource`, the three the model declares the
//!    `BadRequestException` family for and no `ValidationException`
//!    (`smithy_query(query="digest")`) — state the published expression for the same member.
//! 2. **A value that misses the expression inside it.** An ARN that is well formed and fails
//!    on one INNER part of `ResourceArn`'s expression — another service's namespace, a
//!    resource type its `(canary|group)` alternation does not admit, an account that is not
//!    twelve digits, a Region in the wrong shape, a name segment outside the charset of its
//!    last run — is refused with the same published expression, so the dialect is a property
//!    of the operation and not of where in the value the match failed.
//!
//! The expression is read out of the model this crate ships
//! (`canary_lifecycle::declared_pattern`) and the published form comes from
//! `service_common::validation::unanchored`, this tree's one rendering of that drop, so
//! nothing here restates either the model's rule or the drop.

use serde_json::json;

use crate::canary_lifecycle::{declared_pattern, one_violation, pattern_requirement, Synthetics};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The three operations the model declares no `ValidationException` for, each with the
/// method and the request shape that puts `ResourceArn` in its URI label.
const TAGGING_OPERATIONS: [(&str, &str); 3] = [
    ("TagResource", "POST"),
    ("UntagResource", "DELETE"),
    ("ListTagsForResource", "GET"),
];

/// ARNs that satisfy no part of `ResourceArn`'s expression at its ENDS — each is a
/// well-formed ARN that fails on ONE inner part of it — paired with the part it misses.
fn arns_failing_inside() -> Vec<(&'static str, String)> {
    vec![
        (
            "another service's namespace",
            format!("arn:aws:lambda:{REGION}:123456789012:canary:probe-canary"),
        ),
        (
            "a resource type the alternation does not admit",
            format!("arn:aws:synthetics:{REGION}:123456789012:dashboard:probe-canary"),
        ),
        (
            "an account that is not twelve digits",
            format!("arn:aws:synthetics:{REGION}:1234:canary:probe-canary"),
        ),
        (
            "a Region in the wrong shape",
            "arn:aws:synthetics:useast1:123456789012:canary:probe-canary".to_string(),
        ),
        (
            "a name segment outside the last run's charset",
            format!("arn:aws:synthetics:{REGION}:123456789012:canary:PROBE.canary"),
        ),
    ]
}

/// Each tagging operation's request for `arn`, as the model binds `ResourceArn` to the URI
/// label of its route (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`).
fn request(operation: &str, arn: &str) -> (&'static str, String, Option<serde_json::Value>) {
    match operation {
        "TagResource" => (
            "POST",
            format!("/tags/{arn}"),
            Some(json!({ "Tags": { "team": "synthetics" } })),
        ),
        "UntagResource" => ("DELETE", format!("/tags/{arn}?tagKeys=team"), None),
        "ListTagsForResource" => ("GET", format!("/tags/{arn}"), None),
        other => panic!("case {other}: this suite covers the three tagging operations only"),
    }
}

#[tokio::test]
async fn every_tagging_operation_states_the_published_expression_for_a_value_failing_inside_it() {
    let aws = Synthetics::new(REGION);

    let declared = declared_pattern("ResourceArn");
    // The rows prove something only if the model's own expression carries the whole-value
    // pair: that pair is the only difference between the two dialects.
    assert!(
        declared.starts_with('^') && declared.ends_with('$'),
        "case ResourceArn: the modelled pattern {declared:?} is not anchored, so this suite \
         asserts nothing"
    );
    let published = service_common::validation::unanchored(&declared);

    for (operation, modelled_method) in TAGGING_OPERATIONS {
        for (missed, arn) in arns_failing_inside() {
            let case = format!("{operation}/{missed}");
            let (method, path, body) = request(operation, &arn);
            assert_eq!(
                method, modelled_method,
                "case {case}: expected the modelled method {modelled_method}, got {method}"
            );
            let answer = aws.call(method, &path, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 for {arn:?}, got {} ({:?})",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case}: expected x-amzn-errortype: ValidationException, got {:?}",
                answer.error_code
            );
            let expected = one_violation(&arn, "resourceArn", &pattern_requirement(&published));
            assert_eq!(
                answer.at("Message").as_str(),
                Some(expected.as_str()),
                "case {case}: expected Message {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}
