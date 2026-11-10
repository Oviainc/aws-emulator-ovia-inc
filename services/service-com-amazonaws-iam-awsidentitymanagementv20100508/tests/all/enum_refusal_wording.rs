// SPDX-License-Identifier: CC0-1.0

//! How this service words a request that named a value outside an enumerated set, end to
//! end over the awsQuery wire.
//!
//! The rule under test: a value outside the set its member's enum declares is refused with
//! HTTP 400 `ValidationError` whose message names the member in its wire (lower-camel)
//! form and the admissible set, and NOTHING of the rejected value — `1 validation error
//! detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum
//! value set: [<the admissible values>]` — with the set rendered in the order this service
//! publishes for that enum. The other classes of boundary refusal (a string outside its
//! `@pattern`, an absent required member) keep the sentence the framework states, the
//! rejected value included; `tests/all/range_refusal_wording.rs` covers the bound class,
//! which is worded the same value-free way.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! member carries the enum and which operation takes it, how many values the enum
//! declares, the form of the rejected value (another case of an admissible value, a value
//! from a different enum, an empty string), and which other members the same request
//! carries. No single request is the specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share.

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// One row: the call to make, the member path the clause must name, the admissible set the
/// clause must state, and the value the request carried, which the clause must NOT echo.
struct EnumCase {
    case: String,
    action: &'static str,
    params: Vec<(String, String)>,
    member: &'static str,
    admissible: &'static [&'static str],
    rejected: String,
}

/// The admissible set of a managed-policy listing's `Scope` (`iam#policyScopeType`), and
/// of the account's global-endpoint token version
/// (`iam#globalEndpointTokenVersion`), in the order this service states each.
///
/// These are the enums of this service whose published order is not the order the model
/// declares the values in (`All, AWS, Local` and `v1Token, v2Token` respectively); the
/// renderer's order is replaced by them in `src/service.rs`.
const POLICY_SCOPE: &[&str] = &["All", "Local", "AWS"];
const GLOBAL_ENDPOINT_TOKEN_VERSION: &[&str] = &["v2Token", "v1Token"];

/// The admissible sets of the other enums under test, each in the model's declaration
/// order (`smithy_query(query="shape",
/// shape="PolicyUsageType,assignmentStatusType,statusType,EntityType,encodingType")`),
/// which is the order the clause states when this service publishes no order of its own.
///
/// The model order is asserted here DELIBERATELY: the service's refusal states a set in
/// the model's order unless an order AWS itself states for that enum is carried in
/// `src/service.rs`'s published-order table, and no such statement is in hand for these
/// enums. A row here must not be reordered, and no entry added to that table, from a
/// guess about what AWS would print.
const POLICY_USAGE: &[&str] = &["PermissionsPolicy", "PermissionsBoundary"];
const ASSIGNMENT_STATUS: &[&str] = &["Assigned", "Unassigned", "Any"];
const ACCESS_KEY_STATUS: &[&str] = &["Active", "Inactive", "Expired"];
const ENTITY_TYPE: &[&str] = &["User", "Role", "Group", "LocalManagedPolicy", "AWSManagedPolicy"];
const KEY_ENCODING: &[&str] = &["SSH", "PEM"];

/// An access key id the request's own member constraints admit (`accessKeyIdType`, length
/// 16..128, pattern `^[\w]+$`), so the enum is the only thing the request breaks.
const ACCESS_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// An SSH public key id the request's own member constraints admit (`publicKeyIdType`,
/// length 20..128, pattern `^[\w]+$`).
const SSH_PUBLIC_KEY_ID: &str = "APKAEIBAERJR2EXAMPLE";

/// The clause this service states for a value outside an enumerated set.
fn expected_message(member: &str, admissible: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        admissible.join(", ")
    )
}

/// The rows: every enum-bearing member of this service's policy, MFA, access-key, SSH-key
/// and account-setting surface, refused with a value outside its set.
fn enum_cases() -> Vec<EnumCase> {
    let policy_arn = format!("arn:aws:iam::{}:policy/a-policy-not-held", account_id());
    vec![
        // The member the recorded refusal named, and the two further forms of a rejected
        // value: a differently cased admissible value, and the empty string.
        EnumCase {
            case: "a listing scope outside its set".to_string(),
            action: "ListPolicies",
            params: vec![param("Scope", "Bogus")],
            member: "scope",
            admissible: POLICY_SCOPE,
            rejected: "Bogus".to_string(),
        },
        EnumCase {
            case: "a listing scope in the wrong case".to_string(),
            action: "ListPolicies",
            params: vec![param("Scope", "aws")],
            member: "scope",
            admissible: POLICY_SCOPE,
            rejected: "aws".to_string(),
        },
        EnumCase {
            case: "a listing scope beside another member".to_string(),
            action: "ListPolicies",
            params: vec![param("PathPrefix", "/eng/"), param("Scope", "Everything")],
            member: "scope",
            admissible: POLICY_SCOPE,
            rejected: "Everything".to_string(),
        },
        // Another enum on the SAME operation: a two-value set, whose order no publication
        // of this service's overrides.
        EnumCase {
            case: "a policy usage filter outside its set".to_string(),
            action: "ListPolicies",
            params: vec![param("PolicyUsageFilter", "PermissionsEverything")],
            member: "policyUsageFilter",
            admissible: POLICY_USAGE,
            rejected: "PermissionsEverything".to_string(),
        },
        // The same enum reached through a different operation.
        EnumCase {
            case: "a policy usage filter on the entity listing".to_string(),
            action: "ListEntitiesForPolicy",
            params: vec![
                param("PolicyArn", &policy_arn),
                param("PolicyUsageFilter", "Boundary"),
            ],
            member: "policyUsageFilter",
            admissible: POLICY_USAGE,
            rejected: "Boundary".to_string(),
        },
        // A five-value set.
        EnumCase {
            case: "an entity filter outside its set".to_string(),
            action: "ListEntitiesForPolicy",
            params: vec![
                param("PolicyArn", &policy_arn),
                param("EntityFilter", "ServiceLinkedRole"),
            ],
            member: "entityFilter",
            admissible: ENTITY_TYPE,
            rejected: "ServiceLinkedRole".to_string(),
        },
        // A three-value set on a listing of another resource.
        EnumCase {
            case: "an MFA assignment status outside its set".to_string(),
            action: "ListVirtualMFADevices",
            params: vec![param("AssignmentStatus", "Allocated")],
            member: "assignmentStatus",
            admissible: ASSIGNMENT_STATUS,
            rejected: "Allocated".to_string(),
        },
        // A REQUIRED enum member, supplied with a value outside its set: the refusal is
        // the enum class, not an absent member.
        EnumCase {
            case: "an access key status outside its set".to_string(),
            action: "UpdateAccessKey",
            params: vec![
                param("UserName", "a-user"),
                param("AccessKeyId", ACCESS_KEY_ID),
                param("Status", "Disabled"),
            ],
            member: "status",
            admissible: ACCESS_KEY_STATUS,
            rejected: "Disabled".to_string(),
        },
        EnumCase {
            case: "an SSH key encoding outside its set".to_string(),
            action: "GetSSHPublicKey",
            params: vec![
                param("UserName", "a-user"),
                param("SSHPublicKeyId", SSH_PUBLIC_KEY_ID),
                param("Encoding", "OpenSSH"),
            ],
            member: "encoding",
            admissible: KEY_ENCODING,
            rejected: "OpenSSH".to_string(),
        },
        // A two-value set whose PUBLISHED order is not the model's declaration order, on
        // the one required member of an account-wide setting: the set is stated in the
        // order AWS states it, which is what a client matching the clause reads.
        EnumCase {
            case: "a global endpoint token version outside its set".to_string(),
            action: "SetSecurityTokenServicePreferences",
            params: vec![param("GlobalEndpointTokenVersion", "v3Token")],
            member: "globalEndpointTokenVersion",
            admissible: GLOBAL_ENDPOINT_TOKEN_VERSION,
            rejected: "v3Token".to_string(),
        },
    ]
}

/// RULE: a value outside the set its member's enum declares is refused 400
/// `ValidationError`, and the clause names the member and the admissible set WITHOUT
/// echoing the value.
#[tokio::test]
async fn a_value_outside_an_enum_set_is_refused_without_echoing_it() {
    let router = router();

    for EnumCase {
        case,
        action,
        params,
        member,
        admissible,
        rejected,
    } in enum_cases()
    {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the query protocol's refusal code, got {body}"
        );
        let expected = expected_message(member, admissible);
        let message = element(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The rejected value is nowhere in the document, however it was spelled.
        assert!(
            !body.contains(&format!("Value &apos;{rejected}&apos;"))
                && !body.contains(&format!("Value '{rejected}'")),
            "case {case:?}: the clause must not echo {rejected:?}, got {body}"
        );
    }
}

/// One row of the accepted table: what the row is called, the call to make, and the
/// parameters it carries.
struct AcceptedCase {
    case: String,
    action: &'static str,
    params: Vec<(String, String)>,
}

/// RULE: the refusal is about the value being outside the set, so every value the set
/// DOES declare is accepted — the value-free clause is not a blanket refusal of the
/// member.
#[tokio::test]
async fn every_admissible_value_of_each_set_is_accepted() {
    let router = router();

    // Rows: each admissible value of each enum this service's listing surface takes,
    // including the ones the suite above never names. The enums whose operation is still
    // a scaffolded 501, or needs a credential this suite does not hold, are covered by
    // the refusal table above only: the refusal is stated before dispatch, so it is
    // reachable where the 200 is not.
    let mut rows: Vec<AcceptedCase> = Vec::new();
    for value in POLICY_SCOPE {
        rows.push(AcceptedCase {
            case: format!("Scope={value}"),
            action: "ListPolicies",
            params: vec![param("Scope", value)],
        });
    }
    for value in POLICY_USAGE {
        rows.push(AcceptedCase {
            case: format!("PolicyUsageFilter={value}"),
            action: "ListPolicies",
            params: vec![param("PolicyUsageFilter", value)],
        });
    }
    // An AWS-managed policy the catalog carries, so the listing resolves a policy that
    // exists and the row is about the enum value alone.
    let carried_policy = "arn:aws:iam::aws:policy/AdministratorAccess";
    for value in ENTITY_TYPE {
        rows.push(AcceptedCase {
            case: format!("EntityFilter={value}"),
            action: "ListEntitiesForPolicy",
            params: vec![
                param("PolicyArn", carried_policy),
                param("EntityFilter", value),
            ],
        });
    }

    for AcceptedCase {
        case,
        action,
        params,
    } in rows
    {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status}: {body}"
        );
    }
}

/// RULE: the value is withheld from EVERY clause — a request that names an admissible value
/// of the wrong enum on one member and breaks a `@pattern` on another is stated as both
/// clauses, the enum one naming the set and the pattern one naming the expression AWS
/// publishes, and neither naming what the request carried.
#[tokio::test]
async fn a_mixed_refusal_states_each_class_its_own_way() {
    let router = router();
    // `accessKeyIdType` is pattern ^[\w]+$ (length 16..128), so an id of that length
    // carrying a slash is the second violation — and it is a pattern IAM states no
    // sentence of its own for, so the clause is the walk's, which quotes the expression
    // without the whole-value anchors (`tests/all/validation_error_clauses.rs`).
    let malformed_key_id = "AKIA/NOT/VALID/ID";
    let (status, body) = call(
        &router,
        "UpdateAccessKey",
        &[
            ("UserName", "a-user"),
            ("AccessKeyId", malformed_key_id),
            ("Status", "Disabled"),
        ],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status}: {body}");
    let message = element(&body, "Message");
    assert!(
        message.starts_with("2 validation errors detected: "),
        "expected both violations to be stated, got {message:?}"
    );
    assert!(
        message.contains(&format!(
            "Value at 'status' failed to satisfy constraint: Member must satisfy enum value \
             set: [{}]",
            ACCESS_KEY_STATUS.join(", ")
        )),
        "expected the enum clause to be stated without the value, got {message:?}"
    );
    let pattern_clause = concat!(
        r"Value at 'accessKeyId' failed to satisfy constraint: ",
        r"Member must satisfy regular expression pattern: [\w]+"
    );
    assert!(
        message.contains(pattern_clause),
        "expected the pattern clause to be stated without the value, got {message:?}"
    );
    assert!(
        !message.contains(malformed_key_id),
        "expected no clause to echo {malformed_key_id:?}, got {message:?}"
    );
}
