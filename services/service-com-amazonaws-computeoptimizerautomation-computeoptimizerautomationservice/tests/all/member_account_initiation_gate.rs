// SPDX-License-Identifier: CC0-1.0

//! The ASSOCIATION gate on INITIATING automation work in another account, end
//! to end.
//!
//! The rule under test: the two operations that initiate automation work in an
//! account — the on-demand automation of a recommended action, and the rollback
//! of an automation event — act in an account OTHER than the caller's only
//! while that account is currently associated; the caller's own account needs
//! no association. The gate is judged per call, so an account whose association
//! has been removed is refused even though the resource the request names is
//! still stored, and a re-associated account is served again.
//!
//! Every call below goes over the wire: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts, from the UNENROLLED
//! harness, so nothing in this suite reaches into the store — the association
//! state each row is judged on is the one its own `AssociateAccounts` /
//! `DisassociateAccounts` calls put there.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, caller_account_id, recommended_action_inventory,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, member, string_member, unenrolled_emulator_with_engine,
};

/// Member account ids in the model's `AccountId` form (`^[0-9]{12}$`), one per
/// row so no row's association state can disturb another's.
const MEMBER_ASSOCIATED: &str = "111111111111";
const MEMBER_DISASSOCIATED: &str = "222222222222";
const MEMBER_REASSOCIATED: &str = "333333333333";
/// A member account no request of a row ever associates: this service holds no
/// enrollment, and therefore no analysed inventory, for it.
const MEMBER_NEVER_ASSOCIATED: &str = "999988887777";

/// A router with this service mounted and the calling account opted in the way a
/// client opts in — `UpdateEnrollmentConfiguration` over the wire, which is also
/// what records the caller's own analysed inventory.
async fn opted_in_router(case: &str) -> MultiServiceRouter {
    let router = unenrolled_emulator_with_engine().0;
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        case,
    )
    .await;
    router
}

/// Associate `account_id` over the wire.
async fn associate(router: &MultiServiceRouter, account_id: &str, case: &str) {
    call_ok(
        router,
        "AssociateAccounts",
        &json!({"accountIds": [account_id]}),
        case,
    )
    .await;
}

/// Remove `account_id`'s association over the wire.
async fn disassociate(router: &MultiServiceRouter, account_id: &str, case: &str) {
    call_ok(
        router,
        "DisassociateAccounts",
        &json!({"accountIds": [account_id]}),
        case,
    )
    .await;
}

/// The `organizationRuleMode` `ListAccounts` reports for `account_id`, which is
/// how the association state a row set up is read back over the wire.
async fn reported_rule_mode(router: &MultiServiceRouter, account_id: &str, case: &str) -> String {
    let listed = call_ok(router, "ListAccounts", &json!({}), case).await;
    member(&listed, "accounts")
        .as_array()
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| string_member(account, "accountId") == account_id)
                .map(|account| string_member(account, "organizationRuleMode"))
        })
        .unwrap_or_default()
}

/// The first recommended action the analysis of `account_id`'s resources
/// produces. The inventory is a pure function of the account id, so a row can
/// name an action of an account WITHOUT listing it — which is what the row for
/// an account this service holds nothing for needs.
fn first_action_id(account_id: &str) -> String {
    recommended_action_inventory(account_id)
        .first()
        .map(|(action_id, _)| action_id.clone())
        .expect("the analysis describes at least one volume")
}

/// The `eventId`s `ListAutomationEvents` reports for one account.
async fn event_ids_in_account(
    router: &MultiServiceRouter,
    account_id: &str,
    case: &str,
) -> Vec<String> {
    let listed = call_ok(
        router,
        "ListAutomationEvents",
        &json!({"filters": [{"name": "AccountId", "values": [account_id]}]}),
        case,
    )
    .await;
    member(&listed, "automationEvents")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| string_member(event, "eventId"))
        .collect()
}

/// `GetAutomationEvent` for `event_id`, requiring success.
async fn get_event(router: &MultiServiceRouter, event_id: &str, case: &str) -> Value {
    call_ok(
        router,
        "GetAutomationEvent",
        &json!({"eventId": event_id}),
        case,
    )
    .await
}

/// Assert a call was refused `AccessDeniedException` (HTTP 403) carrying this
/// service's access-denied wording.
fn assert_access_denied((status, body): (u16, Value), case: &str) {
    assert_refused((status, body.clone()), 403, "AccessDeniedException", case);
    assert_eq!(
        string_member(&body, "message"),
        state::ACCESS_DENIED_MESSAGE,
        "case {case:?}: the refusal must carry this service's access-denied wording, got {body}"
    );
}

/// Whether a row's account is associated when the work is initiated, and what
/// the service must answer.
struct Case {
    case: &'static str,
    /// The account the recommended action, and so the automation, belongs to.
    account: String,
    /// Associate the account before naming its action.
    associate: bool,
    /// Then remove that association.
    disassociate: bool,
    /// Then put it back.
    reassociate: bool,
    /// The HTTP status the initiating call must answer with.
    expected_status: u16,
    /// The error code it must carry, or `None` when the call must succeed.
    expected_code: Option<&'static str>,
}

/// The table: the association states an account can be in when work is
/// initiated in it, spanning the caller's own account (never associated, and
/// never needing to be), a member account that is associated, one whose
/// association was removed, one that was re-associated, and one this service
/// holds nothing at all for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "caller-own-account",
            account: caller_account_id(),
            associate: false,
            disassociate: false,
            reassociate: false,
            expected_status: 200,
            expected_code: None,
        },
        Case {
            case: "member-associated",
            account: MEMBER_ASSOCIATED.to_string(),
            associate: true,
            disassociate: false,
            reassociate: false,
            expected_status: 200,
            expected_code: None,
        },
        Case {
            case: "member-disassociated",
            account: MEMBER_DISASSOCIATED.to_string(),
            associate: true,
            disassociate: true,
            reassociate: false,
            expected_status: 403,
            expected_code: Some("AccessDeniedException"),
        },
        Case {
            case: "member-reassociated",
            account: MEMBER_REASSOCIATED.to_string(),
            associate: true,
            disassociate: true,
            reassociate: true,
            expected_status: 200,
            expected_code: None,
        },
        Case {
            case: "member-never-associated",
            account: MEMBER_NEVER_ASSOCIATED.to_string(),
            // An account no association has reached has no analysed inventory
            // either, so its action id names no resource of this service at
            // all: existence is resolved before standing, and the answer is
            // the not-found one.
            associate: false,
            disassociate: false,
            reassociate: false,
            expected_status: 404,
            expected_code: Some("ResourceNotFoundException"),
        },
    ]
}

/// Put `row`'s account in the association state the row names, and read that
/// state back over the wire.
async fn arrange(router: &MultiServiceRouter, row: &Case) {
    if row.associate {
        associate(router, &row.account, row.case).await;
    }
    if row.disassociate {
        disassociate(router, &row.account, row.case).await;
    }
    if row.reassociate {
        associate(router, &row.account, row.case).await;
    }
    if row.associate {
        let expected = if row.disassociate && !row.reassociate {
            state::ORGANIZATION_RULE_MODE_NONE_ALLOWED
        } else {
            state::ORGANIZATION_RULE_MODE_ANY_ALLOWED
        };
        assert_eq!(
            reported_rule_mode(router, &row.account, row.case).await,
            expected,
            "case {:?}: the row's association state must be readable back before the \
             initiating call",
            row.case
        );
    }
}

/// An automation is started for a recommended action only while the account it
/// belongs to is one the caller may initiate work in: the caller's own account
/// and an associated member account are served, a member account whose
/// association has been removed is refused — even though its analysed action is
/// still stored — and a re-associated one is served again. A refused call mints
/// no event in that account.
#[tokio::test]
async fn an_automation_starts_only_in_an_account_the_caller_may_initiate_work_in() {
    let router = opted_in_router("opt-in").await;

    for row in cases() {
        let case = row.case;
        arrange(&router, &row).await;

        let action_id = first_action_id(&row.account);
        let (status, body) = call(
            &router,
            "StartAutomationEvent",
            &json!({"recommendedActionId": action_id}),
        )
        .await;

        match row.expected_code {
            None => {
                assert_eq!(
                    status, row.expected_status,
                    "case {case:?}: the account may be automated, got {status} and {body}"
                );
                let event_id = string_member(&body, "eventId");
                let event = get_event(&router, &event_id, case).await;
                assert_eq!(
                    string_member(&event, "accountId"),
                    row.account,
                    "case {case:?}: the automation runs in the account the action is about, \
                     got {event}"
                );
                assert_eq!(
                    event_ids_in_account(&router, &row.account, case).await,
                    vec![event_id],
                    "case {case:?}: the started automation is the one event of that account"
                );
            }
            Some("AccessDeniedException") => {
                assert_access_denied((status, body), case);
                assert_eq!(
                    event_ids_in_account(&router, &row.account, case).await,
                    Vec::<String>::new(),
                    "case {case:?}: a refused start mints no automation event"
                );
            }
            Some(expected_code) => {
                assert_refused((status, body), row.expected_status, expected_code, case);
                assert_eq!(
                    event_ids_in_account(&router, &row.account, case).await,
                    Vec::<String>::new(),
                    "case {case:?}: a refused start mints no automation event"
                );
            }
        }
    }
}

/// A rollback is initiated for a completed automation only while the account the
/// event belongs to is one the caller may initiate work in. Every row's
/// automation is started and completed while its account is associated, so the
/// event is identical across rows and the only thing that differs is the
/// association state at the moment of the rollback: the caller's own account and
/// an associated member account are rolled back, a member account whose
/// association has been removed is refused and its event is left exactly as it
/// was, and a re-associated one is rolled back again.
#[tokio::test]
async fn a_rollback_starts_only_in_an_account_the_caller_may_initiate_work_in() {
    let router = opted_in_router("opt-in").await;

    // Every row that can have an event: the account is associated first, so the
    // automation itself is initiated under the same rule.
    let rows: Vec<Case> = cases()
        .into_iter()
        .filter(|row| row.case != "member-never-associated")
        .collect();

    let mut started: Vec<(Case, String)> = Vec::new();
    for row in rows {
        if row.associate {
            associate(&router, &row.account, row.case).await;
        }
        let answer = call_ok(
            &router,
            "StartAutomationEvent",
            &json!({"recommendedActionId": first_action_id(&row.account)}),
            row.case,
        )
        .await;
        let event_id = string_member(&answer, "eventId");
        started.push((row, event_id));
    }

    // Only a completed automation has anything to roll back.
    tokio::time::sleep(state::EVENT_SETTLEMENT_DELAY + Duration::from_millis(250)).await;

    for (row, event_id) in &started {
        let case = row.case;
        let completed = get_event(&router, event_id, case).await;
        assert_eq!(
            string_member(&completed, "eventStatus"),
            "Complete",
            "case {case:?}: the automation has to be complete before it is rolled back, \
             got {completed}"
        );

        // Now move the account to the association state this row is about.
        if row.disassociate {
            disassociate(&router, &row.account, case).await;
        }
        if row.reassociate {
            associate(&router, &row.account, case).await;
        }

        let answer = call(
            &router,
            "RollbackAutomationEvent",
            &json!({"eventId": event_id, "clientToken": format!("rollback-{case}")}),
        )
        .await;

        if row.expected_code.is_none() {
            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: the account may be rolled back, got {status} and {body}"
            );
            assert_eq!(
                string_member(&body, "eventStatus"),
                "RollbackInProgress",
                "case {case:?}: the rollback it started is running, got {body}"
            );
            continue;
        }

        assert_access_denied(answer, case);
        let unchanged = get_event(&router, event_id, case).await;
        assert_eq!(
            string_member(&unchanged, "eventStatus"),
            "Complete",
            "case {case:?}: a refused rollback leaves the event exactly as it was, \
             got {unchanged}"
        );

        // The refused call consumed nothing: once the account is associated
        // again, the SAME client token rolls the same event back.
        associate(&router, &row.account, case).await;
        let retried = call_ok(
            &router,
            "RollbackAutomationEvent",
            &json!({"eventId": event_id, "clientToken": format!("rollback-{case}")}),
            case,
        )
        .await;
        assert_eq!(
            string_member(&retried, "eventStatus"),
            "RollbackInProgress",
            "case {case:?}: a re-associated account's event rolls back under the token the \
             refused call carried, got {retried}"
        );
    }
}
