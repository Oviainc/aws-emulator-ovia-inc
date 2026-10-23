// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the change-request lifecycle's two settlement rules.
//!
//! Rule 1 — a change request is settled at CREATION, with `isAutoApproved` true, status
//! COMMITTED, every approval APPROVED and its changes applied, exactly when no approval it
//! needs is outstanding: either the requesting account is the only party whose approval the
//! request needs, or the collaboration's `autoApprovedChangeTypes` covers every
//! `ChangeType` the request asks for. Any other request is stored PENDING with a PENDING
//! approval per party and applies nothing.
//!
//! Rule 2 — once a change request is COMMITTED or CANCELLED it can no longer be modified:
//! every `ChangeRequestAction` on it is refused with HTTP 400, `x-amzn-errortype`
//! `ValidationException` and `reason` `INVALID_CONFIGURATION`, and a message composed from
//! the request's change-request id and the status that request is in. PENDING and DENIED
//! are not covered by that rule.
//!
//! The tables vary what the two rules generalise over: the NUMBER of parties whose
//! approval a request needs (0, 1, 2), which kind of `ChangeSpecification` the change
//! carries, whether the collaboration's auto-approval setting covers the types asked for,
//! and — for rule 2 — every value of the `ChangeRequestAction` catalog against every
//! status the rule does and does not name. Requests go over the wire through the same
//! `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, error_message, Emulator,
};
use super::collaboration_change_request_decisions::update_change_request;
use super::collaboration_change_requests::{
    approvals_of, create_change_request, declared_member, get_change_request, member_change,
    seeded_collaboration, sorted, types_of, NEWCOMER_ACCOUNT, PARTNER_ACCOUNT,
};

/// Every `ChangeRequestAction` the model declares (shape `ChangeRequestAction`).
const ALL_ACTIONS: [&str; 4] = ["APPROVE", "DENY", "CANCEL", "COMMIT"];

/// A third `AccountId` (model shape `AccountId`, len 12..12, pattern `^\d+$`), for the row
/// whose collaboration has two parties besides the caller.
const THIRD_ACCOUNT: &str = "777788889999";

/// One row of the settlement table.
struct SettlementCase {
    case: &'static str,
    /// The parties declared BESIDES the caller, who is always the creator.
    members: Value,
    /// The collaboration's `autoApprovedChangeRequestTypes`, when the row sets one.
    auto_approved: Option<Vec<&'static str>>,
    change: Value,
    expect_types: Vec<&'static str>,
    /// Whether the rule settles this request at creation.
    expect_settled: bool,
}

/// Rule 1 over the class: the settlement decision follows from whether any approval is
/// outstanding, not from the shape of the change.
#[tokio::test]
async fn a_request_is_settled_at_creation_exactly_when_no_approval_is_outstanding() {
    let emulator = Emulator::new();
    let creator = account_id();

    let collaboration_edit = json!({
        "specificationType": "COLLABORATION",
        "specification": { "collaboration": { "autoApprovedChangeTypes": ["ADD_MEMBER"] } }
    });

    let cases = vec![
        SettlementCase {
            case: "the caller is the only party and adds a member",
            members: json!([]),
            auto_approved: None,
            change: member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["ADD_MEMBER"],
            expect_settled: true,
        },
        SettlementCase {
            case: "the caller is the only party and edits its own grant",
            members: json!([]),
            auto_approved: None,
            change: member_change(account_id(), &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]),
            expect_types: vec!["GRANT_RECEIVE_RESULTS_ABILITY"],
            expect_settled: true,
        },
        SettlementCase {
            case: "the caller is the only party and edits the collaboration setting",
            members: json!([]),
            auto_approved: None,
            change: collaboration_edit.clone(),
            expect_types: vec!["EDIT_AUTO_APPROVED_CHANGE_TYPES"],
            expect_settled: true,
        },
        SettlementCase {
            case: "one other party must approve the add",
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            auto_approved: None,
            change: member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["ADD_MEMBER"],
            expect_settled: false,
        },
        SettlementCase {
            case: "two other parties must approve the add",
            members: json!([
                declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"]),
                declared_member(THIRD_ACCOUNT, &["CAN_QUERY"]),
            ]),
            auto_approved: None,
            change: member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["ADD_MEMBER"],
            expect_settled: false,
        },
        SettlementCase {
            case: "one other party must approve the collaboration setting edit",
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            auto_approved: None,
            change: collaboration_edit.clone(),
            expect_types: vec!["EDIT_AUTO_APPROVED_CHANGE_TYPES"],
            expect_settled: false,
        },
        SettlementCase {
            case: "another party is a party, but the collaboration auto-approves the add",
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            auto_approved: Some(vec!["ADD_MEMBER"]),
            change: member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["ADD_MEMBER"],
            expect_settled: true,
        },
        SettlementCase {
            case: "another party is a party and the auto-approval covers only one of two types",
            members: json!([declared_member(
                PARTNER_ACCOUNT,
                &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]
            )]),
            auto_approved: Some(vec!["GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY"]),
            change: member_change(
                PARTNER_ACCOUNT,
                &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"],
            ),
            expect_types: vec![
                "GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
                "REVOKE_RECEIVE_RESULTS_ABILITY",
            ],
            expect_settled: false,
        },
        SettlementCase {
            case: "the caller is the only party and the auto-approval covers nothing it asks for",
            members: json!([]),
            auto_approved: Some(vec!["GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY"]),
            change: member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["ADD_MEMBER"],
            expect_settled: true,
        },
    ];

    for case in cases {
        let label = case.case;
        let mut expected_parties: Vec<String> = vec![creator.to_string()];
        for member in case
            .members
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: the row's members are a list"))
        {
            expected_parties.push(
                member["accountId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: a member carries an accountId"))
                    .to_string(),
            );
        }
        expected_parties.sort();

        let collaboration_id = seeded_collaboration(
            &emulator,
            label,
            &["CAN_QUERY"],
            case.members.clone(),
            case.auto_approved.clone(),
        )
        .await;

        let (status, created) =
            create_change_request(&emulator, &collaboration_id, json!([case.change.clone()])).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateCollaborationChangeRequest 200, got {status} with {created}"
        );
        let request = &created["collaborationChangeRequest"];
        assert_eq!(
            types_of(&request["changes"][0]),
            sorted(&case.expect_types),
            "case {label:?}: expected types {:?}, got {request}",
            case.expect_types
        );
        assert_eq!(
            request["isAutoApproved"],
            json!(case.expect_settled),
            "case {label:?}: expected isAutoApproved {}, got {request}",
            case.expect_settled
        );
        let expected_status = if case.expect_settled {
            "COMMITTED"
        } else {
            "PENDING"
        };
        assert_eq!(
            request["status"],
            json!(expected_status),
            "case {label:?}: expected status {expected_status}, got {request}"
        );
        let expected_approval = if case.expect_settled {
            "APPROVED"
        } else {
            "PENDING"
        };
        assert_eq!(
            approvals_of(request),
            expected_parties
                .iter()
                .map(|account| (account.clone(), expected_approval.to_string()))
                .collect::<Vec<_>>(),
            "case {label:?}: expected {expected_approval} for every party {expected_parties:?}, got {request}"
        );

        // The persisted record is what a later reader sees, not just the create response.
        let id = request["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the request carries an id: {request}"));
        let (status, read) = get_change_request(&emulator, &collaboration_id, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetCollaborationChangeRequest 200, got {status} with {read}"
        );
        assert_eq!(
            read["collaborationChangeRequest"], *request,
            "case {label:?}: expected the settled request to read back unchanged, got {read}"
        );

        // A settled request has been APPLIED to the collaboration, so the very same change
        // now asks for nothing the collaboration does not already have (`ChangeTypeList`
        // is len 1..); an unsettled one applied nothing, so it can still be asked for.
        let (status, again) =
            create_change_request(&emulator, &collaboration_id, json!([case.change])).await;
        if case.expect_settled {
            assert_eq!(
                status, 400,
                "case {label:?}: the applied change leaves nothing to ask for, got {status} with {again}"
            );
            assert_eq!(
                error_code(&again),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {again}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: an unsettled request applies nothing, so the change is still askable, got {status} with {again}"
            );
        }
    }
}

/// One row of the modification-guard table.
struct GuardCase {
    /// The status the request is in when the action arrives.
    status: &'static str,
    action: &'static str,
    /// Whether rule 2 refuses the action.
    expect_refused: bool,
}

/// Rule 2 over the class: every action on a COMMITTED or CANCELLED request is refused the
/// same way, and the statuses the rule does not name stay actionable.
#[tokio::test]
async fn a_committed_or_cancelled_request_refuses_every_action() {
    let emulator = Emulator::new();

    let mut cases = Vec::new();
    for status in ["COMMITTED", "CANCELLED"] {
        for action in ALL_ACTIONS {
            cases.push(GuardCase {
                status,
                action,
                expect_refused: true,
            });
        }
    }
    // The contrast rows: the two statuses the rule does not name each still admit an
    // action.
    cases.push(GuardCase {
        status: "PENDING",
        action: "APPROVE",
        expect_refused: false,
    });
    cases.push(GuardCase {
        status: "DENIED",
        action: "CANCEL",
        expect_refused: false,
    });

    for case in cases {
        let label = format!("{} from {}", case.action, case.status);
        let (collaboration_id, id) = request_in_status(&emulator, &label, case.status).await;
        let (_, before) = get_change_request(&emulator, &collaboration_id, &id).await;
        let before = before["collaborationChangeRequest"].clone();

        let (status, error_type, body) = emulator
            .call_with_error_type(
                "PATCH",
                &format!("/collaborations/{collaboration_id}/changeRequests/{id}"),
                Some(json!({ "action": case.action })),
            )
            .await;

        if !case.expect_refused {
            assert_eq!(
                status, 200,
                "case {label:?}: the status does not forbid modification, got {status} with {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {label:?}: expected x-amzn-errortype ValidationException, got {error_type:?} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: expected the body to name ValidationException, got {body}"
        );
        assert_eq!(
            body["reason"],
            json!("INVALID_CONFIGURATION"),
            "case {label:?}: expected reason INVALID_CONFIGURATION, got {body}"
        );
        // The message is composed from the request's change-request id and the status that
        // request is stored in, and states the rule those two fail.
        assert_eq!(
            error_message(&body),
            format!(
                "You can't update change request {id} because it's in {} status. Change requests \
                 in COMMITTED or CANCELLED status can't be modified.",
                case.status
            ),
            "case {label:?}: expected the message to name the id and the status, got {body}"
        );

        // A refused action changes nothing a later caller sees.
        let (status, after) = get_change_request(&emulator, &collaboration_id, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetCollaborationChangeRequest 200, got {status} with {after}"
        );
        assert_eq!(
            after["collaborationChangeRequest"], before,
            "case {label:?}: a refused action must leave the stored request untouched, got {after}"
        );
    }
}

/// A change request sitting in `status`, in a collaboration the caller owns. COMMITTED is
/// reached by the settlement rule itself (a sole party's own request); the rest are
/// reached from a PENDING request in a collaboration that has a second party.
async fn request_in_status(emulator: &Emulator, case: &str, status: &str) -> (String, String) {
    let members = if status == "COMMITTED" {
        json!([])
    } else {
        json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])])
    };
    let collaboration_id =
        seeded_collaboration(emulator, case, &["CAN_QUERY"], members, None).await;
    let (code, created) = create_change_request(
        emulator,
        &collaboration_id,
        json!([member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"])]),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateCollaborationChangeRequest 200, got {code} with {created}"
    );
    let id = created["collaborationChangeRequest"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created request carries an id: {created}"))
        .to_string();

    let walk: &[&str] = match status {
        "COMMITTED" | "PENDING" => &[],
        "CANCELLED" => &["CANCEL"],
        "DENIED" => &["DENY"],
        other => panic!("case {case:?}: no walk to status {other:?}"),
    };
    for action in walk {
        let (code, updated) = update_change_request(emulator, &collaboration_id, &id, action).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected {action} 200 while reaching {status}, got {code} with {updated}"
        );
    }
    let (code, read) = get_change_request(emulator, &collaboration_id, &id).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetCollaborationChangeRequest 200, got {code} with {read}"
    );
    assert_eq!(
        read["collaborationChangeRequest"]["status"],
        json!(status),
        "case {case:?}: expected a request in {status} to start from, got {read}"
    );
    (collaboration_id, id)
}
