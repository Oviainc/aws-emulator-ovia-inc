// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused at the boundary, end to end
//! over the awsQuery wire.
//!
//! The rule under test: a PAGE SIZE outside the `@range` its member declares is refused
//! with HTTP 400 `ValidationError` whose message names the member and the bound it broke
//! and nothing of the number itself — `N validation error(s) detected: Value at
//! 'maxItems' failed to satisfy constraint: Member must have value {greater,less} than or
//! equal to <bound>` — while every other class of boundary refusal, another numeric
//! member outside its own range included, keeps the sentence the framework states with
//! the rejected value in it.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation carries the page size, which bound is broken and by how much, which other
//! members the same request carries, how many violations one request carries, and which
//! other constraint class it breaks. No single request is the specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.

use crate::user_lifecycle::{call, element, error_code, router};

/// A trust policy with a single statement, so the `CreateRole` rows are refused for the
/// member under test and nothing else.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// One row: the call to make, the member path the clause must name, and the requirement
/// it must state.
struct PageSizeCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, &'static str)>,
    requirement: &'static str,
}

/// The rows: the page size broken below its minimum and above its maximum, across the
/// listings this service's model binds it to.
///
/// `MaxItems` targets `maxItemsType` (range 1..1000) on every one of them
/// (`smithy_query(query="operation",
/// operation="ListInstanceProfiles,ListRoles,ListUsers,ListGroups,ListPolicies")`).
fn page_size_cases() -> Vec<PageSizeCase> {
    let below = "Member must have value greater than or equal to 1";
    let above = "Member must have value less than or equal to 1000";
    vec![
        PageSizeCase {
            case: "one below the minimum",
            action: "ListInstanceProfiles",
            params: vec![("MaxItems", "0")],
            requirement: below,
        },
        PageSizeCase {
            case: "far below the minimum",
            action: "ListInstanceProfiles",
            params: vec![("MaxItems", "-1")],
            requirement: below,
        },
        PageSizeCase {
            case: "one above the maximum",
            action: "ListInstanceProfiles",
            params: vec![("MaxItems", "1001")],
            requirement: above,
        },
        PageSizeCase {
            case: "far above the maximum, beside another member",
            action: "ListInstanceProfiles",
            params: vec![("PathPrefix", "/eng/"), ("MaxItems", "99999")],
            requirement: above,
        },
        PageSizeCase {
            case: "the same member on the role listing",
            action: "ListRoles",
            params: vec![("MaxItems", "0")],
            requirement: below,
        },
        PageSizeCase {
            case: "the same member on the user listing",
            action: "ListUsers",
            params: vec![("MaxItems", "1001")],
            requirement: above,
        },
        PageSizeCase {
            case: "the same member on the group listing",
            action: "ListGroups",
            params: vec![("MaxItems", "-100")],
            requirement: below,
        },
        PageSizeCase {
            case: "the same member on the policy listing",
            action: "ListPolicies",
            params: vec![("Scope", "Local"), ("MaxItems", "100000")],
            requirement: above,
        },
    ]
}

/// RULE: a page size outside its declared range is refused 400 `ValidationError`, and the
/// clause names the member and the bound it broke WITHOUT echoing the number.
#[tokio::test]
async fn a_page_size_outside_its_declared_range_is_refused_without_echoing_it() {
    let router = router();

    for PageSizeCase {
        case,
        action,
        params,
        requirement,
    } in page_size_cases()
    {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the query protocol's bound refusal code, got {body}"
        );
        let expected = format!(
            "1 validation error detected: Value at 'maxItems' failed to satisfy constraint: \
             {requirement}"
        );
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            element(&body, "Message")
        );
        // The rejected number is nowhere in the document.
        for (name, value) in &params {
            if *name == "MaxItems" {
                assert!(
                    !body.contains(&format!("Value &apos;{value}&apos;"))
                        && !body.contains(&format!("Value '{value}'")),
                    "case {case:?}: the clause must not echo {value:?}, got {body}"
                );
            }
        }
    }
}

/// RULE: the refusal is about the BOUND, so each published bound itself is accepted — the
/// value-free clause is not a blanket refusal of the member.
#[tokio::test]
async fn each_published_bound_itself_is_accepted() {
    let router = router();

    // Rows: both bounds of the page size, and both bounds of the other range member this
    // suite exercises, on an operation that carries each.
    for (case, action, params) in [
        (
            "the page-size minimum",
            "ListInstanceProfiles",
            vec![("MaxItems", "1")],
        ),
        (
            "the page-size maximum",
            "ListInstanceProfiles",
            vec![("MaxItems", "1000")],
        ),
        (
            "the session minimum",
            "CreateRole",
            vec![
                ("RoleName", "min-session-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("MaxSessionDuration", "3600"),
            ],
        ),
        (
            "the session maximum",
            "CreateRole",
            vec![
                ("RoleName", "max-session-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("MaxSessionDuration", "43200"),
            ],
        ),
    ] {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status}: {body}"
        );
    }
}

/// One row of the table of classes this service does NOT reword: the call to make, the
/// member the clause names and the value it must echo back.
struct EchoedCase<'a> {
    case: &'a str,
    action: &'a str,
    params: Vec<(&'a str, &'a str)>,
    member: &'a str,
    value: &'a str,
}

/// RULE: only the page size is stated value-free — every other class of boundary refusal
/// keeps the sentence the framework states, the rejected value included, including
/// another numeric member outside its own `@range` (`roleMaxSessionDurationType`, range
/// 3600..43200), for which this service has no recorded value-free answer.
#[tokio::test]
async fn another_constraint_class_keeps_the_frameworks_own_sentence() {
    let router = router();

    // Rows: a string outside its `@pattern` on two different members, one outside its
    // `@length` maximum, and the other range member below its minimum and above its
    // maximum on both operations that take it.
    let long_name = "p".repeat(65);
    let cases = vec![
        EchoedCase {
            case: "a profile name outside its pattern",
            action: "CreateInstanceProfile",
            params: vec![("InstanceProfileName", "has space")],
            member: "instanceProfileName",
            value: "has space",
        },
        EchoedCase {
            case: "a user name outside its pattern",
            action: "CreateUser",
            params: vec![("UserName", "has/slash")],
            member: "userName",
            value: "has/slash",
        },
        EchoedCase {
            case: "a role name above its length maximum",
            action: "CreateRole",
            params: vec![
                ("RoleName", long_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
            member: "roleName",
            value: long_name.as_str(),
        },
        EchoedCase {
            case: "a session duration below its minimum",
            action: "CreateRole",
            params: vec![
                ("RoleName", "ranged-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("MaxSessionDuration", "3599"),
            ],
            member: "maxSessionDuration",
            value: "3599",
        },
        EchoedCase {
            case: "a session duration above its maximum",
            action: "CreateRole",
            params: vec![
                ("RoleName", "ranged-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("MaxSessionDuration", "43201"),
            ],
            member: "maxSessionDuration",
            value: "43201",
        },
        EchoedCase {
            case: "that member on the update half",
            action: "UpdateRole",
            params: vec![
                ("RoleName", "ranged-role"),
                ("MaxSessionDuration", "43201"),
            ],
            member: "maxSessionDuration",
            value: "43201",
        },
    ];

    for EchoedCase {
        case,
        action,
        params,
        member,
        value,
    } in cases
    {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        // The code is the framework's own for a boundary refusal on this protocol, and
        // these rows go nowhere near the page-size rewording — so this pins the code
        // independently of it.
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the query protocol's bound refusal code, got {body}"
        );
        let message = element(&body, "Message");
        assert!(
            message.contains(&format!("Value '{value}' at '{member}'")),
            "case {case:?}: expected the framework's own sentence naming the value, got \
             {message:?}"
        );
    }
}

/// RULE: a request that breaks the page-size bound AND another constraint is stated as
/// both clauses — the page size value-free, the other as the framework states it.
#[tokio::test]
async fn a_mixed_refusal_states_each_class_its_own_way() {
    let router = router();
    // `pathPrefixType` is pattern ^\u002F[\u0021-\u007F]*$, so a prefix that does not
    // begin with a slash is the second violation.
    let (status, body) = call(
        &router,
        "ListRoles",
        &[("PathPrefix", "eng"), ("MaxItems", "0")],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status}: {body}");
    let message = element(&body, "Message");
    assert!(
        message.starts_with("2 validation errors detected: "),
        "expected both violations to be stated, got {message:?}"
    );
    assert!(
        message.contains(
            "Value at 'maxItems' failed to satisfy constraint: Member must have value \
             greater than or equal to 1"
        ),
        "expected the page-size clause to be stated without the number, got {message:?}"
    );
    assert!(
        message.contains("Value 'eng' at 'pathPrefix'"),
        "expected the pattern clause to keep the framework's sentence, got {message:?}"
    );
}

/// RULE: an ABSENT required member is a different refusal class and is not restated —
/// it keeps the framework's own code and wording, so a client branching on either sees
/// what it would have seen.
#[tokio::test]
async fn an_absent_required_member_is_not_restated() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateRole",
        &[("AssumeRolePolicyDocument", TRUST_POLICY)],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status}: {body}");
    let message = element(&body, "Message");
    assert!(
        message.contains("'roleName'") && message.contains("Member must not be null"),
        "expected the framework's absent-member sentence, got {message:?}"
    );
}
