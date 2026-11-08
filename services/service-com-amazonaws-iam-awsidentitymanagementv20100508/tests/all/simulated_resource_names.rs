// SPDX-License-Identifier: CC0-1.0

//! How a simulation refuses a `ResourceArns` entry that names no resource.
//!
//! The rule under test: an entry of `ResourceArns` that is neither an Amazon Resource Name
//! nor the wildcard the member defaults to names no resource a statement's `Resource`
//! element could be decided against, so the request is refused `InvalidInput` (HTTP 400)
//! with the message naming every such entry, in the order the request carries them. "A list
//! of ARNs of Amazon Web Services resources to include in the simulation. If this parameter
//! is not provided, then the value defaults to * (all resources)"
//! (`SimulateCustomPolicyRequest$ResourceArns`).
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: how the
//! entry fails to be an ARN (no `arn` prefix, too few fields, an empty partition or
//! vendor, text that is no ARN at all), where it sits in the list, how many entries are
//! malformed, and which of the two simulation operations took the member — beside the rows
//! that must NOT be refused: well-formed ARNs of several services, a partition-global one,
//! and the wildcard itself.
//!
//! The awsQuery harness is the one the other suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::request_condition_context::{create_user, user_arn};
use crate::user_lifecycle::{call, element, error_code, router};

/// The error code every refusal here carries (`InvalidInputException`, `awsQueryError` code
/// `InvalidInput`, HTTP 400).
const INVALID_INPUT: &str = "InvalidInput";

/// The one document every call supplies: a readable policy, so the refusal is about the
/// resources the request names and nothing else.
const DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#;

/// The action every call asks about.
const ACTION: &str = "s3:ListBucket";

/// The two operations that take `ResourceArns`, by the `Action` each is called under; the
/// `Principal` spelling takes the entity whose own policies join the request's.
const OPERATIONS: &[&str] = &["SimulateCustomPolicy", "SimulatePrincipalPolicy"];

/// One call of `operation` asking about `resources`.
async fn simulate(
    router: &MultiServiceRouter,
    operation: &str,
    principal_arn: &str,
    resources: &[&str],
) -> (u16, String) {
    let mut owned: Vec<(String, String)> = vec![
        ("PolicyInputList.member.1".to_string(), DOCUMENT.to_string()),
        ("ActionNames.member.1".to_string(), ACTION.to_string()),
    ];
    for (position, resource) in (1..).zip(resources) {
        owned.push((
            format!("ResourceArns.member.{position}"),
            (*resource).to_string(),
        ));
    }
    if operation == "SimulatePrincipalPolicy" {
        owned.push(("PolicySourceArn".to_string(), principal_arn.to_string()));
    }
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call(router, operation, &params).await
}

/// A well-formed ARN a row carries beside a malformed one.
const BUCKET: &str = "arn:aws:s3:::reports";

/// One row of the refused table: the resources the request names, and the entries the
/// refusal must name, in that order.
struct RefusedCase {
    case: &'static str,
    resources: &'static [&'static str],
    named: &'static [&'static str],
}

/// Rows spanning each way an entry fails to be an ARN and each place it can sit in the
/// list.
const REFUSED_CASES: &[RefusedCase] = &[
    RefusedCase {
        case: "text that is no ARN at all",
        resources: &["not-an-arn"],
        named: &["not-an-arn"],
    },
    RefusedCase {
        case: "an ARN missing its leading field",
        resources: &["aws:s3:::reports"],
        named: &["aws:s3:::reports"],
    },
    RefusedCase {
        case: "an ARN carrying too few fields",
        resources: &["arn:aws:s3"],
        named: &["arn:aws:s3"],
    },
    RefusedCase {
        case: "an ARN whose partition is empty",
        resources: &["arn::s3:::reports"],
        named: &["arn::s3:::reports"],
    },
    RefusedCase {
        case: "an ARN whose vendor is empty",
        resources: &["arn:aws::us-east-1:000000000000:reports"],
        named: &["arn:aws::us-east-1:000000000000:reports"],
    },
    RefusedCase {
        case: "an ARN whose resource is empty",
        resources: &["arn:aws:s3:::"],
        named: &["arn:aws:s3:::"],
    },
    RefusedCase {
        case: "the second of two entries",
        resources: &[BUCKET, "not-an-arn"],
        named: &["not-an-arn"],
    },
    RefusedCase {
        case: "the first of two entries",
        resources: &["not-an-arn", BUCKET],
        named: &["not-an-arn"],
    },
    RefusedCase {
        case: "two malformed entries among well-formed ones",
        resources: &[BUCKET, "not-an-arn", BUCKET, "arn:aws:s3"],
        named: &["not-an-arn", "arn:aws:s3"],
    },
];

/// The message a refusal naming `named` carries.
fn expected_message(named: &[&str]) -> String {
    format!("ResourceNames are not in a valid ARN format: {}.", named.join(", "))
}

/// RULE: an entry of `ResourceArns` that is no Amazon Resource Name is refused
/// `InvalidInput`, and the refusal names every such entry in the order the request carries
/// them — whichever simulation operation took the list.
#[tokio::test]
async fn a_resource_that_is_not_an_arn_is_refused_naming_it() {
    let router = router();
    let principal = "resource-names-principal";
    create_user(&router, "the named principal", principal, &[]).await;
    let principal_arn = user_arn(principal);

    for operation in OPERATIONS {
        for case in REFUSED_CASES {
            let (status, body) =
                simulate(&router, operation, &principal_arn, case.resources).await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, INVALID_INPUT),
                "case {operation:?}/{:?}: expected HTTP 400 {INVALID_INPUT}, got {status} \
                 {code}: {body}",
                case.case
            );
            let message = element(&body, "Message");
            let expected = expected_message(case.named);
            assert_eq!(
                message, expected,
                "case {operation:?}/{:?}: expected the message {expected:?}, got \
                 {message:?}: {body}",
                case.case
            );
        }
    }
}

/// The resource lists a simulation is served for: well-formed ARNs of several services,
/// including a partition-global one that carries no Region and no account, the wildcard the
/// member defaults to, and a list of several.
const SERVED_CASES: &[(&str, &[&str])] = &[
    ("one bucket ARN", &[BUCKET]),
    (
        "a partition-global ARN with no Region and no account",
        &["arn:aws:s3:::reports/2024/q1.csv"],
    ),
    (
        "an ARN of another service, with a Region and an account",
        &["arn:aws:dynamodb:us-east-1:000000000000:table/orders"],
    ),
    (
        "an ARN whose resource carries colons",
        &["arn:aws:states:us-east-1:000000000000:execution:flow:run-1"],
    ),
    (
        // "If this parameter is not provided, then the value defaults to * (all
        // resources)", so the wildcard is a value the member names a resource set with.
        "the wildcard the member defaults to",
        &["*"],
    ),
    (
        "several well-formed ARNs",
        &[BUCKET, "arn:aws:dynamodb:us-east-1:000000000000:table/orders"],
    ),
];

/// RULE: a list whose every entry names a resource is not refused on this ground at all —
/// an ARN of any service, with or without a Region and an account, and the wildcard the
/// member defaults to, are each a resource the simulation decides about.
#[tokio::test]
async fn resources_that_name_something_are_simulated() {
    let router = router();
    let principal = "resource-names-served";
    create_user(&router, "the named principal", principal, &[]).await;
    let principal_arn = user_arn(principal);

    for operation in OPERATIONS {
        for (case, resources) in SERVED_CASES {
            let (status, body) = simulate(&router, operation, &principal_arn, resources).await;
            assert_eq!(
                status, 200,
                "case {operation:?}/{case:?}: expected 200, got {status}: {body}"
            );
            // Every resource the request named is reported back, which is what says the
            // simulation decided about each of them.
            for resource in *resources {
                assert!(
                    body.contains(resource),
                    "case {operation:?}/{case:?}: expected the report to name {resource:?}, \
                     got {body}"
                );
            }
        }
    }
}
