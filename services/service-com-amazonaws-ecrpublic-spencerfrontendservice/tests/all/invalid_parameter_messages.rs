// SPDX-License-Identifier: CC0-1.0

//! How ECR Public states a request it refuses before any handler runs.
//!
//! The rule under test: a value that violates a constraint the model declares is refused
//! 400 `InvalidParameterException` whose message is ECR's template filled from the
//! request — `Invalid parameter at '<member path>' failed to satisfy constraint:
//! '<the rule>'` — naming the member path and the rule and NOTHING else: not the rejected
//! value, not a count of its bytes, and not a count of clauses. Several refused members
//! are several clauses of the one message.
//!
//! The table spans the constraint classes the model declares and the members that carry
//! them — a `@pattern`, both ends of a `@length` on a top-level member and on a nested
//! one, both ends of a `@range`, and an absent `@required` member — with each published
//! bound tested at the bound (which must be accepted) and one past it (which must be
//! refused), so the rows differ along the rule rather than only in their values.

use serde_json::{json, Value};

use super::harness::error_message;
use crate::repository_image_lifecycle::{call, ok, refused, service};

/// The modelled `RepositoryName` bounds and pattern (`@length(2..205)`,
/// `@pattern(^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$)`).
const REPOSITORY_NAME_MIN: usize = 2;
const REPOSITORY_NAME_MAX: usize = 205;

/// The expression `RepositoryName`'s `@pattern` states, as the refusal reports it: the
/// model's expression without the anchors that make its match total.
const REPOSITORY_NAME_EXPRESSION: &str =
    "(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*";

/// `RepositoryDescription`'s modelled maximum (`@length(0..1024)`), reached through
/// `CreateRepository.catalogData.description`.
const DESCRIPTION_MAX: usize = 1024;

/// `RepositoryPolicyText`'s modelled maximum (`@length(0..10240)`).
const POLICY_TEXT_MAX: usize = 10_240;

/// `MaxResults`' modelled range (`@range(1..1000)`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// One request that must be refused, and the clause the refusal must carry.
struct RefusalCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The dotted member path the clause names.
    member_path: &'static str,
    /// The rule the clause states.
    requirement: String,
    /// The value the refusal must NOT echo back, when the row has one.
    rejected_value: Option<String>,
}

/// A `repositoryName` of exactly `length` characters that the modelled pattern admits.
fn name_of(length: usize) -> String {
    "a".repeat(length)
}

/// A repository policy the service accepts, padded through its `Sid` to exactly `bytes`
/// bytes, so `policyText`'s inclusive upper bound is exercised with a document that is
/// refused for its LENGTH alone when one byte is added to it.
fn policy_of_exactly(bytes: usize) -> String {
    let statement = |sid: &str| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": sid,
                "Effect": "Allow",
                "Principal": "*",
                "Action": "ecr-public:BatchGetImage",
            }],
        })
        .to_string()
    };
    let padding = bytes - statement("").len();
    statement(&"p".repeat(padding))
}

/// A name one character UNDER the modelled minimum. Spelled with a digit the template
/// itself never carries, so the "the message does not echo the rejected value" assertion
/// below is about the echo rather than about a letter the sentence happens to share.
const TOO_SHORT_NAME: &str = "7";

fn refusal_cases() -> Vec<RefusalCase> {
    let too_long_name = name_of(REPOSITORY_NAME_MAX + 1);
    let bad_name = "Invalid_Name!!not-a-repository";
    let too_long_description = "D".repeat(DESCRIPTION_MAX + 1);
    let too_long_policy = format!(
        "{{\"Version\": \"2012-10-17\", \"Comment\": \"{}\"}}",
        "p".repeat(POLICY_TEXT_MAX)
    );
    vec![
        RefusalCase {
            case: "a repository name outside the modelled pattern",
            operation: "CreateRepository",
            body: json!({"repositoryName": bad_name}),
            member_path: "repositoryName",
            requirement: format!(
                "must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"
            ),
            rejected_value: Some(bad_name.to_string()),
        },
        RefusalCase {
            case: "a repository name one character under the minimum",
            operation: "CreateRepository",
            body: json!({"repositoryName": TOO_SHORT_NAME}),
            member_path: "repositoryName",
            requirement: format!(
                "Member must have length greater than or equal to {REPOSITORY_NAME_MIN}"
            ),
            rejected_value: Some(TOO_SHORT_NAME.to_string()),
        },
        RefusalCase {
            case: "a repository name one character over the maximum",
            operation: "CreateRepository",
            body: json!({"repositoryName": too_long_name}),
            member_path: "repositoryName",
            requirement: format!(
                "Member must have length less than or equal to {REPOSITORY_NAME_MAX}"
            ),
            rejected_value: Some(too_long_name.clone()),
        },
        RefusalCase {
            case: "a nested member over its maximum",
            operation: "CreateRepository",
            body: json!({
                "repositoryName": "team/app",
                "catalogData": {"description": too_long_description.clone()},
            }),
            member_path: "catalogData.description",
            requirement: format!(
                "Member must have length less than or equal to {DESCRIPTION_MAX}"
            ),
            rejected_value: Some(too_long_description),
        },
        RefusalCase {
            case: "a policy document over its maximum",
            operation: "SetRepositoryPolicy",
            body: json!({"repositoryName": "team/app", "policyText": too_long_policy.clone()}),
            member_path: "policyText",
            requirement: format!(
                "Member must have length less than or equal to {POLICY_TEXT_MAX}"
            ),
            rejected_value: Some(too_long_policy),
        },
        RefusalCase {
            case: "a page size under the modelled range",
            operation: "DescribeRepositories",
            body: json!({"maxResults": MAX_RESULTS_MIN - 1}),
            member_path: "maxResults",
            requirement: format!(
                "Member must have value greater than or equal to {MAX_RESULTS_MIN}"
            ),
            rejected_value: None,
        },
        RefusalCase {
            case: "a page size over the modelled range",
            operation: "DescribeImages",
            body: json!({"repositoryName": "team/app", "maxResults": MAX_RESULTS_MAX + 1}),
            member_path: "maxResults",
            requirement: format!(
                "Member must have value less than or equal to {MAX_RESULTS_MAX}"
            ),
            rejected_value: None,
        },
        RefusalCase {
            case: "an absent required member",
            operation: "CreateRepository",
            body: json!({}),
            member_path: "repositoryName",
            requirement: "Member must not be null".to_string(),
            rejected_value: None,
        },
    ]
}

/// The clause ECR fills from the request for one refused member.
fn clause(member_path: &str, requirement: &str) -> String {
    format!("Invalid parameter at '{member_path}' failed to satisfy constraint: '{requirement}'")
}

#[tokio::test]
async fn a_refused_member_is_stated_as_the_ecr_template_over_the_member_and_the_rule() {
    let router = service();

    for case in refusal_cases() {
        let label = case.case;
        let body = refused(
            &router,
            case.operation,
            case.body,
            "InvalidParameterException",
            label,
        )
        .await;
        let message = error_message(&body);
        assert_eq!(
            message,
            clause(case.member_path, &case.requirement),
            "case {label:?}: expected ECR's template over the member and the rule, got \
             {message:?}"
        );
        // The three things the template does NOT carry.
        assert!(
            !message.contains("validation error"),
            "case {label:?}: the message must not count clauses the way the framework's \
             envelope does, got {message:?}"
        );
        assert!(
            !message.contains("bytes]"),
            "case {label:?}: the message must not state the size of the rejected value, \
             got {message:?}"
        );
        if let Some(rejected) = case.rejected_value.as_deref() {
            assert!(
                !message.contains(rejected),
                "case {label:?}: the message must not echo the rejected value, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_published_bound_is_accepted_at_the_bound() {
    let router = service();

    // `repositoryName` at both ends of its `@length`, with `catalogData.description` and
    // `policyText` at theirs, and a page size at each end of its `@range`: the values one
    // step beyond each of these are the refused rows above.
    let shortest = name_of(REPOSITORY_NAME_MIN);
    let longest = name_of(REPOSITORY_NAME_MAX);
    for (case, name) in [("the shortest name", &shortest), ("the longest name", &longest)] {
        ok(
            &router,
            "CreateRepository",
            json!({
                "repositoryName": name,
                "catalogData": {"description": "D".repeat(DESCRIPTION_MAX)},
            }),
            case,
        )
        .await;
        ok(
            &router,
            "SetRepositoryPolicy",
            json!({
                "repositoryName": name,
                "policyText": policy_of_exactly(POLICY_TEXT_MAX),
            }),
            case,
        )
        .await;
    }
    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        ok(
            &router,
            "DescribeRepositories",
            json!({"maxResults": max_results}),
            "a page size at the bound",
        )
        .await;
        ok(
            &router,
            "DescribeImages",
            json!({"repositoryName": shortest, "maxResults": max_results}),
            "a page size at the bound",
        )
        .await;
    }
}

#[tokio::test]
async fn two_refused_members_are_two_clauses_of_one_message() {
    let router = service();
    let (status, body) = call(
        &router,
        "CreateRepository",
        json!({
            "repositoryName": "Invalid_Name!!two-clauses",
            "catalogData": {"description": "D".repeat(DESCRIPTION_MAX + 1)},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected 400 for two refused members, got {status} with {body}"
    );
    let message = error_message(&body);
    for expected in [
        clause(
            "repositoryName",
            &format!("must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"),
        ),
        clause(
            "catalogData.description",
            &format!("Member must have length less than or equal to {DESCRIPTION_MAX}"),
        ),
    ] {
        assert!(
            message.contains(&expected),
            "expected the message to carry {expected:?}, got {message:?}"
        );
    }
    assert!(
        !message.contains("validation error") && !message.contains("bytes]"),
        "expected clauses alone, got {message:?}"
    );
}
