// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused at the boundary, end to end
//! over the awsQuery wire.
//!
//! The rule under test: a value outside a BOUND its member declares — a number outside
//! its `@range`, or a string or a collection outside its `@length` — is refused with HTTP
//! 400 `ValidationError` whose message names the member and the bound it broke and
//! nothing of the value itself (`N validation error(s) detected: Value at '<member>'
//! failed to satisfy constraint: Member must have {value,length} {greater,less} than or
//! equal to <bound>`), while the other classes of boundary refusal — a string outside its
//! `@pattern`, an absent required member — keep the sentence the framework states with the
//! rejected value in it. A value outside an enumerated set is stated value-free too, and
//! is read back by `tests/all/enum_refusal_wording.rs`.
//!
//! The members whose WHOLE form AWS states one fixed sentence for are refused by IAM
//! itself rather than by this layer, so neither their bound nor their pattern is ever
//! stated as a clause here; that rule, and what a request breaking one of those members
//! AND a bound is answered with, is `tests/all/member_format_refusal_wording.rs`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! member carries the bound and which operation takes it, whether the bound is a `@range`
//! on a number or a `@length` on a string or a list, which end of the bound is broken and
//! by how much, which other members the same request carries, how many violations one
//! request carries, and which other constraint class it breaks. No single request is the
//! specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, element, error_code, router};

/// A trust policy with a single statement, so the `CreateRole` rows are refused for the
/// member under test and nothing else.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// One row: the call to make, the member path the clause must name, the requirement it
/// must state, and the value the request carried, which the clause must NOT echo.
struct BoundCase {
    case: String,
    action: &'static str,
    params: Vec<(String, String)>,
    member: &'static str,
    requirement: String,
    echoed: Vec<String>,
}

/// The requirement clause the framework states for a bound, built from the model's own
/// bound (`vendor/smithy-schema-server/src/constraints.rs::Violation::requirement`).
fn below_value(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

fn above_value(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

fn above_length(max: i64) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The rows: every bound this service's role and listing surface declares, broken at each
/// end.
///
/// Bounds come from the model: `MaxItems` targets `maxItemsType` (range 1..1000) on every
/// paginated listing, `MaxSessionDuration` targets `roleMaxSessionDurationType` (range
/// 3600..43200), `Description` targets `roleDescriptionType` (length 0..1000), `RoleName`
/// targets `roleNameType` (length 1..64) and `Tags` targets `tagListType` (length 0..50)
/// (`smithy_query(query="operation",
/// operation="ListInstanceProfiles,ListRoles,ListUsers,ListGroups,ListPolicies,CreateRole,UpdateRole,UpdateRoleDescription")`).
fn bound_cases() -> Vec<BoundCase> {
    let listing_floor = below_value(1);
    let listing_ceiling = above_value(1000);
    let session_floor = below_value(3600);
    let session_ceiling = above_value(43200);
    let description_ceiling = above_length(1000);

    let page_size_rows = [
        ("one below the minimum", "ListInstanceProfiles", "0"),
        ("far below the minimum", "ListInstanceProfiles", "-1"),
        ("the same member on the role listing", "ListRoles", "0"),
        ("the same member on the group listing", "ListGroups", "-100"),
    ];
    let mut rows: Vec<BoundCase> = Vec::new();
    for (case, action, value) in page_size_rows {
        rows.push(BoundCase {
            case: case.to_string(),
            action,
            params: vec![param("MaxItems", value)],
            member: "maxItems",
            requirement: listing_floor.clone(),
            echoed: vec![value.to_string()],
        });
    }
    for (case, action, value) in [
        ("one above the maximum", "ListInstanceProfiles", "1001"),
        ("the same member on the user listing", "ListUsers", "1001"),
    ] {
        rows.push(BoundCase {
            case: case.to_string(),
            action,
            params: vec![param("MaxItems", value)],
            member: "maxItems",
            requirement: listing_ceiling.clone(),
            echoed: vec![value.to_string()],
        });
    }
    rows.push(BoundCase {
        case: "far above the maximum, beside another member".to_string(),
        action: "ListInstanceProfiles",
        params: vec![param("PathPrefix", "/eng/"), param("MaxItems", "99999")],
        member: "maxItems",
        requirement: listing_ceiling.clone(),
        echoed: vec!["99999".to_string()],
    });
    rows.push(BoundCase {
        case: "the same member on the policy listing".to_string(),
        action: "ListPolicies",
        params: vec![param("Scope", "Local"), param("MaxItems", "100000")],
        member: "maxItems",
        requirement: listing_ceiling.clone(),
        echoed: vec!["100000".to_string()],
    });

    // A session duration below its minimum, including zero, and above its maximum, on
    // both operations that take it.
    for (case, action, value) in [
        ("a session duration one below its minimum", "CreateRole", "3599"),
        ("a session duration of zero", "CreateRole", "0"),
        ("a session duration on the update half", "UpdateRole", "3599"),
    ] {
        rows.push(BoundCase {
            case: case.to_string(),
            action,
            params: session_params(action, value),
            member: "maxSessionDuration",
            requirement: session_floor.clone(),
            echoed: vec![value.to_string()],
        });
    }
    for (case, action, value) in [
        ("a session duration one above its maximum", "CreateRole", "43201"),
        (
            "a session duration far above its maximum on the update half",
            "UpdateRole",
            "432000",
        ),
    ] {
        rows.push(BoundCase {
            case: case.to_string(),
            action,
            params: session_params(action, value),
            member: "maxSessionDuration",
            requirement: session_ceiling.clone(),
            echoed: vec![value.to_string()],
        });
    }

    // A STRING above its `@length` maximum, on each operation that takes a description:
    // one character over, and far over.
    let one_over = "d".repeat(1001);
    let far_over = "d".repeat(5000);
    for (case, action, value) in [
        ("a description one character over its maximum", "UpdateRole", &one_over),
        ("a description far over its maximum", "UpdateRole", &far_over),
        (
            "a description over its maximum on the create half",
            "CreateRole",
            &one_over,
        ),
        (
            "a description over its maximum on the description-only half",
            "UpdateRoleDescription",
            &one_over,
        ),
    ] {
        let mut params = vec![param("RoleName", "bounded-role"), param("Description", value)];
        if action == "CreateRole" {
            params.push(param("AssumeRolePolicyDocument", TRUST_POLICY));
        }
        rows.push(BoundCase {
            case: case.to_string(),
            action,
            params,
            member: "description",
            requirement: description_ceiling.clone(),
            echoed: vec![value.clone()],
        });
    }

    // A role name above the `@length` maximum its own member declares — the same class on
    // a different member, with a name the member's pattern admits so the length is the
    // only bound broken.
    let long_name = "p".repeat(65);
    rows.push(BoundCase {
        case: "a role name one character over its maximum".to_string(),
        action: "CreateRole",
        params: vec![
            param("RoleName", &long_name),
            param("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
        member: "roleName",
        requirement: above_length(64),
        echoed: vec![long_name],
    });

    // A COLLECTION above its `@length` maximum: the walk renders its SIZE where a scalar
    // renders its value, and neither is echoed.
    let mut tag_params = vec![
        param("RoleName", "tagged-role"),
        param("AssumeRolePolicyDocument", TRUST_POLICY),
    ];
    for position in 1..=51 {
        tag_params.push(param(&format!("Tags.member.{position}.Key"), &format!("k{position}")));
        tag_params.push(param(&format!("Tags.member.{position}.Value"), "v"));
    }
    rows.push(BoundCase {
        case: "one tag over the list's maximum".to_string(),
        action: "CreateRole",
        params: tag_params,
        member: "tags",
        requirement: above_length(50),
        echoed: vec!["51 items".to_string()],
    });

    rows
}

/// The parameters a session-duration row carries: the duration under test, plus whatever
/// else `action` requires.
fn session_params(action: &str, value: &str) -> Vec<(String, String)> {
    let mut params = vec![
        param("RoleName", "ranged-role"),
        param("MaxSessionDuration", value),
    ];
    if action == "CreateRole" {
        params.push(param("AssumeRolePolicyDocument", TRUST_POLICY));
    }
    params
}

/// RULE: a value outside a bound its member declares is refused 400 `ValidationError`, and
/// the clause names the member and the bound it broke WITHOUT echoing the value.
#[tokio::test]
async fn a_value_outside_a_declared_bound_is_refused_without_echoing_it() {
    let router = router();

    for BoundCase {
        case,
        action,
        params,
        member,
        requirement,
        echoed,
    } in bound_cases()
    {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
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
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             {requirement}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The rejected value is nowhere in the document.
        for value in &echoed {
            assert!(
                !body.contains(&format!("Value &apos;{value}&apos;"))
                    && !body.contains(&format!("Value '{value}'")),
                "case {case:?}: the clause must not echo {value:?}, got {body}"
            );
        }
    }
}

/// RULE: the refusal is about the BOUND, so each published bound itself is accepted — the
/// value-free clause is not a blanket refusal of the member.
#[tokio::test]
async fn each_published_bound_itself_is_accepted() {
    let router = router();

    // A role to drive the update rows against, so what they answer is about the bound and
    // not about a role that does not exist.
    let created = vec![
        param("RoleName", "bound-role"),
        param("AssumeRolePolicyDocument", TRUST_POLICY),
    ];
    let (status, body) = call(&router, "CreateRole", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "the fixture CreateRole must succeed, got {status}: {body}"
    );

    // Rows: both bounds of the page size, both bounds of the session duration, and both
    // bounds of a description's length — each on an operation that carries it.
    let longest_description = "d".repeat(1000);
    let rows = vec![
        (
            "the page-size minimum".to_string(),
            "ListInstanceProfiles",
            vec![param("MaxItems", "1")],
        ),
        (
            "the page-size maximum".to_string(),
            "ListInstanceProfiles",
            vec![param("MaxItems", "1000")],
        ),
        (
            "the session minimum".to_string(),
            "CreateRole",
            vec![
                param("RoleName", "min-session-role"),
                param("AssumeRolePolicyDocument", TRUST_POLICY),
                param("MaxSessionDuration", "3600"),
            ],
        ),
        (
            "the session maximum".to_string(),
            "CreateRole",
            vec![
                param("RoleName", "max-session-role"),
                param("AssumeRolePolicyDocument", TRUST_POLICY),
                param("MaxSessionDuration", "43200"),
            ],
        ),
        (
            "the session maximum on the update half".to_string(),
            "UpdateRole",
            vec![
                param("RoleName", "bound-role"),
                param("MaxSessionDuration", "43200"),
            ],
        ),
        (
            "the description length floor".to_string(),
            "UpdateRole",
            vec![param("RoleName", "bound-role"), param("Description", "")],
        ),
        (
            "the description length ceiling".to_string(),
            "UpdateRole",
            vec![
                param("RoleName", "bound-role"),
                param("Description", &longest_description),
            ],
        ),
        (
            "the description length ceiling on the description-only half".to_string(),
            "UpdateRoleDescription",
            vec![
                param("RoleName", "bound-role"),
                param("Description", &longest_description),
            ],
        ),
    ];
    for (case, action, params) in rows {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
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

/// RULE: a string outside a `@pattern` that is not one IAM states a sentence of its own
/// for is another constraint class and keeps the sentence the framework states, the
/// rejected value included — the value-free clause belongs to the classes this service
/// states that way (a declared bound, and an enumerated set), and the fixed sentence to the
/// forms IAM takes itself (`tests/all/member_format_refusal_wording.rs`).
#[tokio::test]
async fn another_constraint_class_keeps_the_frameworks_own_sentence() {
    let router = router();

    // Rows: a string outside its `@pattern` on three different members, across three
    // operations — each a pattern IAM publishes no sentence of its own for.
    let cases = vec![
        EchoedCase {
            case: "an access key id outside its pattern",
            action: "UpdateAccessKey",
            // `accessKeyIdType` is pattern ^[\w]+$ with length 16..128, so an id of that
            // length carrying a slash is outside the pattern alone.
            params: vec![
                ("UserName", "a-user"),
                ("AccessKeyId", "AKIA/NOT/VALID/ID"),
                ("Status", "Active"),
            ],
            member: "accessKeyId",
            value: "AKIA/NOT/VALID/ID",
        },
        EchoedCase {
            case: "an MFA device serial number outside its pattern",
            action: "DeactivateMFADevice",
            // `serialNumberType` is pattern ^[\w+=/:,.@-]+$ with length 9..256, so a value
            // of that length carrying a space is outside the pattern alone.
            params: vec![("UserName", "a-user"), ("SerialNumber", "has space 123")],
            member: "serialNumber",
            value: "has space 123",
        },
        EchoedCase {
            case: "a pagination marker outside its pattern",
            action: "ListRoles",
            // `markerType` is pattern ^[\u0020-\u00FF]+$, so a token carrying a character
            // above that range is outside it.
            params: vec![("Marker", "page\u{20AC}")],
            member: "marker",
            value: "page\u{20AC}",
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
        // these rows go nowhere near the bound rewording — so this pins the code
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

/// RULE: a request that breaks a declared bound AND another constraint is stated as both
/// clauses — the bound value-free, the other as the framework states it.
#[tokio::test]
async fn a_mixed_refusal_states_each_class_its_own_way() {
    let router = router();
    // `markerType` is pattern ^[\u0020-\u00FF]+$, so a token carrying a character above
    // that range is the second violation beside the page size's broken `@range` — and it
    // is a pattern IAM states no sentence of its own for, so both clauses are the walk's.
    let (status, body) = call(
        &router,
        "ListRoles",
        &[("Marker", "page\u{20AC}"), ("MaxItems", "0")],
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
            "Value at 'maxItems' failed to satisfy constraint: Member must have \
             value greater than or equal to 1"
        ),
        "expected the page-size clause to be stated without the number, got {message:?}"
    );
    assert!(
        message.contains("Value 'page\u{20AC}' at 'marker'"),
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
