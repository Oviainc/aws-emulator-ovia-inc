// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `AccessDenied` message IAM answers a refused call with.
//!
//! The rule under test: the message is ONE sentence — `User: ` + the ARN of the principal
//! that signed the request + ` is not authorized to perform: ` + the IAM action the call
//! was decided on + ` on resource: ` + the resource it was decided against + ` ` + why the
//! decision went that way — and it ENDS there. Nothing is appended to it: no further
//! sentence, no pointer at somewhere else to read, and no value that is minted per
//! refusal, so the same refused call is worded identically on every run and whichever
//! Region it addressed.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation is refused, whether the resource is an ARN, a path-scoped collection, the
//! account-wide `*` or a named entity, and which of the two refusals it is (an explicit
//! `Deny` about the request, or no `Allow` for it).
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so a request is built and read exactly one way.

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call_signed_by, call_signed_in_region, element,
    error_code, router,
};

/// The reason clause for a call no statement in force allows — the implicit deny.
fn no_allow(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The reason clause for a call a statement about it explicitly denies.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// An `Allow` for every IAM action on every resource: what a row about an explicit `Deny`
/// is layered on top of, so the row demonstrates the `Deny` and not a missing grant.
const ALLOW_EVERYTHING: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:*","Resource":"*"}]}"#;

/// One row: the documents the calling user holds, the awsQuery call it makes, and the
/// three variable parts the refusal must name.
struct Case {
    case: &'static str,
    /// `(PolicyName, PolicyDocument)` pairs put on the caller in this order.
    policies: Vec<(String, String)>,
    /// The awsQuery `Action` and its form parameters.
    action: &'static str,
    params: Vec<(String, String)>,
    /// The IAM action name the decision is made on.
    iam_action: &'static str,
    /// The resource as the refusal NAMES it: an ARN for a request that addresses a
    /// collection, the entity's type and name for one that addresses a single entity.
    named_as: String,
    /// The clause closing the sentence.
    reason: String,
}

/// The rows. Each runs against its own fresh IAM user, so the documents a row puts are
/// the only ones in force when it calls.
fn cases() -> Vec<Case> {
    let account = account_id();
    vec![
        // A collection read with no parameters: the account's users at the root path.
        Case {
            case: "implicit deny on a collection read",
            policies: Vec::new(),
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
        },
        // The same read with a page size: a parameter that does not touch the decision
        // must not touch the wording either.
        Case {
            case: "implicit deny on a collection read asking for one item",
            policies: Vec::new(),
            action: "ListUsers",
            params: vec![("MaxItems".to_string(), "1".to_string())],
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
        },
        // A path-scoped collection: the resource the refusal names is the prefix the
        // request addressed, not the root.
        Case {
            case: "implicit deny on a path-scoped collection read",
            policies: Vec::new(),
            action: "ListGroups",
            params: vec![("PathPrefix".to_string(), "/eng/tools/".to_string())],
            iam_action: "iam:ListGroups",
            named_as: format!("arn:aws:iam::{account}:group/eng/tools/"),
            reason: no_allow("iam:ListGroups"),
        },
        // Another collection and another action, to show neither is special-cased.
        Case {
            case: "implicit deny listing roles",
            policies: Vec::new(),
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            named_as: format!("arn:aws:iam::{account}:role/"),
            reason: no_allow("iam:ListRoles"),
        },
        // A request that addresses the account's whole configuration: the resource is
        // the account-wide one, so the message carries no ARN at all.
        Case {
            case: "implicit deny on an account-wide read",
            policies: Vec::new(),
            action: "GetAccountAuthorizationDetails",
            params: Vec::new(),
            iam_action: "iam:GetAccountAuthorizationDetails",
            named_as: "*".to_string(),
            reason: no_allow("iam:GetAccountAuthorizationDetails"),
        },
        // A request that addresses ONE entity by name: the refusal names the entity.
        Case {
            case: "implicit deny reading one group by name",
            policies: Vec::new(),
            action: "GetGroup",
            params: vec![("GroupName".to_string(), "payroll".to_string())],
            iam_action: "iam:GetGroup",
            named_as: "group payroll".to_string(),
            reason: no_allow("iam:GetGroup"),
        },
        // An explicit `Deny` over a grant for everything: the other reason clause, on a
        // collection read.
        Case {
            case: "explicit deny on a collection read",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-listusers".to_string(),
                    statement("Deny", "iam:ListUsers", "*"),
                ),
            ],
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: EXPLICIT_DENY.to_string(),
        },
        // An explicit `Deny` on a request that addresses one entity by name.
        Case {
            case: "explicit deny reading one group by name",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-getgroup".to_string(),
                    statement("Deny", "iam:GetGroup", "*"),
                ),
            ],
            action: "GetGroup",
            params: vec![("GroupName".to_string(), "payroll".to_string())],
            iam_action: "iam:GetGroup",
            named_as: "group payroll".to_string(),
            reason: EXPLICIT_DENY.to_string(),
        },
        // An explicit `Deny` on the account-wide read.
        Case {
            case: "explicit deny on an account-wide read",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-details".to_string(),
                    statement("Deny", "iam:GetAccountAuthorizationDetails", "*"),
                ),
            ],
            action: "GetAccountAuthorizationDetails",
            params: Vec::new(),
            iam_action: "iam:GetAccountAuthorizationDetails",
            named_as: "*".to_string(),
            reason: EXPLICIT_DENY.to_string(),
        },
    ]
}

/// RULE: a refusal's message is the single sentence naming the principal, the action and
/// the resource the decision was made on, closed by why it went that way — and it ends
/// there, carrying no appended clause.
#[tokio::test]
async fn a_refusal_is_worded_as_one_sentence_that_ends_at_the_reason() {
    let router = router();

    for (row, case) in cases().into_iter().enumerate() {
        let user_name = format!("denied{row}");
        let access_key_id = principal(&router, case.case, &user_name, &case.policies).await;
        let params: Vec<(&str, &str)> = case
            .params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();

        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), case.action, &params).await;

        assert_eq!(
            status, 403,
            "case {:?}: expected HTTP 403, got {status}: {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {:?}: expected the error code AccessDenied, got {:?}",
            case.case,
            error_code(&body)
        );

        let message = element(&body, "Message");
        assert_access_denied_message(
            case.case,
            &message,
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            case.iam_action,
            &case.named_as,
            &case.reason,
        );
        // The sentence ends at the reason: nothing follows it, so there is no second
        // sentence to find and no character after the reason's last word.
        assert!(
            message.ends_with(&case.reason),
            "case {:?}: expected the message to end at the reason {:?}, got {message:?}",
            case.case,
            case.reason
        );
        assert_eq!(
            message.matches(". ").count(),
            0,
            "case {:?}: expected one sentence with nothing appended, got {message:?}",
            case.case
        );
    }
}

/// RULE: every part of the message is a fact of the request and of the documents in
/// force, so repeating the same refused call — and making it in another Region — answers
/// the identical message.
#[tokio::test]
async fn the_same_refused_call_is_worded_identically_every_time() {
    let router = router();
    let user_name = "repeated";
    // No identity-based policy at all, so every call below is the implicit deny.
    let access_key_id = principal(&router, "repeated refusals", user_name, &[]).await;

    let mut seen: Vec<String> = Vec::new();
    // Rows spanning repeated calls and Regions in three of AWS's geographies, including
    // the one IAM's global endpoint lives in: a message carrying a per-refusal value, or
    // anything drawn from the request's Region, would differ across them.
    for region in ["us-east-1", "us-east-1", "eu-west-1", "ap-southeast-2"] {
        let (status, body) =
            call_signed_in_region(&router, Some(&access_key_id), region, "ListUsers", &[]).await;
        assert_eq!(
            status, 403,
            "case {region:?}: expected the implicit deny to be refused, got {status}: {body}"
        );
        seen.push(element(&body, "Message"));
    }

    let expected = format!(
        "User: arn:aws:iam::{account}:user/{user_name} is not authorized to perform: \
         iam:ListUsers on resource: arn:aws:iam::{account}:user/ {}",
        no_allow("iam:ListUsers"),
        account = account_id(),
    );
    for (attempt, message) in seen.iter().enumerate() {
        assert_eq!(
            message, &expected,
            "case attempt {attempt}: expected every refusal of the same call to be worded \
             alike, got {message:?}"
        );
    }
}
