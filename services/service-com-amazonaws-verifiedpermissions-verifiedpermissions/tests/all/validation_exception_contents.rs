// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for what a 400 `ValidationException` CARRIES, in the two
//! places this service fills one in.
//!
//! 1. A request that broke a constraint the MODEL declares is refused at the
//!    boundary, before any handler runs, and the sentence that refusal states
//!    addresses the offending member as a JSON pointer rooted at the request
//!    body, terminates the count with a period, and states nothing of the value
//!    that was submitted.
//! 2. Policy content a `STRICT` store's SCHEMA refuses reports ONE `fieldList`
//!    entry per diagnostic the Cedar validator raised about it — not just the
//!    first — so a client that reads the whole diagnostic set sees every problem
//!    with the policy at once.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;

use crate::harness::{
    call, error_code, fault_message, field_list, ok, router, store_with_mode, user_pool_arn,
};
use crate::schema::photo_app_schema;

// ---------------------------------------------------------------------------
// 1. The boundary refusal's sentence
// ---------------------------------------------------------------------------

/// `CognitoUserPoolConfiguration$userPoolArn`'s `@pattern`, as the model writes
/// it — anchors included (`smithy_query(query="shape",
/// shape="CognitoUserPoolConfiguration")`).
const USER_POOL_ARN_PATTERN: &str =
    r"^arn:[a-zA-Z0-9-]+:cognito-idp:(([a-zA-Z0-9-]+:\d{12}:userpool/[\w-]+_[0-9a-zA-Z]+))$";

/// `OpenIdConnectConfiguration$issuer`'s `@pattern`
/// (`smithy_query(query="shape", shape="OpenIdConnectConfiguration")`).
const ISSUER_PATTERN: &str = r"^https://.*$";

/// `UserPoolArn`'s `@length` maximum (`smithy_query(query="shape",
/// shape="CognitoUserPoolConfiguration")`).
const USER_POOL_ARN_MAX_LENGTH: usize = 255;

/// A well-formed Cognito user pool ARN that is `bytes` long, reached by padding
/// the POOL ID — so the only constraint such a value can break is the length
/// maximum, never the pattern.
fn user_pool_arn_of(bytes: usize) -> String {
    let base = user_pool_arn("us-east-1", "us-east-1_");
    format!(
        "{base}{}",
        "a".repeat(bytes.saturating_sub(base.chars().count()))
    )
}

/// The clause one broken constraint states: the member as a JSON pointer, the
/// requirement it failed, and nothing of the submitted value.
fn clause(pointer: &str, requirement: &str) -> String {
    format!("Value at '{pointer}' failed to satisfy constraint: {requirement}")
}

/// RULE: a request that broke a constraint the model declares is refused 400
/// `ValidationException`, and the message is
/// `N validation error(s) detected. <clause>; …` — a PERIOD after the count, each
/// clause naming its member as a slash-delimited JSON pointer rooted at the
/// request body, and no clause echoing the value that was submitted.
///
/// The rows span the operations, the depth the offending member sits at (top
/// level, inside a union arm, inside the second element of a list), and the
/// constraint class that was broken (a pattern, a length maximum, a length
/// minimum, an absent required member).
#[tokio::test]
async fn a_boundary_refusal_addresses_the_member_as_a_pointer_and_withholds_the_value() {
    let router = router();
    let store_id = store_with_mode(&router, "OFF").await;
    let over_long_arn = user_pool_arn_of(USER_POOL_ARN_MAX_LENGTH + 1);

    // (case, operation, request, expected message, the submitted value the
    //  message must NOT echo)
    let cases: Vec<(&str, &str, Value, String, Option<&str>)> = vec![
        (
            // The recorded case: an ARN that is not a user pool ARN at all.
            "a pattern broken by a member inside a union arm",
            "CreateIdentitySource",
            json!({
                "policyStoreId": store_id,
                "configuration": { "cognitoUserPoolConfiguration": {
                    "userPoolArn": "arn:aws:s3:::not-a-user-pool-us-west-2" } },
            }),
            format!(
                "1 validation error detected. {}",
                clause(
                    "/configuration/cognitoUserPoolConfiguration/userPoolArn",
                    &format!(
                        "Member must satisfy regular expression pattern: {USER_POOL_ARN_PATTERN}"
                    ),
                )
            ),
            Some("arn:aws:s3:::not-a-user-pool-us-west-2"),
        ),
        (
            // The same rule on the OTHER arm of the same union, so the pointer
            // follows the arm the request carried rather than one arm's names.
            "a pattern broken by a member inside the other union arm",
            "CreateIdentitySource",
            json!({
                "policyStoreId": store_id,
                "principalEntityType": "PhotoApp::User",
                "configuration": { "openIdConnectConfiguration": {
                    "issuer": "http://idp.example.com",
                    "tokenSelection": { "identityTokenOnly": {} } } },
            }),
            format!(
                "1 validation error detected. {}",
                clause(
                    "/configuration/openIdConnectConfiguration/issuer",
                    &format!("Member must satisfy regular expression pattern: {ISSUER_PATTERN}"),
                )
            ),
            Some("http://idp.example.com"),
        ),
        (
            // A length MAXIMUM, on a value that satisfies the pattern: only its
            // size is wrong, so the clause states the size requirement.
            "a length maximum broken by a member inside a union arm",
            "CreateIdentitySource",
            json!({
                "policyStoreId": store_id,
                "configuration": { "cognitoUserPoolConfiguration": {
                    "userPoolArn": over_long_arn } },
            }),
            format!(
                "1 validation error detected. {}",
                clause(
                    "/configuration/cognitoUserPoolConfiguration/userPoolArn",
                    &format!(
                        "Member must have length less than or equal to {USER_POOL_ARN_MAX_LENGTH}"
                    ),
                )
            ),
            Some(over_long_arn.as_str()),
        ),
        (
            // A length MINIMUM, on a top-level member of a read operation.
            "a length minimum broken by a top-level member",
            "GetPolicy",
            json!({ "policyStoreId": "", "policyId": "SPEXAMPLE" }),
            format!(
                "1 validation error detected. {}",
                clause(
                    "/policyStoreId",
                    "Member must have length greater than or equal to 1",
                )
            ),
            None,
        ),
        (
            // An absent required member: the same envelope and the same pointer
            // form, with the requirement AWS states for absence.
            "a required top-level member absent from the request",
            "CreatePolicyStore",
            json!({}),
            format!(
                "1 validation error detected. {}",
                clause("/validationSettings", "Member must not be null")
            ),
            None,
        ),
        (
            // A member inside the SECOND element of a list: the pointer names
            // the element's own 0-based position, with no `member` step.
            "a length minimum broken inside the second element of a list",
            "BatchIsAuthorized",
            json!({
                "policyStoreId": store_id,
                "requests": [
                    {
                        "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation" },
                    },
                    {
                        "principal": { "entityType": "PhotoApp::User", "entityId": "" },
                        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation" },
                    },
                ],
            }),
            format!(
                "1 validation error detected. {}",
                clause(
                    "/requests/1/principal/entityId",
                    "Member must have length greater than or equal to 1",
                )
            ),
            None,
        ),
    ];

    for (case, operation, request, expected, withheld) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(status, 400, "case {case:?}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the ValidationException code, got {body}"
        );
        let stated = fault_message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected message {expected:?}, got {stated:?}"
        );
        if let Some(value) = withheld {
            assert!(
                !stated.contains(value),
                "case {case:?}: expected the submitted value to be withheld, got {stated:?}"
            );
        }
    }
}

/// RULE (the same one, on a request that broke TWO constraints): the count is
/// pluralised and still followed by a period, and the message carries one clause
/// per violation, each addressing its own member as a pointer.
#[tokio::test]
async fn a_boundary_refusal_states_one_clause_per_broken_constraint() {
    let router = router();
    let (status, body) = call(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": "", "policyId": "" }),
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {body}");
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected the ValidationException code, got {body}"
    );
    let stated = fault_message(&body);
    assert!(
        stated.starts_with("2 validation errors detected. "),
        "expected the plural envelope terminated with a period, got {stated:?}"
    );
    for pointer in ["/policyStoreId", "/policyId"] {
        let expected = clause(pointer, "Member must have length greater than or equal to 1");
        assert!(
            stated.contains(&expected),
            "expected a clause {expected:?} for {pointer}, got {stated:?}"
        );
    }
    assert!(
        stated.contains("; "),
        "expected the two clauses separated by '; ', got {stated:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. Every reason the schema raised
// ---------------------------------------------------------------------------

/// RULE: a policy the store's SCHEMA refuses in `STRICT` mode is answered with
/// ONE `fieldList` entry per diagnostic the Cedar validator raised about it —
/// every one of them, not only the first — each naming the request member whose
/// content was validated, and a policy the validator raises NOTHING about is
/// stored.
///
/// The rows span the classes of diagnostic the service's own
/// `ValidationException` documentation enumerates (an action id the schema does
/// not declare, entity types it does not declare, an action applied to a
/// principal and resource the schema does not sanction, a condition that can
/// never hold), the operation that validates (policy create, policy update,
/// template create), and HOW MANY diagnostics one policy raises — which is what
/// pins "every reason" rather than "one".
#[tokio::test]
async fn a_strict_schema_refusal_reports_every_reason_cedar_raised() {
    let router = router();
    let store_id = store_with_mode(&router, "STRICT").await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;

    // A policy the schema sanctions, so the UpdatePolicy row has something to
    // replace — and proof that a policy the validator raises nothing about is
    // stored rather than refused.
    let created = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement":
                "permit(principal == PhotoApp::User::\"alice\", \
                 action == PhotoApp::Action::\"viewPhoto\", \
                 resource == PhotoApp::Photo::\"vacation.jpg\");" } },
        }),
    )
    .await;
    let policy_id = created["policyId"].as_str().unwrap_or_default().to_string();

    // (case, operation, request, the validated member, the fragment each
    //  reported reason must state, in order)
    let cases: Vec<(&str, &str, Value, &str, Vec<&str>)> = vec![
        (
            // The recorded case: Cedar reports BOTH the action id it does not
            // know and the scope that therefore applies to nothing.
            "an action id the schema does not declare",
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::User::\"alice\", \
                     action == PhotoApp::Action::\"deletePhoto\", \
                     resource == PhotoApp::Photo::\"vacation.jpg\");" } },
            }),
            "/definition",
            vec![
                "unrecognized action `PhotoApp::Action::\"deletePhoto\"`",
                "unable to find an applicable action given the policy scope constraints",
            ],
        ),
        (
            // Two undeclared entity types raise one reason each, and the scope
            // they leave applies to nothing: three reasons, three entries.
            "two entity types the schema does not declare",
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::Ghost::\"g\", \
                     action == PhotoApp::Action::\"viewPhoto\", \
                     resource == PhotoApp::Album::\"a\");" } },
            }),
            "/definition",
            vec![
                "unrecognized entity type `PhotoApp::Ghost`",
                "unrecognized entity type `PhotoApp::Album`",
                "unable to find an applicable action given the policy scope constraints",
            ],
        ),
        (
            // An action the schema declares, applied to a principal and a
            // resource it does not sanction for that action.
            "an action applied to a principal and resource the schema does not sanction",
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::Photo::\"vacation.jpg\", \
                     action == PhotoApp::Action::\"viewPhoto\", \
                     resource == PhotoApp::User::\"alice\");" } },
            }),
            "/definition",
            vec!["unable to find an applicable action given the policy scope constraints"],
        ),
        (
            // A condition that can never hold: the policy could never affect a
            // decision, which is its own reason.
            "a condition that always evaluates to false",
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::User::\"alice\", \
                     action == PhotoApp::Action::\"viewPhoto\", \
                     resource == PhotoApp::Photo::\"vacation.jpg\") \
                     when { 1 == 2 };" } },
            }),
            "/definition",
            vec!["policy is impossible"],
        ),
        (
            // The same rule on the UPDATE path: only the action changes, so the
            // schema is what refuses the replacement.
            "an update whose action the schema does not declare",
            "UpdatePolicy",
            json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::User::\"alice\", \
                     action == PhotoApp::Action::\"deletePhoto\", \
                     resource == PhotoApp::Photo::\"vacation.jpg\");" } },
            }),
            "/definition",
            vec![
                "unrecognized action `PhotoApp::Action::\"deletePhoto\"`",
                "unable to find an applicable action given the policy scope constraints",
            ],
        ),
        (
            // And on a policy TEMPLATE, which the same validation covers — at
            // the member whose content a template operation validates.
            "a template whose action the schema does not declare",
            "CreatePolicyTemplate",
            json!({
                "policyStoreId": store_id,
                "statement": "permit(principal == ?principal, \
                              action == PhotoApp::Action::\"deletePhoto\", \
                              resource == PhotoApp::Photo::\"vacation.jpg\");",
            }),
            "/template/statement",
            vec![
                "unrecognized action `PhotoApp::Action::\"deletePhoto\"`",
                "unable to find an applicable action given the policy scope constraints",
            ],
        ),
    ];

    for (case, operation, request, path, fragments) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(status, 400, "case {case:?}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the ValidationException code, got {body}"
        );
        assert_eq!(
            fault_message(&body),
            INVALID_INPUT_MESSAGE,
            "case {case:?}: expected the fixed summary message, got {body}"
        );
        let fields = field_list(&body);
        assert_eq!(
            fields.len(),
            fragments.len(),
            "case {case:?}: expected one fieldList entry per reason ({}), got {fields:?}",
            fragments.len()
        );
        for (index, fragment) in fragments.iter().enumerate() {
            let (stated_path, stated_message) = &fields[index];
            assert_eq!(
                stated_path, path,
                "case {case:?}: expected entry {index} at {path:?}, got {fields:?}"
            );
            assert!(
                stated_message.contains(fragment),
                "case {case:?}: expected entry {index} to state {fragment:?}, got {fields:?}"
            );
        }
    }
}
