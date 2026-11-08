// SPDX-License-Identifier: CC0-1.0

//! The EXECUTION ROLE a budget action is written with is resolved against the account's
//! IAM records before the action is stored.
//!
//! The RULE asserted here: a budget action is executed and reverted AS the role it names —
//! "the role passed for action execution and reversion" (`Action$ExecutionRoleArn`) — so
//! every write that establishes that member resolves the role first, and a role IAM
//! answers for as one this account does not hold, or whose trust policy does not let this
//! service's principal assume it, is refused with the modelled `AccessDeniedException`
//! ("You are not authorized to use this operation with the given parameters.")
//! with NOTHING written. The refusal names the role the request asked for and the grant it
//! is missing.
//!
//! A value IAM was not asked about grounds no refusal: a role of ANOTHER account is
//! refused earlier, by the cross-member rule that "roles and actions must be in the same
//! account" (`tests/all/budget_action_execution_role.rs`), and this suite pins that
//! precedence rather than restating that rule.
//!
//! The rows span what the rule generalises over: WHAT the value names (a role of this
//! account trusted by this service, the same role under a path, a role trusted among
//! several principals, a role trusted through a wildcard action, a role that exists and
//! trusts another service, a role whose trust policy denies this service explicitly, and a
//! well-formed ARN naming a role the account does not hold) and WHICH write establishes
//! the member (`CreateBudgetAction` and `UpdateBudgetAction`).
//!
//! IAM is mounted beside budgets on ONE engine — the shape `core/router/src/lib.rs` mounts
//! services in — and every role is created through IAM's own operations, so a role exists
//! only because IAM answered for it and the resolution under test runs over the same
//! in-process callback as in production.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use super::budget_action_execution::{email_subscriber, iam_definition};
use super::budget_lifecycle::{caller_account, call, is_error, same};
use super::client_error_envelopes::message;

/// The status `AccessDeniedException` is answered with: HTTP 400, the status this service
/// answers every refusal of a request at whatever 4xx the error shape's `@httpError`
/// declares (`client_error_envelopes.rs`).
const REFUSAL_STATUS: u16 = 400;

/// The Region the IAM hop is addressed in; budgets is global and this harness signs its
/// requests in `us-east-1`.
const REGION: &str = "us-east-1";

/// The principal a role this service acts as has to trust.
const BUDGETS_PRINCIPAL: &str = "budgets.amazonaws.com";

/// A service principal that is NOT this service, for the role that exists and trusts
/// someone else.
const OTHER_PRINCIPAL: &str = "lambda.amazonaws.com";

/// budgets and IAM on ONE engine, plus an IAM SDK client over the router's own in-process
/// callback: the very seam this service's role resolution reaches IAM through.
fn emulator() -> (MultiServiceRouter, ::aws_sdk_iam::Client) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    let callback = EmbeddedEmulator::from_router(router.clone());
    let iam = ::aws_sdk_iam::Client::from_conf(
        ::aws_sdk_iam::Config::builder()
            .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
            .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
            )
            .build(),
    );
    (router, iam)
}

/// A trust policy carrying `statements`.
fn trust(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One trust statement: `effect` the `action` for the service principals `principals`.
fn statement(effect: &str, principals: Value, action: &str) -> Value {
    json!({
        "Effect": effect,
        "Principal": {"Service": principals},
        "Action": action,
    })
}

/// Create `role_name` under `path` with `trust_document` through IAM itself, and answer
/// the ARN IAM minted for it.
async fn create_role(
    iam: &::aws_sdk_iam::Client,
    path: &str,
    role_name: &str,
    trust_document: &str,
) -> String {
    iam.create_role()
        .role_name(role_name)
        .path(path)
        .assume_role_policy_document(trust_document)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: IAM must create the role {role_name}: {error:?}"))
        .role()
        .map(|role| role.arn().to_string())
        .unwrap_or_else(|| panic!("setup: IAM must report an ARN for {role_name}"))
}

/// Create the budget the actions below hang off.
async fn seed_budget(router: &MultiServiceRouter, account_id: &str, budget_name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "50", "Unit": "USD"}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({budget_name}) to answer 200, got {status} {body}"
    );
}

/// A `CreateBudgetAction` body running as `execution_role_arn`.
fn create_action_body(account_id: &str, budget_name: &str, execution_role_arn: &str) -> Value {
    json!({
        "AccountId": account_id,
        "BudgetName": budget_name,
        "NotificationType": "ACTUAL",
        "ActionType": "APPLY_IAM_POLICY",
        "ActionThreshold": {"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"},
        "Definition": iam_definition(),
        "ExecutionRoleArn": execution_role_arn,
        "ApprovalModel": "MANUAL",
        "Subscribers": [email_subscriber(0)]
    })
}

/// The actions `DescribeBudgetActionsForBudget` reports for `budget_name`.
async fn listed_actions(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeBudgetActionsForBudget",
        &json!({"AccountId": account_id, "BudgetName": budget_name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the action listing of {budget_name:?} to answer 200, got {status} {body}"
    );
    body["Actions"].as_array().cloned().unwrap_or_default()
}

/// The sentence a refused write carries: the role the request named and the grant it is
/// missing. The ARN is the request's own value, so only the fixed text is written here.
fn expected_message(execution_role_arn: &str) -> String {
    format!(
        "Budgets permission required to assume [ExecutionRole: {execution_role_arn}]. \
         Please follow the instruction to grant assumeRole access to [Service Principal: \
         {BUDGETS_PRINCIPAL}]."
    )
}

/// What a row expects of the write that names its role.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// The role is one this service can assume: the write is served.
    Served,
    /// IAM answered that the role is not assumable by this service: the modelled
    /// `AccessDeniedException`.
    Unassumable,
    /// The role is not this account's at all, which the cross-member account rule refuses
    /// before IAM is asked.
    ForeignAccount,
}

/// One execution role the rule is asserted over.
struct Row {
    case: &'static str,
    role_arn: String,
    verdict: Verdict,
}

/// The role table: every row is a well-formed `RoleArn` (the modelled `@pattern` admits
/// it), differing in what IAM holds under it.
async fn rows(iam: &::aws_sdk_iam::Client, account_id: &str) -> Vec<Row> {
    let assumable = create_role(
        iam,
        "/",
        "budget-action-role",
        &trust(json!([statement(
            "Allow",
            json!(BUDGETS_PRINCIPAL),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let under_path = create_role(
        iam,
        "/service-role/",
        "budget-action-path-role",
        &trust(json!(statement(
            "Allow",
            json!(BUDGETS_PRINCIPAL),
            "sts:AssumeRole"
        ))),
    )
    .await;
    let among_principals = create_role(
        iam,
        "/",
        "budget-action-shared-role",
        &trust(json!([statement(
            "Allow",
            json!([OTHER_PRINCIPAL, BUDGETS_PRINCIPAL]),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let wildcard_action = create_role(
        iam,
        "/",
        "budget-action-wildcard-role",
        &trust(json!([statement(
            "Allow",
            json!(BUDGETS_PRINCIPAL),
            "sts:*"
        )])),
    )
    .await;
    let other_service = create_role(
        iam,
        "/",
        "budget-action-other-role",
        &trust(json!([statement(
            "Allow",
            json!(OTHER_PRINCIPAL),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let denied = create_role(
        iam,
        "/",
        "budget-action-denied-role",
        &trust(json!([
            statement("Allow", json!(BUDGETS_PRINCIPAL), "sts:AssumeRole"),
            statement("Deny", json!(BUDGETS_PRINCIPAL), "sts:AssumeRole"),
        ])),
    )
    .await;

    vec![
        Row {
            case: "a role of this account that trusts this service",
            role_arn: assumable,
            verdict: Verdict::Served,
        },
        Row {
            case: "the same grant on a role under a path",
            role_arn: under_path,
            verdict: Verdict::Served,
        },
        Row {
            case: "a role that trusts this service among several principals",
            role_arn: among_principals,
            verdict: Verdict::Served,
        },
        Row {
            case: "a role that trusts this service through an action wildcard",
            role_arn: wildcard_action,
            verdict: Verdict::Served,
        },
        Row {
            case: "a role that exists and trusts another service",
            role_arn: other_service,
            verdict: Verdict::Unassumable,
        },
        Row {
            case: "a role whose trust policy denies this service explicitly",
            role_arn: denied,
            verdict: Verdict::Unassumable,
        },
        Row {
            case: "a well-formed ARN naming a role this account does not hold",
            role_arn: format!("arn:aws:iam::{account_id}:role/never-created"),
            verdict: Verdict::Unassumable,
        },
        Row {
            case: "a role of another account",
            role_arn: "arn:aws:iam::210987654321:role/elsewhere".to_string(),
            verdict: Verdict::ForeignAccount,
        },
    ]
}

/// RULE: `CreateBudgetAction` resolves the execution role before it persists anything — a
/// role this service cannot assume is the modelled `AccessDeniedException` with the
/// documented sentence, and
/// the budget still holds no action.
#[tokio::test]
async fn a_create_resolves_its_execution_role_before_it_persists_the_action() {
    let (router, iam) = emulator();
    let account_id = caller_account();
    let table = rows(&iam, &account_id).await;

    for (position, row) in table.iter().enumerate() {
        let case = row.case;
        let budget_name = format!("role-create-{position}");
        seed_budget(&router, &account_id, &budget_name).await;
        let request = create_action_body(&account_id, &budget_name, &row.role_arn);
        let (status, body) = call(&router, "CreateBudgetAction", &request).await;

        match row.verdict {
            Verdict::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be served, got {status} {body}"
                );
                let listed = listed_actions(&router, &account_id, &budget_name).await;
                assert_eq!(
                    listed.len(),
                    1,
                    "case {case:?}: expected the created action to be held, got {listed:?}"
                );
                assert!(
                    same(&listed[0]["ExecutionRoleArn"], &json!(&row.role_arn)),
                    "case {case:?}: expected the action to carry the role it was created \
                     with, got {}",
                    listed[0]["ExecutionRoleArn"]
                );
            }
            Verdict::Unassumable => {
                assert_eq!(
                    status, REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {REFUSAL_STATUS}, got {status} {body}"
                );
                assert!(
                    is_error(&body, "AccessDeniedException"),
                    "case {case:?}: expected AccessDeniedException, got {body}"
                );
                let text = message(&body);
                let wanted = expected_message(&row.role_arn);
                assert_eq!(
                    text, wanted,
                    "case {case:?}: expected the message {wanted:?}, got {text:?}"
                );
                let listed = listed_actions(&router, &account_id, &budget_name).await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: expected a refused create to store no action, got {listed:?}"
                );
            }
            Verdict::ForeignAccount => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account rule's refusal, got {status} {body}"
                );
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {body}"
                );
                let listed = listed_actions(&router, &account_id, &budget_name).await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: expected a refused create to store no action, got {listed:?}"
                );
            }
        }
    }
}

/// RULE, on the other write that establishes the member: `UpdateBudgetAction` holds a
/// REPLACEMENT execution role to the same resolution, and a refused update leaves the
/// stored action carrying the role it already had.
#[tokio::test]
async fn an_update_holds_a_replacement_execution_role_to_the_same_resolution() {
    let (router, iam) = emulator();
    let account_id = caller_account();
    let table = rows(&iam, &account_id).await;
    let original = table
        .iter()
        .find(|row| row.verdict == Verdict::Served)
        .map(|row| row.role_arn.clone())
        .expect("the table holds an assumable role");

    for (position, row) in table.iter().enumerate() {
        let case = row.case;
        let budget_name = format!("role-update-{position}");
        seed_budget(&router, &account_id, &budget_name).await;
        let (status, created) = call(
            &router,
            "CreateBudgetAction",
            &create_action_body(&account_id, &budget_name, &original),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup expected the action to be created, got {status} {created}"
        );
        let action_id = created["ActionId"].as_str().unwrap_or_default().to_string();

        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "ActionId": &action_id,
                "ExecutionRoleArn": &row.role_arn
            }),
        )
        .await;

        let expected_role = match row.verdict {
            Verdict::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update to be served, got {status} {body}"
                );
                &row.role_arn
            }
            Verdict::Unassumable => {
                assert_eq!(
                    status, REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {REFUSAL_STATUS}, got {status} {body}"
                );
                assert!(
                    is_error(&body, "AccessDeniedException"),
                    "case {case:?}: expected AccessDeniedException, got {body}"
                );
                let text = message(&body);
                let wanted = expected_message(&row.role_arn);
                assert_eq!(
                    text, wanted,
                    "case {case:?}: expected the message {wanted:?}, got {text:?}"
                );
                &original
            }
            Verdict::ForeignAccount => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account rule's refusal, got {status} {body}"
                );
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {body}"
                );
                &original
            }
        };

        let listed = listed_actions(&router, &account_id, &budget_name).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected the budget to still hold its action, got {listed:?}"
        );
        assert!(
            same(&listed[0]["ExecutionRoleArn"], &json!(expected_role)),
            "case {case:?}: expected the stored role to be {expected_role}, got {}",
            listed[0]["ExecutionRoleArn"]
        );
    }
}
