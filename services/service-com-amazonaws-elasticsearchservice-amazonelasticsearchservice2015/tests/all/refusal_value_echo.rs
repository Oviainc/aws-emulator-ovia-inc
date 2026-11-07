// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the VALUE half of a boundary-refusal clause on this API: when the
//! refusal echoes the value a member submitted, and when it names the member and its
//! requirements alone.
//!
//! The rule: a member the request broke ONE constraint of is stated with the value it
//! submitted (`Value '<submitted>' at '<member path>' failed to satisfy constraint: …`),
//! and a member the request broke SEVERAL constraints of is stated without it
//! (`Value at '<member path>' failed to satisfy constraint: …`) — in every one of that
//! member's clauses. What distinguishes the clauses of such a report is the requirement
//! each names; the value is the same in all of them.
//!
//! Neither half is about the value's size or its shape: the rows below span a short value
//! and a 36-character dotted one, a value bound to a URI label, to a query parameter and
//! to a body member nested in a structure, and the constraint classes this model declares
//! for them (`@length`'s two bounds, `@pattern`, an enumerated value set — the
//! constraints `smithy_query(query="shape",shape="DomainName,PackageName,UserPoolId,\
//! CognitoOptions,EngineType")` reports for these members).

use serde_json::{json, Value};

use crate::harness::{call_in, router};

/// The Region every call is signed for.
const REGION: &str = "us-east-1";

/// The requirement clause for `DomainName`'s and `PackageName`'s upper length bound
/// (`@length(3, 28)`).
const AT_MOST_28: &str = "Member must have length less than or equal to 28";

/// The requirement clause for their lower length bound.
const AT_LEAST_3: &str = "Member must have length greater than or equal to 3";

/// The requirement clause for their `@pattern`, quoted as this API quotes an expression.
const NAME_EXPRESSION: &str = r"Member must satisfy regular expression pattern: [a-z][a-z0-9\-]+";

/// RULE: a member with ONE violation is stated with the value it submitted, whatever that
/// value is and wherever it was bound.
#[tokio::test]
async fn a_member_with_one_violation_states_the_value_it_submitted() {
    let router = router();

    for (case, method, path, body, member, value) in [
        (
            "a URI label outside its pattern",
            "GET",
            "/2015-01-01/es/domain/BADDOMAIN".to_string(),
            None,
            "domainName",
            "BADDOMAIN",
        ),
        (
            "a URI label below its length floor",
            "GET",
            "/2015-01-01/es/domain/ab/config".to_string(),
            None,
            "domainName",
            "ab",
        ),
        (
            "a query parameter outside its value set",
            "GET",
            "/2015-01-01/domain?engineType=opensearch".to_string(),
            None,
            "engineType",
            "opensearch",
        ),
        (
            "a body member below its length floor",
            "POST",
            "/2015-01-01/packages".to_string(),
            Some(json!({
                "PackageName": "ab",
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v1.txt"}
            })),
            "packageName",
            "ab",
        ),
        (
            // A 36-character dotted value on a member nested in a structure: not
            // label-shaped, and still echoed in full, because it broke ONE constraint.
            "a nested body member outside its pattern",
            "POST",
            "/2015-01-01/es/domain".to_string(),
            Some(json!({
                "DomainName": "echo-probe-domain",
                "CognitoOptions": {
                    "Enabled": true,
                    "UserPoolId": "cognito-pool.us-west-2.amazonaws.com"
                }
            })),
            "cognitoOptions.userPoolId",
            "cognito-pool.us-west-2.amazonaws.com",
        ),
    ] {
        let answer = call_in(&router, REGION, method, &path, body.as_ref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: the request must be refused with 400, got {} {}",
            answer.status, answer.body
        );
        let message = answer.text(case, "/message");
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {case:?}: one violation is one validation error, got {message:?}"
        );
        assert!(
            message.contains(&format!("Value '{value}' at '{member}' failed to satisfy")),
            "case {case:?}: the clause must state the submitted value at {member:?}, got \
             {message:?}"
        );
    }
}

/// One row of the table below: a request that breaks SEVERAL constraints of one member.
struct Several {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path every clause names.
    member: &'static str,
    /// The value the request submitted, which no clause may state.
    value: &'static str,
    /// The requirement each clause states, in the order the report states them.
    requirements: &'static [&'static str],
}

/// RULE: a member with SEVERAL violations is stated without the value it submitted, in
/// every one of its clauses — and every requirement it broke is still reported.
#[tokio::test]
async fn a_member_with_several_violations_is_stated_without_its_value() {
    let router = router();

    let rows = vec![
        Several {
            // Past the upper bound AND outside the expression: a name that is an endpoint
            // rather than a domain name.
            case: "a URI label too long and outside its pattern",
            method: "GET",
            path: "/2015-01-01/es/domain/vpc-probe-domain.us-west-2.es.amazonaws.com"
                .to_string(),
            body: None,
            member: "domainName",
            value: "vpc-probe-domain.us-west-2.es.amazonaws.com",
            requirements: &[AT_MOST_28, NAME_EXPRESSION],
        },
        Several {
            // The other bound: below the floor AND outside the expression.
            case: "a URI label too short and outside its pattern",
            method: "DELETE",
            path: "/2015-01-01/es/domain/A".to_string(),
            body: None,
            member: "domainName",
            value: "A",
            requirements: &[AT_LEAST_3, NAME_EXPRESSION],
        },
        Several {
            // The same member bound in the BODY rather than to a label.
            case: "a body member too long and outside its pattern",
            method: "POST",
            path: "/2015-01-01/es/domain".to_string(),
            body: Some(json!({"DomainName": "Bad.Domain.Name.That.Is.Too.Long"})),
            member: "domainName",
            value: "Bad.Domain.Name.That.Is.Too.Long",
            requirements: &[AT_MOST_28, NAME_EXPRESSION],
        },
        Several {
            // Another resource's name member, same two constraint classes.
            case: "a package name too long and outside its pattern",
            method: "POST",
            path: "/2015-01-01/packages".to_string(),
            body: Some(json!({
                "PackageName": "THIS-PACKAGE-NAME-IS-DEFINITELY-TOO-LONG",
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v1.txt"}
            })),
            member: "packageName",
            value: "THIS-PACKAGE-NAME-IS-DEFINITELY-TOO-LONG",
            requirements: &[AT_MOST_28, NAME_EXPRESSION],
        },
    ];

    for row in rows {
        let Several {
            case,
            method,
            path,
            body,
            member,
            value,
            requirements,
        } = row;
        let answer = call_in(&router, REGION, method, &path, body.as_ref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: the request must be refused with 400, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: the error must be ValidationException, got {:?} / {}",
            answer.error_type,
            answer.body
        );
        let message = answer.text(case, "/message");
        let expected = format!(
            "{} validation errors detected: {}",
            requirements.len(),
            requirements
                .iter()
                .map(|requirement| format!(
                    "Value at '{member}' failed to satisfy constraint: {requirement}"
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(value),
            "case {case:?}: no clause may state the submitted value, got {message:?}"
        );
    }
}
