// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCollaborationChangeRequest`.
//!
//! The rule under test: an update takes ONE `ChangeRequestAction` on a stored change
//! request and the request's own state decides the outcome — APPROVE and DENY record the
//! calling party's vote and the votes then settle the request's `status` (DENIED on any
//! denial, APPROVED once every party has approved, PENDING while a vote is outstanding),
//! the collaboration owner's CANCEL settles it CANCELLED, and the owner's COMMIT is
//! admitted only from APPROVED, which is the single action that applies the request's
//! changes to the collaboration. A request in a terminal status (COMMITTED, DENIED,
//! CANCELLED) admits no action, and every decision restamps `updateTime` and reads back
//! unchanged through `GetCollaborationChangeRequest`.
//!
//! The tables vary what the rule generalises over: every value of the
//! `ChangeRequestAction` catalog against every value of the `ChangeRequestStatus` catalog,
//! the NUMBER of parties whose votes a request waits for, and which kind of
//! `ChangeSpecification` a committed change applies. Requests go over the wire through the
//! same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, Emulator};
use super::collaboration_change_requests::{
    approvals_of, create_change_request, declared_member, get_change_request, member_change,
    seeded_collaboration, sorted, types_of, NEWCOMER_ACCOUNT, PARTNER_ACCOUNT,
};

/// Every `ChangeRequestAction` the model declares (shape `ChangeRequestAction`).
const ALL_ACTIONS: [&str; 4] = ["APPROVE", "DENY", "CANCEL", "COMMIT"];

/// Every `ChangeRequestStatus` the model declares (shape `ChangeRequestStatus`) that a
/// request can be sitting in when an action arrives.
const TERMINAL_STATUSES: [&str; 3] = ["COMMITTED", "DENIED", "CANCELLED"];

async fn update_change_request(
    emulator: &Emulator,
    collaboration_id: &str,
    change_request_id: &str,
    action: &str,
) -> (u16, Value) {
    emulator
        .call(
            "PATCH",
            &format!("/collaborations/{collaboration_id}/changeRequests/{change_request_id}"),
            Some(json!({ "action": action })),
        )
        .await
}

/// A collaboration whose only party is the caller, plus one PENDING change request
/// against it (an `ADD_MEMBER` the collaboration does not auto-approve). Returns
/// `(collaboration id, change request id)`.
async fn pending_request(emulator: &Emulator, case: &str) -> (String, String) {
    let collaboration_id =
        seeded_collaboration(emulator, case, &["CAN_QUERY"], json!([]), None).await;
    let (status, created) = create_change_request(
        emulator,
        &collaboration_id,
        json!([member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateCollaborationChangeRequest 200, got {status} with {created}"
    );
    let request = &created["collaborationChangeRequest"];
    assert_eq!(
        request["status"],
        json!("PENDING"),
        "case {case:?}: expected a PENDING request to start from, got {created}"
    );
    let id = request["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created request carries an id: {created}"))
        .to_string();
    (collaboration_id, id)
}

/// A change request sitting in `status`, in a collaboration whose only party is the
/// caller. The caller is that collaboration's owner, so it can take every action.
async fn request_in_status(
    emulator: &Emulator,
    case: &str,
    status: &str,
) -> (String, String, Value) {
    let (collaboration_id, id) = pending_request(emulator, case).await;
    // The actions that walk a PENDING request to each status; "PENDING" needs none.
    let walk: &[&str] = match status {
        "PENDING" => &[],
        "APPROVED" => &["APPROVE"],
        "DENIED" => &["DENY"],
        "CANCELLED" => &["CANCEL"],
        "COMMITTED" => &["APPROVE", "COMMIT"],
        other => panic!("case {case:?}: no walk to status {other:?}"),
    };
    let mut body = Value::Null;
    for action in walk {
        let (code, updated) = update_change_request(emulator, &collaboration_id, &id, action).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected {action} 200 while reaching {status}, got {code} with {updated}"
        );
        body = updated;
    }
    if !walk.is_empty() {
        assert_eq!(
            body["collaborationChangeRequest"]["status"],
            json!(status),
            "case {case:?}: expected the walk to leave the request {status}, got {body}"
        );
    }
    (collaboration_id, id, body)
}

fn as_epoch(request: &Value, member: &str) -> f64 {
    request[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is an epoch-seconds timestamp: {request}"))
}

/// One row of the action/status table.
struct DecisionCase {
    from_status: &'static str,
    action: &'static str,
    expect_http: u16,
    /// The `status` the request carries afterwards, for a row that succeeds.
    expect_status: &'static str,
    /// The wire error code, for a row that is refused.
    expect_code: &'static str,
    /// The caller's own approval afterwards, for a row that succeeds.
    expect_approval: &'static str,
}

/// Every `ChangeRequestAction` against every `ChangeRequestStatus`: the action is admitted
/// only where the request's status admits it, and the status it leaves behind is the one
/// the votes and the action prescribe.
#[tokio::test]
async fn each_action_is_admitted_only_from_the_status_that_allows_it() {
    let emulator = Emulator::new();
    let creator = account_id();

    let mut cases = vec![
        DecisionCase {
            from_status: "PENDING",
            action: "APPROVE",
            expect_http: 200,
            // The caller is the collaboration's only party, so its approval is the last
            // one outstanding.
            expect_status: "APPROVED",
            expect_code: "",
            expect_approval: "APPROVED",
        },
        DecisionCase {
            from_status: "PENDING",
            action: "DENY",
            expect_http: 200,
            expect_status: "DENIED",
            expect_code: "",
            expect_approval: "DENIED",
        },
        DecisionCase {
            from_status: "PENDING",
            action: "CANCEL",
            expect_http: 200,
            expect_status: "CANCELLED",
            expect_code: "",
            // A cancellation is not a vote: the recorded approval is untouched.
            expect_approval: "PENDING",
        },
        DecisionCase {
            from_status: "PENDING",
            action: "COMMIT",
            expect_http: 409,
            expect_status: "",
            expect_code: "ConflictException",
            expect_approval: "",
        },
        DecisionCase {
            from_status: "APPROVED",
            action: "APPROVE",
            expect_http: 409,
            expect_status: "",
            expect_code: "ConflictException",
            expect_approval: "",
        },
        DecisionCase {
            from_status: "APPROVED",
            action: "DENY",
            expect_http: 409,
            expect_status: "",
            expect_code: "ConflictException",
            expect_approval: "",
        },
        DecisionCase {
            from_status: "APPROVED",
            action: "CANCEL",
            expect_http: 200,
            expect_status: "CANCELLED",
            expect_code: "",
            expect_approval: "APPROVED",
        },
        DecisionCase {
            from_status: "APPROVED",
            action: "COMMIT",
            expect_http: 200,
            expect_status: "COMMITTED",
            expect_code: "",
            expect_approval: "APPROVED",
        },
    ];
    // A terminal request admits nothing at all, whichever action arrives.
    for from_status in TERMINAL_STATUSES {
        for action in ALL_ACTIONS {
            cases.push(DecisionCase {
                from_status,
                action,
                expect_http: 409,
                expect_status: "",
                expect_code: "ConflictException",
                expect_approval: "",
            });
        }
    }

    for case in cases {
        let label = format!("{} from {}", case.action, case.from_status);
        let (collaboration_id, id, _) = request_in_status(&emulator, &label, case.from_status).await;
        let (_, before) = get_change_request(&emulator, &collaboration_id, &id).await;
        let before = before["collaborationChangeRequest"].clone();

        let (status, body) =
            update_change_request(&emulator, &collaboration_id, &id, case.action).await;
        assert_eq!(
            status, case.expect_http,
            "case {label:?}: expected HTTP {}, got {status} with {body}",
            case.expect_http
        );

        if case.expect_http != 200 {
            assert_eq!(
                error_code(&body),
                case.expect_code,
                "case {label:?}: expected {}, got {body}",
                case.expect_code
            );
            // A refused decision changes nothing a later caller sees.
            let (status, after) = get_change_request(&emulator, &collaboration_id, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetCollaborationChangeRequest 200, got {status} with {after}"
            );
            assert_eq!(
                after["collaborationChangeRequest"], before,
                "case {label:?}: a refused decision must leave the stored request untouched, got {after}"
            );
            continue;
        }

        let request = &body["collaborationChangeRequest"];
        assert_eq!(
            request["status"],
            json!(case.expect_status),
            "case {label:?}: expected status {}, got {request}",
            case.expect_status
        );
        assert_eq!(
            approvals_of(request),
            vec![(creator.to_string(), case.expect_approval.to_string())],
            "case {label:?}: expected the caller's approval to be {}, got {request}",
            case.expect_approval
        );
        assert_eq!(
            request["id"], before["id"],
            "case {label:?}: an update must not mint a new request, got {request}"
        );
        assert_eq!(
            request["createTime"], before["createTime"],
            "case {label:?}: an update must not move createTime, got {request}"
        );
        assert!(
            as_epoch(request, "updateTime") > as_epoch(&before, "createTime"),
            "case {label:?}: expected updateTime after createTime, got {request}"
        );
        assert_eq!(
            request["changes"], before["changes"],
            "case {label:?}: a decision must not rewrite the changes, got {request}"
        );
        assert_eq!(
            request["isAutoApproved"], before["isAutoApproved"],
            "case {label:?}: a decision must not rewrite isAutoApproved, got {request}"
        );

        let (status, read) = get_change_request(&emulator, &collaboration_id, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetCollaborationChangeRequest 200, got {status} with {read}"
        );
        assert_eq!(
            read["collaborationChangeRequest"], *request,
            "case {label:?}: expected the decision to read back unchanged, got {read}"
        );
    }
}

/// A request waits for EVERY party's vote: the caller's approval settles it only when the
/// caller is the only party, and a request still holding a PENDING vote cannot be
/// committed. The row's variable is how many parties the collaboration has.
#[tokio::test]
async fn a_request_is_approved_only_once_every_party_has_voted() {
    let emulator = Emulator::new();
    let creator = account_id();

    let rows: [(&str, Value, Vec<&str>); 3] = [
        ("the caller is the only party", json!([]), vec![]),
        (
            "one other party",
            json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            vec![PARTNER_ACCOUNT],
        ),
        (
            "two other parties",
            json!([
                declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"]),
                declared_member("777788889999", &["CAN_QUERY"]),
            ]),
            vec![PARTNER_ACCOUNT, "777788889999"],
        ),
    ];

    for (label, members, others) in rows {
        let collaboration_id =
            seeded_collaboration(&emulator, label, &["CAN_QUERY"], members, None).await;
        let (status, created) = create_change_request(
            &emulator,
            &collaboration_id,
            json!([member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {created}"
        );
        let id = created["collaborationChangeRequest"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the request carries an id: {created}"))
            .to_string();

        let (status, voted) = update_change_request(&emulator, &collaboration_id, &id, "APPROVE").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected APPROVE 200, got {status} with {voted}"
        );
        let request = &voted["collaborationChangeRequest"];
        let expected_status = if others.is_empty() {
            "APPROVED"
        } else {
            "PENDING"
        };
        assert_eq!(
            request["status"],
            json!(expected_status),
            "case {label:?}: expected {expected_status} with {} vote(s) outstanding, got {request}",
            others.len()
        );

        let mut expected_approvals: Vec<(String, String)> =
            vec![(creator.to_string(), "APPROVED".to_string())];
        for other in &others {
            expected_approvals.push((other.to_string(), "PENDING".to_string()));
        }
        expected_approvals.sort();
        assert_eq!(
            approvals_of(request),
            expected_approvals,
            "case {label:?}: expected only the caller's vote recorded, got {request}"
        );

        // A request with an outstanding vote has nothing to commit yet; one every party
        // approved does.
        let (status, committed) =
            update_change_request(&emulator, &collaboration_id, &id, "COMMIT").await;
        if others.is_empty() {
            assert_eq!(
                status, 200,
                "case {label:?}: an approved request is committable, got {status} with {committed}"
            );
            assert_eq!(
                committed["collaborationChangeRequest"]["status"],
                json!("COMMITTED"),
                "case {label:?}: expected COMMITTED, got {committed}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {label:?}: a request awaiting a vote is not committable, got {status} with {committed}"
            );
            assert_eq!(
                error_code(&committed),
                "ConflictException",
                "case {label:?}: expected ConflictException, got {committed}"
            );
        }
    }
}

/// COMMIT is the only decision that applies a change: APPROVE, DENY and CANCEL leave the
/// collaboration exactly as it was.
#[tokio::test]
async fn only_a_commit_applies_the_change_to_the_collaboration() {
    let emulator = Emulator::new();

    // Each row drives a pending edit of the collaboration's own auto-approval setting to
    // one decision and then reads the collaboration back.
    let rows: [(&str, &[&str], bool); 4] = [
        ("APPROVE leaves it unapplied", &["APPROVE"], false),
        ("DENY leaves it unapplied", &["DENY"], false),
        ("CANCEL leaves it unapplied", &["CANCEL"], false),
        ("COMMIT applies it", &["APPROVE", "COMMIT"], true),
    ];
    let requested_setting = json!(["ADD_MEMBER", "GRANT_RECEIVE_RESULTS_ABILITY"]);

    for (label, actions, expect_applied) in rows {
        let collaboration_id =
            seeded_collaboration(&emulator, label, &["CAN_QUERY"], json!([]), None).await;
        let (status, created) = create_change_request(
            &emulator,
            &collaboration_id,
            json!([{
                "specificationType": "COLLABORATION",
                "specification": { "collaboration": {
                    "autoApprovedChangeTypes": requested_setting,
                }}
            }]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {created}"
        );
        let id = created["collaborationChangeRequest"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the request carries an id: {created}"))
            .to_string();

        for action in actions {
            let (status, body) =
                update_change_request(&emulator, &collaboration_id, &id, action).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected {action} 200, got {status} with {body}"
            );
        }

        let (status, collaboration) = emulator.get_collaboration(&collaboration_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetCollaboration 200, got {status} with {collaboration}"
        );
        let expected = if expect_applied {
            requested_setting.clone()
        } else {
            Value::Null
        };
        assert_eq!(
            collaboration["collaboration"]["autoApprovedChangeTypes"], expected,
            "case {label:?}: expected autoApprovedChangeTypes {expected}, got {collaboration}"
        );
    }
}

/// A committed MEMBER change reaches the collaboration's parties: the added account
/// becomes one (so it is an approver of the next request, and adding it again asks for
/// nothing), and the caller's own committed grant is restated on its membership.
#[tokio::test]
async fn a_committed_member_change_is_applied_to_the_collaboration() {
    let emulator = Emulator::new();
    let creator = account_id();

    // 1. An account that was not a party becomes one.
    let collaboration_id = seeded_collaboration(
        &emulator,
        "a committed add makes the account a party",
        &["CAN_QUERY"],
        json!([]),
        None,
    )
    .await;
    let add = member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]);
    let (status, created) =
        create_change_request(&emulator, &collaboration_id, json!([add.clone()])).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {created}",
        "a committed add makes the account a party"
    );
    let id = created["collaborationChangeRequest"]["id"]
        .as_str()
        .expect("the request carries an id")
        .to_string();
    for action in ["APPROVE", "COMMIT"] {
        let (status, body) = update_change_request(&emulator, &collaboration_id, &id, action).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected {action} 200, got {status} with {body}",
            "a committed add makes the account a party"
        );
    }

    // Asking for the same add again now asks for nothing (`ChangeTypeList` is len 1..).
    let (status, again) = create_change_request(&emulator, &collaboration_id, json!([add])).await;
    assert_eq!(
        status, 400,
        "case {:?}: the applied add leaves nothing to ask for, got {status} with {again}",
        "a committed add makes the account a party"
    );
    assert_eq!(
        error_code(&again),
        "ValidationException",
        "case {:?}: expected ValidationException, got {again}",
        "a committed add makes the account a party"
    );

    // And the account is now a party: the next request derives a GRANT against its stored
    // grant and records its vote as outstanding.
    let (status, next) = create_change_request(
        &emulator,
        &collaboration_id,
        json!([member_change(
            NEWCOMER_ACCOUNT,
            &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]
        )]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {next}",
        "a committed add makes the account a party"
    );
    let next_request = &next["collaborationChangeRequest"];
    assert_eq!(
        types_of(&next_request["changes"][0]),
        sorted(&["GRANT_RECEIVE_RESULTS_ABILITY"]),
        "case {:?}: expected a grant against the stored member, got {next_request}",
        "a committed add makes the account a party"
    );
    let mut expected_approvals = vec![
        (creator.to_string(), "PENDING".to_string()),
        (NEWCOMER_ACCOUNT.to_string(), "PENDING".to_string()),
    ];
    expected_approvals.sort();
    assert_eq!(
        approvals_of(next_request),
        expected_approvals,
        "case {:?}: expected the added account among the approvers, got {next_request}",
        "a committed add makes the account a party"
    );

    // 2. The caller's own committed grant is restated on the membership it holds.
    let label = "a committed grant reaches the caller's membership";
    let collaboration_id =
        seeded_collaboration(&emulator, label, &["CAN_QUERY"], json!([]), None).await;
    let (status, membership) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateMembership 200, got {status} with {membership}"
    );
    let membership_id = membership["membership"]["id"]
        .as_str()
        .expect("the created membership carries an id")
        .to_string();

    let (status, created) = create_change_request(
        &emulator,
        &collaboration_id,
        json!([member_change(creator, &["CAN_QUERY", "CAN_RECEIVE_RESULTS"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected 200, got {status} with {created}"
    );
    let id = created["collaborationChangeRequest"]["id"]
        .as_str()
        .expect("the request carries an id")
        .to_string();
    for action in ["APPROVE", "COMMIT"] {
        let (status, body) = update_change_request(&emulator, &collaboration_id, &id, action).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected {action} 200, got {status} with {body}"
        );
    }

    let (status, read) = emulator.get_membership(&membership_id).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected GetMembership 200, got {status} with {read}"
    );
    let mut abilities: Vec<String> = read["membership"]["memberAbilities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: the membership carries abilities: {read}"))
        .iter()
        .map(|value| value.as_str().expect("an ability").to_string())
        .collect();
    abilities.sort();
    assert_eq!(
        abilities,
        sorted(&["CAN_QUERY", "CAN_RECEIVE_RESULTS"]),
        "case {label:?}: expected the committed grant on the membership, got {read}"
    );
}

/// An update addresses a change request under the collaboration it was made against, and
/// refuses an unreachable collaboration the way every other collaboration-scoped
/// operation does.
#[tokio::test]
async fn updating_a_change_request_is_scoped_to_its_collaboration() {
    let emulator = Emulator::new();
    let (first, id) = pending_request(&emulator, "the request's own collaboration").await;
    let second =
        seeded_collaboration(&emulator, "another collaboration", &["CAN_QUERY"], json!([]), None)
            .await;

    let cases: [(&str, &str, &str, u16, &str); 3] = [
        (
            "a change request of another collaboration",
            &second,
            &id,
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id no change request is stored under",
            &first,
            "11111111-1111-4111-8111-111111111111",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a collaboration this account is not a party to",
            "00000000-0000-4000-8000-000000000000",
            &id,
            403,
            "AccessDeniedException",
        ),
    ];

    for (label, collaboration_id, change_request_id, expect_status, expect_code) in cases {
        let (status, body) =
            update_change_request(&emulator, collaboration_id, change_request_id, "APPROVE").await;
        assert_eq!(
            status, expect_status,
            "case {label:?}: expected HTTP {expect_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expect_code,
            "case {label:?}: expected {expect_code}, got {body}"
        );
    }

    // The request is still PENDING: none of those refusals touched it.
    let (status, read) = get_change_request(&emulator, &first, &id).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected GetCollaborationChangeRequest 200, got {status} with {read}",
        "the request's own collaboration"
    );
    assert_eq!(
        read["collaborationChangeRequest"]["status"],
        json!("PENDING"),
        "case {:?}: a refused update must not move the request, got {read}",
        "the request's own collaboration"
    );
}
