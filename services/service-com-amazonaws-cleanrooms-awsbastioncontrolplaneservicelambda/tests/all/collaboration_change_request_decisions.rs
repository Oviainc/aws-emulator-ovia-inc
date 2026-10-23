// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCollaborationChangeRequest`.
//!
//! The rule under test: an update takes ONE `ChangeRequestAction` on a stored change
//! request and the request's own state decides the outcome — APPROVE and DENY record the
//! calling party's vote and the votes then settle the request's `status` (DENIED on any
//! denial, APPROVED once every party has approved, PENDING while a vote is outstanding),
//! the collaboration owner's CANCEL settles it CANCELLED, and the owner's COMMIT is
//! admitted only from APPROVED, which is the single action that applies the request's
//! changes to the collaboration. A request that can no longer be modified (COMMITTED or
//! CANCELLED) refuses every action with `ValidationException`; every decision that is
//! admitted restamps `updateTime` and reads back unchanged through
//! `GetCollaborationChangeRequest`.
//!
//! The tables vary what the rule generalises over: every value of the
//! `ChangeRequestAction` catalog against every `ChangeRequestStatus` a stored request can
//! be reached in, the NUMBER of parties whose votes a request waits for, and which kind of
//! `ChangeSpecification` a committed change applies. Requests go over the wire through the
//! same `MultiServiceRouter` the production router uses.
//!
//! APPROVED is not among the statuses these tables start from: a request reaches APPROVED
//! only once EVERY party has approved, and every emulated caller presents the one account
//! id, so a request whose other parties have votes outstanding can never collect them —
//! a request the caller is the only party to is settled at creation instead (see
//! [`super::collaboration_change_request_auto_commit`]).

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, Emulator};
use super::collaboration_change_requests::{
    approvals_of, create_change_request, declared_member, get_change_request, member_change,
    seeded_collaboration, sorted, types_of, NEWCOMER_ACCOUNT, PARTNER_ACCOUNT,
};

/// Every `ChangeRequestAction` the model declares (shape `ChangeRequestAction`).
const ALL_ACTIONS: [&str; 4] = ["APPROVE", "DENY", "CANCEL", "COMMIT"];

/// The `ChangeRequestStatus` values in which a stored request admits NO modification at
/// all, as the service's own refusal states the rule ("Change requests in COMMITTED or
/// CANCELLED status can't be modified").
const UNMODIFIABLE_STATUSES: [&str; 2] = ["COMMITTED", "CANCELLED"];

pub(crate) async fn update_change_request(
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

/// A collaboration with one party BESIDES the caller, plus one PENDING change request
/// against it (an `ADD_MEMBER` the collaboration does not auto-approve, so the other
/// party's approval is outstanding). Returns `(collaboration id, change request id)`.
async fn pending_request(emulator: &Emulator, case: &str) -> (String, String) {
    let collaboration_id = seeded_collaboration(
        emulator,
        case,
        &["CAN_QUERY"],
        json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
        None,
    )
    .await;
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

/// A collaboration whose only party is the caller, plus one change request against it.
/// No approval of that request is outstanding, so the service settles it COMMITTED at
/// creation. Returns `(collaboration id, change request id)`.
async fn committed_request(emulator: &Emulator, case: &str) -> (String, String) {
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
        json!("COMMITTED"),
        "case {case:?}: expected the sole party's own request to be settled at creation, got {created}"
    );
    let id = request["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created request carries an id: {created}"))
        .to_string();
    (collaboration_id, id)
}

/// A change request sitting in `status`. The caller is the collaboration's owner in every
/// case, so it can take every action.
async fn request_in_status(emulator: &Emulator, case: &str, status: &str) -> (String, String) {
    if status == "COMMITTED" {
        return committed_request(emulator, case).await;
    }
    let (collaboration_id, id) = pending_request(emulator, case).await;
    // The actions that walk a PENDING request to each remaining status; "PENDING" needs
    // none.
    let walk: &[&str] = match status {
        "PENDING" => &[],
        "DENIED" => &["DENY"],
        "CANCELLED" => &["CANCEL"],
        other => panic!("case {case:?}: no walk to status {other:?}"),
    };
    for action in walk {
        let (code, updated) = update_change_request(emulator, &collaboration_id, &id, action).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected {action} 200 while reaching {status}, got {code} with {updated}"
        );
        assert_eq!(
            updated["collaborationChangeRequest"]["status"],
            json!(status),
            "case {case:?}: expected the walk to leave the request {status}, got {updated}"
        );
    }
    (collaboration_id, id)
}

fn as_epoch(request: &Value, member: &str) -> f64 {
    request[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is an epoch-seconds timestamp: {request}"))
}

/// The calling account's own entry in a change request's `approvals` map.
fn caller_approval(request: &Value, caller: &str) -> String {
    approvals_of(request)
        .into_iter()
        .find(|(account, _)| account == caller)
        .map(|(_, status)| status)
        .unwrap_or_else(|| panic!("the request records the caller's approval: {request}"))
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
    /// The CALLER's own approval afterwards, for a row that succeeds.
    expect_approval: &'static str,
}

/// Every `ChangeRequestAction` against every `ChangeRequestStatus` a stored request can be
/// reached in: the action is admitted only where the request's status admits it, and the
/// status it leaves behind is the one the votes and the action prescribe.
#[tokio::test]
async fn each_action_is_admitted_only_from_the_status_that_allows_it() {
    let emulator = Emulator::new();
    let creator = account_id();

    let mut cases = vec![
        DecisionCase {
            from_status: "PENDING",
            action: "APPROVE",
            expect_http: 200,
            // The other party's vote is still outstanding, so the caller's approval does
            // not settle the request.
            expect_status: "PENDING",
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
            from_status: "DENIED",
            action: "APPROVE",
            expect_http: 409,
            expect_status: "",
            expect_code: "ConflictException",
            expect_approval: "",
        },
        DecisionCase {
            from_status: "DENIED",
            action: "DENY",
            expect_http: 409,
            expect_status: "",
            expect_code: "ConflictException",
            expect_approval: "",
        },
        DecisionCase {
            // A denied request is not one of the unmodifiable statuses, so the owner can
            // still withdraw it.
            from_status: "DENIED",
            action: "CANCEL",
            expect_http: 200,
            expect_status: "CANCELLED",
            expect_code: "",
            expect_approval: "DENIED",
        },
        DecisionCase {
            from_status: "DENIED",
            action: "COMMIT",
            expect_http: 409,
            expect_status: "",
            expect_code: "ConflictException",
            expect_approval: "",
        },
    ];
    // A request that can no longer be modified refuses every action with the same
    // ValidationException.
    for from_status in UNMODIFIABLE_STATUSES {
        for action in ALL_ACTIONS {
            cases.push(DecisionCase {
                from_status,
                action,
                expect_http: 400,
                expect_status: "",
                expect_code: "ValidationException",
                expect_approval: "",
            });
        }
    }

    for case in cases {
        let label = format!("{} from {}", case.action, case.from_status);
        let (collaboration_id, id) = request_in_status(&emulator, &label, case.from_status).await;
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
            caller_approval(request, creator),
            case.expect_approval,
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
            as_epoch(request, "updateTime") >= as_epoch(&before, "createTime"),
            "case {label:?}: expected updateTime at or after createTime, got {request}"
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

/// A request waits for EVERY party's vote: the caller's approval settles nothing while
/// another party has a vote outstanding, and such a request cannot be committed. The
/// row's variable is how many parties BESIDES the caller the collaboration has.
#[tokio::test]
async fn a_request_waits_for_every_party_that_has_a_vote_outstanding() {
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
        let request = &created["collaborationChangeRequest"];
        let id = request["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the request carries an id: {created}"))
            .to_string();

        if others.is_empty() {
            // Nothing is outstanding, so the request was settled at creation and admits
            // no action afterwards.
            assert_eq!(
                request["status"],
                json!("COMMITTED"),
                "case {label:?}: expected the sole party's request to be settled, got {request}"
            );
            let (status, refused) =
                update_change_request(&emulator, &collaboration_id, &id, "APPROVE").await;
            assert_eq!(
                status, 400,
                "case {label:?}: a settled request admits no vote, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {refused}"
            );
            continue;
        }

        assert_eq!(
            request["status"],
            json!("PENDING"),
            "case {label:?}: expected PENDING with {} vote(s) outstanding, got {request}",
            others.len()
        );

        let (status, voted) =
            update_change_request(&emulator, &collaboration_id, &id, "APPROVE").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected APPROVE 200, got {status} with {voted}"
        );
        let request = &voted["collaborationChangeRequest"];
        assert_eq!(
            request["status"],
            json!("PENDING"),
            "case {label:?}: expected PENDING with {} vote(s) still outstanding, got {request}",
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

        // A request with an outstanding vote has nothing to commit yet.
        let (status, committed) =
            update_change_request(&emulator, &collaboration_id, &id, "COMMIT").await;
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

/// A change reaches the collaboration only through a COMMITTED request: APPROVE, DENY and
/// CANCEL leave the collaboration exactly as it was, while a request settled at creation
/// has already applied it.
#[tokio::test]
async fn a_change_reaches_the_collaboration_only_once_the_request_is_committed() {
    let emulator = Emulator::new();
    let requested_setting = json!(["ADD_MEMBER", "GRANT_RECEIVE_RESULTS_ABILITY"]);

    // Each row drives an edit of the collaboration's own auto-approval setting to one
    // decision and then reads the collaboration back. The first three rows need the edit
    // to stay un-committed, so their collaboration has a second party whose approval is
    // outstanding; the last has only the caller, so the create settles it.
    let rows: [(&str, Value, &[&str], bool); 4] = [
        (
            "APPROVE leaves it unapplied",
            json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            &["APPROVE"],
            false,
        ),
        (
            "DENY leaves it unapplied",
            json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            &["DENY"],
            false,
        ),
        (
            "CANCEL leaves it unapplied",
            json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            &["CANCEL"],
            false,
        ),
        (
            "the sole party's own request applies it at creation",
            json!([]),
            &[],
            true,
        ),
    ];

    for (label, members, actions, expect_applied) in rows {
        let collaboration_id =
            seeded_collaboration(&emulator, label, &["CAN_QUERY"], members, None).await;
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

    // 1. An account that was not a party becomes one. The caller is the collaboration's
    //    only party, so its own request is committed at creation.
    let label = "a committed add makes the account a party";
    let collaboration_id =
        seeded_collaboration(&emulator, label, &["CAN_QUERY"], json!([]), None).await;
    let add = member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]);
    let (status, created) =
        create_change_request(&emulator, &collaboration_id, json!([add.clone()])).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected 200, got {status} with {created}"
    );
    assert_eq!(
        created["collaborationChangeRequest"]["status"],
        json!("COMMITTED"),
        "case {label:?}: expected the sole party's request to be committed, got {created}"
    );

    // Asking for the same add again now asks for nothing (`ChangeTypeList` is len 1..).
    let (status, again) = create_change_request(&emulator, &collaboration_id, json!([add])).await;
    assert_eq!(
        status, 400,
        "case {label:?}: the applied add leaves nothing to ask for, got {status} with {again}"
    );
    assert_eq!(
        error_code(&again),
        "ValidationException",
        "case {label:?}: expected ValidationException, got {again}"
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
        "case {label:?}: expected 200, got {status} with {next}"
    );
    let next_request = &next["collaborationChangeRequest"];
    assert_eq!(
        types_of(&next_request["changes"][0]),
        sorted(&["GRANT_RECEIVE_RESULTS_ABILITY"]),
        "case {label:?}: expected a grant against the stored member, got {next_request}"
    );
    assert_eq!(
        next_request["status"],
        json!("PENDING"),
        "case {label:?}: the added party's approval is outstanding, got {next_request}"
    );
    let mut expected_approvals = vec![
        (creator.to_string(), "PENDING".to_string()),
        (NEWCOMER_ACCOUNT.to_string(), "PENDING".to_string()),
    ];
    expected_approvals.sort();
    assert_eq!(
        approvals_of(next_request),
        expected_approvals,
        "case {label:?}: expected the added account among the approvers, got {next_request}"
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
    assert_eq!(
        created["collaborationChangeRequest"]["status"],
        json!("COMMITTED"),
        "case {label:?}: expected the sole party's request to be committed, got {created}"
    );

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
