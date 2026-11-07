// SPDX-License-Identifier: CC0-1.0

//! What the FUNCTION's OWN resource-based policy says about an invocation — the third
//! complement of `tests/all/invoke_identity_authorization.rs` (which owns what the CALLER's
//! policies say) and of `tests/all/invoke_authorization.rs` (which owns the ordering of the
//! decision) and `tests/all/invoke_authorization_ceiling.rs` (the boundary above a grant). It
//! drives its principals through the fixtures `invoke_authorization` publishes, so there is
//! one way a principal is made in this crate.
//!
//! RULE: within one account the two kinds of document are evaluated together — "the
//! resource-based policy and the identity-based policy are evaluated together, and access is
//! granted if either allows it" — so an explicit `Deny` in the function's own policy refuses
//! an invocation whatever grants it, INCLUDING the account's own published credential, which
//! nothing else in the decision bounds; a statement of that policy that names some other
//! principal grants the caller nothing; one that names only the ACCOUNT delegates to that
//! account's identity-based policies rather than granting ("you must also attach an
//! identity-based policy"); and a grant it DID make stops permitting the invocation the moment
//! `RemovePermission` takes the statement out, because the decision is made on the documents
//! as they stand at the instant of the call.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! beside Lambda on ONE engine, writes the function's policy through Lambda's own
//! `PutResourcePolicy` / `DeleteResourcePolicy` / `AddPermission` / `RemovePermission`, and
//! reads the decision off the invocation itself — a served row answering what the handler
//! returned, so a row that was permitted but never ran fails here.
//!
//! The rows differ along what the rule generalises over: WHOM the statement names (every
//! principal, the caller itself, another principal of the account, the account root), its
//! EFFECT (a deny, a grant), and WHO invokes (an IAM user whose own policies allow the action,
//! one whose policies allow nothing, and the account's own published credential).

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::router_with_iam;
use crate::event_source_mappings::{call, Answer};
use crate::invoke_authorization::{
    assert_denied, assert_invoked, document, function_arn, invoke_as, principal, refusal,
    statement, user_arn, ACCOUNT, FUNCTIONS_URI, HANDLER_SOURCE, INVOKE_ACTION,
};

/// Where the resource-policy operations are bound.
const RESOURCE_POLICY_URI: &str = "/2026-07-09/resource-policy";

/// The function every row invokes.
const FUNCTION: &str = "orders";

/// How long past the due instant a row waits before invoking. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The reason a refusal states when nothing the caller carries allows the invocation, and the
/// reason it states when the function's own policy denies it outright.
fn no_identity_grant() -> String {
    format!("because no identity-based policy allows the {INVOKE_ACTION} action")
}
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

/// One resource-based policy document: `effect` for `principal` on this function.
fn function_policy(effect: &str, principal: Value) -> String {
    document(json!([{
        "Sid": "row",
        "Effect": effect,
        "Principal": principal,
        "Action": INVOKE_ACTION,
        "Resource": function_arn(FUNCTION),
    }]))
}

/// Create the function every row invokes and wait out the provisioning window the create
/// documents, so a permitted invocation is one the function's lifecycle serves.
async fn given_active_function(router: &MultiServiceRouter) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": FUNCTION,
            "Role": format!("arn:aws:iam::{ACCOUNT}:role/service-role/lambda-exec"),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 5,
            "Code": {
                "ZipFile": ::base64::Engine::encode(
                    &::base64::engine::general_purpose::STANDARD,
                    crate::synchronous_invoke_run::package(HANDLER_SOURCE),
                ),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case \"the fixture CreateFunction\": expected 201, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Install `policy` as the whole resource policy of the function, or DELETE the policy when
/// the row installs none — both through the service's own API, as the account's own
/// credential.
async fn set_function_policy(router: &MultiServiceRouter, case: &str, policy: Option<&str>) {
    let uri = format!("{RESOURCE_POLICY_URI}/{}", function_arn(FUNCTION));
    let (answer, expected): (Answer, u16) = match policy {
        Some(policy) => (
            call(router, "PUT", &uri, &json!({ "Policy": policy })).await,
            200,
        ),
        None => (call(router, "DELETE", &uri, &Value::Null).await, 204),
    };
    assert_eq!(
        answer.status,
        expected,
        "case {case:?}: writing the function policy must answer {expected}, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Who a row invokes as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// An IAM user of the account, with a key IAM minted for it.
    User,
    /// The account's own published credential, which holds no IAM policies and owns its
    /// functions — the caller the rest of the decision does not bound.
    Account,
}

/// One row of the table.
struct Row {
    case: &'static str,
    caller: Caller,
    /// The inline policy the row's user holds, or `None` for one holding none.
    identity: Option<String>,
    /// The whole resource policy the row installs on the function.
    policy: String,
    /// The clause the refusal carries, or `None` when the invocation is served.
    refused_with: Option<String>,
}

/// What a row's resource-policy document writes where the CALLER's own principal ARN goes:
/// the principal is the row's own (an index-named user), so the document names it only once
/// that identity exists.
const CALLER_PLACEHOLDER: &str = "<the caller>";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a deny of every principal, against a user its identity policy allows",
            caller: Caller::User,
            identity: Some(document(json!([statement(
                "Allow",
                INVOKE_ACTION,
                &function_arn(FUNCTION)
            )]))),
            policy: function_policy("Deny", json!("*")),
            refused_with: Some(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "a deny of every principal, against the account's own credential",
            caller: Caller::Account,
            identity: None,
            policy: function_policy("Deny", json!("*")),
            refused_with: Some(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "a grant naming another principal of the account",
            caller: Caller::User,
            identity: None,
            policy: function_policy("Allow", json!({ "AWS": user_arn("someone-else") })),
            refused_with: Some(no_identity_grant()),
        },
        Row {
            case: "a grant naming only the account root, which delegates rather than grants",
            caller: Caller::User,
            identity: None,
            policy: function_policy(
                "Allow",
                json!({ "AWS": format!("arn:aws:iam::{ACCOUNT}:root") }),
            ),
            refused_with: Some(no_identity_grant()),
        },
        Row {
            case: "a grant naming the calling user, which permits it on its own",
            caller: Caller::User,
            identity: None,
            policy: function_policy("Allow", json!({ "AWS": CALLER_PLACEHOLDER })),
            refused_with: None,
        },
    ]
}

#[tokio::test]
async fn the_functions_own_policy_denies_and_grants_an_invocation_on_its_own() {
    let router = router_with_iam();
    given_active_function(&router).await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let name = format!("invoker-{index}");
        let (key, blamed) = match row.caller {
            Caller::User => (
                Some(principal(&router, case, &name, row.identity.clone(), None).await),
                user_arn(&name),
            ),
            Caller::Account => (
                None,
                ::service_common::caller_principal::ambient_user_arn(),
            ),
        };
        // The identity exists before the function's policy is written, so a statement naming
        // the CALLER names the principal this row really invokes as.
        let policy = row.policy.replace(CALLER_PLACEHOLDER, &blamed);
        set_function_policy(&router, case, Some(&policy)).await;

        let answer = invoke_as(&router, key.as_deref(), FUNCTION, &[]).await;

        match &row.refused_with {
            Some(reason) => assert_denied(
                case,
                &answer,
                &refusal(&blamed, &function_arn(FUNCTION), reason),
            ),
            None => assert_invoked(case, &answer),
        }
    }
}

/// RULE: a grant the function's policy made stops permitting the invocation the moment
/// `RemovePermission` takes the statement out — the decision is made on the documents as they
/// stand at the instant of the call, not on the ones in force when the grant was written.
///
/// The grant is written through `AddPermission` (the statement form a client uses) and the
/// account's own credential invokes throughout, so the refusal at the end is the withdrawal's
/// doing and not a broken invoke path.
#[tokio::test]
async fn a_withdrawn_grant_stops_permitting_the_invocation() {
    let case = "the grant removed";
    let router = router_with_iam();
    given_active_function(&router).await;

    let name = "late-invoker";
    let key = principal(&router, case, name, None, None).await;
    let refused = refusal(
        &user_arn(name),
        &function_arn(FUNCTION),
        &no_identity_grant(),
    );
    let statement_id = "grant-the-user";

    let added = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/policy"),
        &json!({
            "StatementId": statement_id,
            "Action": INVOKE_ACTION,
            "Principal": user_arn(name),
        }),
    )
    .await;
    assert_eq!(
        added.status,
        201,
        "case {case:?}: the fixture AddPermission must succeed, got {} {}",
        added.status,
        added.text()
    );
    assert_invoked(case, &invoke_as(&router, Some(&key), FUNCTION, &[]).await);

    let removed = call(
        &router,
        "DELETE",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/policy/{statement_id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        removed.status,
        204,
        "case {case:?}: RemovePermission must succeed, got {} {}",
        removed.status,
        removed.text()
    );

    assert_denied(
        case,
        &invoke_as(&router, Some(&key), FUNCTION, &[]).await,
        &refused,
    );
    // The account's own credential still invokes the same function, so the refusal above is
    // the withdrawn grant and not the function.
    assert_invoked(case, &invoke_as(&router, None, FUNCTION, &[]).await);
}
