// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite pinning `DescribeBudgetAction` as the REPORTING operation the model
//! documents it to be ("Describes a budget action detail"), driven over the wire through
//! the same `MultiServiceRouter` the production router serves from (the harness is the
//! lifecycle suite's, so every suite in this target drives the service identically).
//!
//! The RULE every table below asserts: a reporting operation answers from stored state,
//! so repeated calls answer identically and nothing a later caller sees changes — with
//! the single exception of a transition the CLOCK had already made due, which is
//! materialised on the way out because it was owed at its own instant whether or not
//! anybody called.
//!
//! Each table spans that class rather than one request:
//!
//! 1. Over the WHOLE modelled `ActionStatus` catalog, a read reports the stored status,
//!    echoes the account and budget it was asked about, answers byte-identically when
//!    repeated, and leaves the stored action, the settle it still owes and its history
//!    exactly as they were.
//! 2. Over every `*_IN_PROGRESS` status (and the settled status its own execution names),
//!    a read whose settle has fallen due reports the settled status, discharges the
//!    settle, appends exactly the ONE record that status change is — the event the
//!    service generated rather than a caller — and changes NO other member of the action.
//! 3. Over the ways a read can fail to locate an action, the refusal changes nothing: a
//!    later read of the action that does exist answers exactly as it did before.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The enum catalog is not re-typed here: this is the model-derived constant the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from it
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    ACTION_STATUS_VALUES, EXECUTION_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    action_execution, action_is_in_progress, ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    PendingSettle, EVENT_TYPE_SYSTEM,
};

use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};

/// An `ExecutionRoleArn` satisfying `RoleArn`'s modelled `@pattern` and `@length(32, 618)`
/// (`smithy_query(query="shape",shape="RoleArn")`), in the account the action lives in:
/// "roles and actions must be in the same account" (`Action$ExecutionRoleArn`).
fn execution_role(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/budget-action-reader")
}

/// An `ActionId` of exactly the modelled shape
/// (`smithy_query(query="shape",shape="ActionId")`:
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`) that
/// no action in this suite carries, so a request naming it is refused for what it names
/// rather than for how it is spelled.
const UNHELD_ACTION_ID: &str = "0ffd1ce5-0000-4000-8000-0123456789ab";

/// A budget satisfying every modelled constraint and documented cross-member rule, so an
/// action row varies only what it means to.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// A `CreateBudgetAction` body: an `APPLY_IAM_POLICY` action, whose `Definition` member
/// is the one that action type "determines the format for" (`Action$ActionType`).
fn create_body(account_id: &str, budget_name: &str) -> Value {
    json!({
        "AccountId": account_id,
        "BudgetName": budget_name,
        "NotificationType": "ACTUAL",
        "ActionType": "APPLY_IAM_POLICY",
        "ActionThreshold": {"ActionThresholdValue": 75.0, "ActionThresholdType": "PERCENTAGE"},
        "Definition": {
            "IamActionDefinition": {
                "PolicyArn": "arn:aws:iam::123456789012:policy/budget-restrict",
                "Roles": ["BudgetRestrictedRole"]
            }
        },
        "ExecutionRoleArn": execution_role(account_id),
        "ApprovalModel": "MANUAL",
        "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "budget-ops@example.com"}]
    })
}

/// Create `name` and one action on it, and answer the minted `ActionId`, failing the
/// test if either setup call did not take.
async fn seed_budget_with_action(
    router: &MultiServiceRouter,
    account_id: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": account_id, "Budget": budget(name)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({name}) to answer 200, got {status} {body}"
    );
    let (status, body) = call(router, "CreateBudgetAction", &create_body(account_id, name)).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction({name}) to answer 200, got {status} {body}"
    );
    body["ActionId"]
        .as_str()
        .expect("CreateBudgetAction mints an ActionId")
        .to_string()
}

/// One `DescribeBudgetAction` call.
async fn describe_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "ActionId": action_id
        }),
    )
    .await
}

/// The history a LATER caller sees for this action, read over the wire.
async fn history_records(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeBudgetActionHistories",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "ActionId": action_id
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeBudgetActionHistories to answer 200, got {status} {body}"
    );
    body["ActionHistories"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// How many records that history holds.
async fn history_len(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> usize {
    history_records(router, account_id, budget_name, action_id)
        .await
        .len()
}

/// Everything about the stored action that a read must not change: its status, the
/// settle it still owes, and how many history records it holds.
#[derive(Debug, PartialEq, Eq)]
struct Stored {
    status: String,
    pending: Option<(SystemTime, String)>,
    history: usize,
    actions_on_budget: usize,
}

/// Read the stored action straight out of the engine, so a test compares what the NEXT
/// caller would be served rather than only what this one was.
fn stored(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> Stored {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    let held = store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .expect("the seeded budget");
    let action = held
        .actions
        .iter()
        .find(|candidate| candidate.action.action_id == action_id)
        .expect("the seeded action");
    Stored {
        status: action.action.status.clone(),
        pending: action
            .pending
            .as_ref()
            .map(|pending| (pending.settles_at, pending.settles_to.clone())),
        history: action.history.len(),
        actions_on_budget: held.actions.len(),
    }
}

/// Put the stored action into `status`, with `pending` the settle the clock owes from it.
///
/// Seeding is how the whole `ActionStatus` catalog is reached: nothing in this emulator
/// fails an action's work, so no request here mints a `*_FAILURE` status, and a row for
/// one would otherwise be unreachable.
fn seed_status(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    status: &str,
    pending: Option<PendingSettle>,
) {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let mut store = state.lock();
    let held = store
        .budgets
        .get_mut(&(account_id.to_string(), budget_name.to_string()))
        .expect("the seeded budget");
    let action = held
        .actions
        .iter_mut()
        .find(|candidate| candidate.action.action_id == action_id)
        .expect("the seeded action");
    action.action.status = status.to_string();
    action.pending = pending;
}

/// The settled status the clock owes an action that carries `in_progress`: the execution
/// whose `*_IN_PROGRESS` status this is names the status it settles to.
///
/// Searched over the whole modelled `ExecutionType` / `ActionStatus` catalogs, so an
/// execution type the model gains cannot slip past this helper. The transitions
/// THEMSELVES are pinned independently of the service's own table by the
/// `budget_action_execution` suite, which writes them out; here the settled status is
/// only the expected value of a read, so it is taken from the state machine.
fn settles_to(in_progress: &str) -> String {
    for execution_type in EXECUTION_TYPE_VALUES {
        for from in ACTION_STATUS_VALUES {
            if let Some(execution) = action_execution(execution_type, from) {
                if execution.in_progress == in_progress {
                    return execution.settles_to.to_string();
                }
            }
        }
    }
    panic!("no execution leaves an action in {in_progress}");
}

/// A budget name for `status`'s row: lower-cased, with the underscores the modelled
/// `BudgetName` pattern has no quarrel with turned into hyphens for readability.
fn row_budget_name(prefix: &str, status: &str) -> String {
    format!("{prefix}-{}", status.to_ascii_lowercase().replace('_', "-"))
}

/// The two bodies with `Action$Status` forced equal — so a comparison asserts "nothing
/// BUT the status differs" without hand-listing every other member.
fn same_but_status(left: &Value, right: &Value) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left["Action"]["Status"] = Value::Null;
    right["Action"]["Status"] = Value::Null;
    same(&left, &right)
}

/// RULE, part 1: over the whole modelled `ActionStatus` catalog, a read reports the
/// stored action, is repeatable, and changes nothing a later caller sees — including the
/// settle an in-flight action still owes, which a read may not bring forward.
#[tokio::test]
async fn a_read_reports_the_stored_action_and_changes_nothing() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for status in ACTION_STATUS_VALUES {
        let budget_name = row_budget_name("read-unchanged", status);
        let action_id = seed_budget_with_action(&router, &account_id, &budget_name).await;

        // An `*_IN_PROGRESS` status is the one the service holds WITH a settle owed; the
        // settle is seeded far enough ahead that no read in this row can reach it.
        let owed = action_is_in_progress(status).then(|| PendingSettle {
            settles_at: SystemTime::now() + Duration::from_secs(3_600),
            settles_to: settles_to(status),
        });
        seed_status(
            &engine,
            &account_id,
            &budget_name,
            &action_id,
            status,
            owed,
        );

        let before = stored(&engine, &account_id, &budget_name, &action_id);
        let history_before = history_len(&router, &account_id, &budget_name, &action_id).await;

        let (http, first) = describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            http, 200,
            "case {status:?}: expected DescribeBudgetAction to answer 200, got {http} {first}"
        );
        assert_eq!(
            first["Action"]["Status"].as_str(),
            Some(*status),
            "case {status:?}: expected the stored status to be reported, got {:?}",
            first["Action"]["Status"]
        );
        assert_eq!(
            first["AccountId"].as_str(),
            Some(account_id.as_str()),
            "case {status:?}: expected the account asked about to be echoed, got {:?}",
            first["AccountId"]
        );
        assert_eq!(
            first["BudgetName"].as_str(),
            Some(budget_name.as_str()),
            "case {status:?}: expected the budget asked about to be echoed, got {:?}",
            first["BudgetName"]
        );
        assert_eq!(
            first["Action"]["ActionId"].as_str(),
            Some(action_id.as_str()),
            "case {status:?}: expected the action asked about to be reported, got {:?}",
            first["Action"]["ActionId"]
        );

        let (http, second) = describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            http, 200,
            "case {status:?}: expected a repeated DescribeBudgetAction to answer 200, \
             got {http} {second}"
        );
        assert!(
            same(&first, &second),
            "case {status:?}: expected a repeated read to answer identically, got {first} \
             then {second}"
        );

        let after = stored(&engine, &account_id, &budget_name, &action_id);
        assert_eq!(
            after, before,
            "case {status:?}: expected the stored action to be untouched by reads, it was \
             {before:?} and is now {after:?}"
        );
        let history_after = history_len(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            history_after, history_before,
            "case {status:?}: expected reads to append no history record, the history held \
             {history_before} and now holds {history_after}"
        );
    }
}

/// RULE, part 2: the ONE change a read makes is the settle the clock had already made
/// due — asserted over every `*_IN_PROGRESS` status, with the settled status that
/// status's own execution names.
#[tokio::test]
async fn a_read_materialises_only_the_settle_the_clock_already_owed() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    let in_progress: Vec<&&str> = ACTION_STATUS_VALUES
        .iter()
        .filter(|status| action_is_in_progress(status))
        .collect();
    assert!(
        !in_progress.is_empty(),
        "expected the modelled ActionStatus catalog to carry in-progress statuses, got \
         {ACTION_STATUS_VALUES:?}"
    );

    for status in in_progress {
        let settled = settles_to(status);
        let budget_name = row_budget_name("read-settles", status);
        let action_id = seed_budget_with_action(&router, &account_id, &budget_name).await;

        // The settle still ahead: the read reports the in-flight status and leaves the
        // settle owed, unmoved.
        let settles_at = SystemTime::now() + Duration::from_secs(3_600);
        seed_status(
            &engine,
            &account_id,
            &budget_name,
            &action_id,
            status,
            Some(PendingSettle {
                settles_at,
                settles_to: settled.clone(),
            }),
        );
        let (http, in_flight) = describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            http, 200,
            "case {status:?}: expected DescribeBudgetAction to answer 200, got {http} {in_flight}"
        );
        assert_eq!(
            in_flight["Action"]["Status"].as_str(),
            Some(*status),
            "case {status:?}: expected the in-flight status while the settle is not due, got {:?}",
            in_flight["Action"]["Status"]
        );
        let held = stored(&engine, &account_id, &budget_name, &action_id);
        assert_eq!(
            held.pending,
            Some((settles_at, settled.clone())),
            "case {status:?}: expected the owed settle to be left where it was, got {:?}",
            held.pending
        );
        let history_before = history_len(&router, &account_id, &budget_name, &action_id).await;

        // The same settle, now due: the clock owed it at its instant, so the read reports
        // the settled status and discharges it.
        seed_status(
            &engine,
            &account_id,
            &budget_name,
            &action_id,
            status,
            Some(PendingSettle {
                settles_at: SystemTime::now() - Duration::from_secs(1),
                settles_to: settled.clone(),
            }),
        );
        let (http, settled_body) =
            describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            http, 200,
            "case {status:?}: expected DescribeBudgetAction to answer 200, got {http} \
             {settled_body}"
        );
        assert_eq!(
            settled_body["Action"]["Status"].as_str(),
            Some(settled.as_str()),
            "case {status:?}: expected the settled status {settled} once the settle fell due, \
             got {:?}",
            settled_body["Action"]["Status"]
        );
        assert!(
            same_but_status(&in_flight, &settled_body),
            "case {status:?}: expected the settle to change the status and nothing else, got \
             {in_flight} then {settled_body}"
        );

        let held = stored(&engine, &account_id, &budget_name, &action_id);
        assert_eq!(
            held.pending, None,
            "case {status:?}: expected the discharged settle to be owed no longer, got {:?}",
            held.pending
        );
        assert_eq!(
            held.status, settled,
            "case {status:?}: expected the stored status to be {settled}, got {:?}",
            held.status
        );
        // The settle is itself a status change of the action, and an action's history is
        // the log of its status changes — so it leaves exactly ONE record: the one no
        // caller triggered, carrying the status the action now holds.
        let records = history_records(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            records.len(),
            history_before + 1,
            "case {status:?}: expected the settle to append exactly its own record, the \
             history held {history_before} and now holds {}",
            records.len()
        );
        let appended = records.last().expect("the history is not empty");
        assert_eq!(
            appended["EventType"].as_str(),
            Some(EVENT_TYPE_SYSTEM),
            "case {status:?}: expected the settle to be recorded as the service's own event, \
             got {appended}"
        );
        assert_eq!(
            appended["Status"].as_str(),
            Some(settled.as_str()),
            "case {status:?}: expected the appended record to carry the settled status \
             {settled}, got {appended}"
        );

        // And it is a settle, not a step: reading again reports the same settled status.
        let (http, again) = describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            http, 200,
            "case {status:?}: expected a repeated read to answer 200, got {http} {again}"
        );
        assert!(
            same(&settled_body, &again),
            "case {status:?}: expected a settled action to read back identically, got \
             {settled_body} then {again}"
        );
    }
}

/// One row of the refusal table: how the request fails to name an action this account
/// holds on this budget.
struct Miss {
    case: &'static str,
    /// Offsets applied to the request that WOULD have located the action. The account is
    /// not one of them: every request names the one account this service serves, and one
    /// that names another is refused before any lookup (`account_scoping.rs`).
    budget_name: Option<String>,
    action_id: Option<&'static str>,
}

/// RULE, part 3: a read that locates nothing is refused `NotFoundException` and changes
/// nothing — the action that does exist reads back exactly as it did before.
///
/// The STATUS a refusal carries is this service's envelope rule, not this suite's
/// subject: every refusal of the request is answered at HTTP 400 whatever 4xx its error
/// shape declares, which the `client_error_envelopes` suite pins over the whole set of
/// error shapes. The rows below assert the error code and, above all, that a refused
/// read left the store alone.
#[tokio::test]
async fn a_read_that_locates_nothing_changes_nothing() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "read-misses";
    let action_id = seed_budget_with_action(&router, &account_id, budget_name).await;
    // A second budget of the same account, so a row can name a budget that EXISTS and
    // simply does not own this action.
    let other_budget = "read-misses-sibling";
    seed_budget_with_action(&router, &account_id, other_budget).await;

    let (http, baseline) = describe_action(&router, &account_id, budget_name, &action_id).await;
    assert_eq!(
        http, 200,
        "setup: expected DescribeBudgetAction to answer 200, got {http} {baseline}"
    );
    let before = stored(&engine, &account_id, budget_name, &action_id);

    let misses = vec![
        Miss {
            case: "an action id no action carries",
            budget_name: None,
            action_id: Some(UNHELD_ACTION_ID),
        },
        Miss {
            case: "a budget of this account that does not own the action",
            budget_name: Some(other_budget.to_string()),
            action_id: None,
        },
        Miss {
            case: "a budget the account holds nothing under",
            budget_name: Some("read-misses-absent".to_string()),
            action_id: None,
        },
        Miss {
            case: "the owning budget's name in another case",
            budget_name: Some(budget_name.to_uppercase()),
            action_id: None,
        },
    ];

    for miss in misses {
        let case = miss.case;
        let (http, body) = describe_action(
            &router,
            &account_id,
            miss.budget_name.as_deref().unwrap_or(budget_name),
            miss.action_id.unwrap_or(&action_id),
        )
        .await;
        assert_eq!(
            http, 400,
            "case {case:?}: expected DescribeBudgetAction to be refused with 400, got {http} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected a NotFoundException, got {body}"
        );

        let after = stored(&engine, &account_id, budget_name, &action_id);
        assert_eq!(
            after, before,
            "case {case:?}: expected a refused read to change nothing, the stored action was \
             {before:?} and is now {after:?}"
        );
        let (http, again) = describe_action(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected the action that exists to still read back, got {http} {again}"
        );
        assert!(
            same(&baseline, &again),
            "case {case:?}: expected the action that exists to read back unchanged, got \
             {baseline} then {again}"
        );
    }
}
