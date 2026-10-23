// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateCollaborationChangeRequest` and
//! `GetCollaborationChangeRequest`.
//!
//! The rule under test: a created change request's `changes[*].types` is the DIFFERENCE
//! between the specification the request carried and the collaboration as stored, named
//! from the model's `ChangeType` catalog; `isAutoApproved` is true exactly when the
//! collaboration's `autoApprovedChangeTypes` covers every one of those types, in which
//! case the change is applied to the collaboration and the request is COMMITTED with
//! every approval APPROVED; otherwise it is stored PENDING with a PENDING approval per
//! party to the collaboration and applies nothing. A specification that asks for no
//! modification the collaboration does not already have, or one the catalog cannot name,
//! is refused (`ChangeTypeList` is `len 1..`). Every stored request reads back unchanged
//! through `GetCollaborationChangeRequest`.
//!
//! The tables vary what the rule generalises over: which ability the specification moves
//! and in which direction (every grant/revoke pair the `ChangeType` catalog names), how
//! many types one change produces, whether the subject is already a party, which kind of
//! specification the change carries, which cost makes a member a payer candidate, and
//! every value of the `AutoApprovedChangeType` catalog. Requests go over the wire
//! through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, is_model_uuid, minimal_collaboration, Emulator,
};

/// A 12-digit `AccountId` (model shape `AccountId`, len 12..12, pattern `^\d+$`) that is
/// not the caller's, used as a collaboration member in these tables.
const PARTNER_ACCOUNT: &str = "111122223333";
/// A second non-caller `AccountId`, for the rows that add an account to a collaboration.
const NEWCOMER_ACCOUNT: &str = "444455556666";

/// Every `AutoApprovedChangeType` the model declares (shape `AutoApprovedChangeType`) —
/// the 5-value subset of the 12-value `ChangeType` catalog a collaboration can
/// auto-approve.
const ALL_AUTO_APPROVED_CHANGE_TYPES: [&str; 5] = [
    "ADD_MEMBER",
    "GRANT_RECEIVE_RESULTS_ABILITY",
    "REVOKE_RECEIVE_RESULTS_ABILITY",
    "GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
    "REVOKE_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
];

/// Create a collaboration for one row: `minimal_collaboration` with the members the row
/// prescribes, the creator abilities it prescribes and, when given, the change types the
/// collaboration auto-approves. Returns its id.
async fn seeded_collaboration(
    emulator: &Emulator,
    case: &str,
    creator_abilities: &[&str],
    members: Value,
    auto_approved: Option<Vec<&str>>,
) -> String {
    let mut body = minimal_collaboration(case);
    body["creatorMemberAbilities"] = json!(creator_abilities);
    body["members"] = members;
    if let Some(auto_approved) = auto_approved {
        body["autoApprovedChangeRequestTypes"] = json!(auto_approved);
    }
    let (status, created) = emulator.create_collaboration(body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created collaboration carries an id"))
        .to_string()
}

async fn create_change_request(
    emulator: &Emulator,
    collaboration_id: &str,
    changes: Value,
) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/collaborations/{collaboration_id}/changeRequests"),
            Some(json!({ "changes": changes })),
        )
        .await
}

async fn get_change_request(
    emulator: &Emulator,
    collaboration_id: &str,
    change_request_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/collaborations/{collaboration_id}/changeRequests/{change_request_id}"),
            None,
        )
        .await
}

/// A `MEMBER` change for `account` stating the abilities it should hold.
fn member_change(account: &str, abilities: &[&str]) -> Value {
    json!({
        "specificationType": "MEMBER",
        "specification": {
            "member": {
                "accountId": account,
                "memberAbilities": abilities,
                "displayName": "partner",
            }
        }
    })
}

/// A declared collaboration member (`MemberSpecification`) holding `abilities`.
fn declared_member(account: &str, abilities: &[&str]) -> Value {
    json!({
        "accountId": account,
        "memberAbilities": abilities,
        "displayName": "partner",
    })
}

/// The `changes[*].types` a response reports, sorted so a row asserts a SET.
fn types_of(change: &Value) -> Vec<String> {
    let mut types: Vec<String> = change["types"]
        .as_array()
        .unwrap_or_else(|| panic!("the change carries types: {change}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("a change type is a string: {change}"))
                .to_string()
        })
        .collect();
    types.sort();
    types
}

fn sorted(values: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = values.iter().map(|value| value.to_string()).collect();
    sorted.sort();
    sorted
}

/// The `(account, approval status)` pairs a change request carries, sorted.
fn approvals_of(request: &Value) -> Vec<(String, String)> {
    let mut approvals: Vec<(String, String)> = request["approvals"]
        .as_object()
        .unwrap_or_else(|| panic!("the change request carries approvals: {request}"))
        .iter()
        .map(|(account, details)| {
            (
                account.clone(),
                details["status"]
                    .as_str()
                    .unwrap_or_else(|| panic!("an approval carries a status: {request}"))
                    .to_string(),
            )
        })
        .collect();
    approvals.sort();
    approvals
}

/// One row of the derived-change-types table.
struct TypesCase {
    case: &'static str,
    creator_abilities: Vec<&'static str>,
    members: Value,
    change: Value,
    expect_types: Vec<&'static str>,
}

/// The service derives each change's `types` from the stored collaboration, covering
/// every grant/revoke pair the `ChangeType` catalog names plus the add, payer-candidate
/// and collaboration-setting types — and reports a request nobody auto-approves as
/// PENDING, with one PENDING approval per party.
#[tokio::test]
async fn change_request_types_are_derived_from_the_stored_collaboration() {
    let emulator = Emulator::new();
    let creator = account_id();

    let cases = vec![
        TypesCase {
            case: "an account that is not a party yet is added",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([]),
            change: member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["ADD_MEMBER"],
        },
        TypesCase {
            case: "a member gains the receive-results ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            change: member_change(PARTNER_ACCOUNT, &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]),
            expect_types: vec!["GRANT_RECEIVE_RESULTS_ABILITY"],
        },
        TypesCase {
            case: "a member loses the receive-results ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(
                PARTNER_ACCOUNT,
                &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]
            )]),
            change: member_change(PARTNER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["REVOKE_RECEIVE_RESULTS_ABILITY"],
        },
        TypesCase {
            case: "a member gains the export-query-analysis-log ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            change: member_change(
                PARTNER_ACCOUNT,
                &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"],
            ),
            expect_types: vec!["GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY"],
        },
        TypesCase {
            case: "a member loses the export-query-analysis-log ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(
                PARTNER_ACCOUNT,
                &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"]
            )]),
            change: member_change(PARTNER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["REVOKE_EXPORT_QUERY_ANALYSIS_LOG_ABILITY"],
        },
        TypesCase {
            case: "one change moves two abilities at once",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(
                PARTNER_ACCOUNT,
                &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]
            )]),
            change: member_change(
                PARTNER_ACCOUNT,
                &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"],
            ),
            expect_types: vec![
                "REVOKE_RECEIVE_RESULTS_ABILITY",
                "GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
            ],
        },
        TypesCase {
            case: "the collaboration creator gains an ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([]),
            change: member_change(creator, &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]),
            expect_types: vec!["GRANT_RECEIVE_RESULTS_ABILITY"],
        },
        TypesCase {
            case: "a member gains the model-output ML ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            change: json!({
                "specificationType": "MEMBER",
                "specification": { "member": {
                    "accountId": PARTNER_ACCOUNT,
                    "memberAbilities": ["CAN_QUERY"],
                    "mlMemberAbilities": { "customMLMemberAbilities": ["CAN_RECEIVE_MODEL_OUTPUT"] },
                    "displayName": "partner",
                }}
            }),
            expect_types: vec!["GRANT_CAN_RECEIVE_MODEL_OUTPUT"],
        },
        TypesCase {
            case: "a member loses the model-output ML ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([{
                "accountId": PARTNER_ACCOUNT,
                "memberAbilities": ["CAN_QUERY"],
                "mlMemberAbilities": { "customMLMemberAbilities": ["CAN_RECEIVE_MODEL_OUTPUT"] },
                "displayName": "partner",
            }]),
            change: member_change(PARTNER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["REVOKE_CAN_RECEIVE_MODEL_OUTPUT"],
        },
        TypesCase {
            case: "a member gains the inference-output ML ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            change: json!({
                "specificationType": "MEMBER",
                "specification": { "member": {
                    "accountId": PARTNER_ACCOUNT,
                    "memberAbilities": ["CAN_QUERY"],
                    "mlMemberAbilities": { "customMLMemberAbilities": ["CAN_RECEIVE_INFERENCE_OUTPUT"] },
                    "displayName": "partner",
                }}
            }),
            expect_types: vec!["GRANT_CAN_RECEIVE_INFERENCE_OUTPUT"],
        },
        TypesCase {
            case: "a member loses the inference-output ML ability",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([{
                "accountId": PARTNER_ACCOUNT,
                "memberAbilities": ["CAN_QUERY"],
                "mlMemberAbilities": { "customMLMemberAbilities": ["CAN_RECEIVE_INFERENCE_OUTPUT"] },
                "displayName": "partner",
            }]),
            change: member_change(PARTNER_ACCOUNT, &["CAN_QUERY"]),
            expect_types: vec!["REVOKE_CAN_RECEIVE_INFERENCE_OUTPUT"],
        },
        TypesCase {
            case: "a member becomes responsible for query compute",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            change: json!({
                "specificationType": "MEMBER",
                "specification": { "member": {
                    "accountId": PARTNER_ACCOUNT,
                    "memberAbilities": ["CAN_QUERY"],
                    "paymentConfiguration": { "queryCompute": { "isResponsible": true } },
                    "displayName": "partner",
                }}
            }),
            expect_types: vec!["ADD_PAYER_CANDIDATE"],
        },
        TypesCase {
            case: "a member becomes responsible for a machine-learning cost only",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            change: json!({
                "specificationType": "MEMBER",
                "specification": { "member": {
                    "accountId": PARTNER_ACCOUNT,
                    "memberAbilities": ["CAN_QUERY"],
                    "paymentConfiguration": {
                        "queryCompute": { "isResponsible": false },
                        "machineLearning": { "modelTraining": { "isResponsible": true } },
                    },
                    "displayName": "partner",
                }}
            }),
            expect_types: vec!["ADD_PAYER_CANDIDATE"],
        },
        TypesCase {
            case: "a member stops being responsible for every cost",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([{
                "accountId": PARTNER_ACCOUNT,
                "memberAbilities": ["CAN_QUERY"],
                "paymentConfiguration": { "queryCompute": { "isResponsible": true } },
                "displayName": "partner",
            }]),
            change: json!({
                "specificationType": "MEMBER",
                "specification": { "member": {
                    "accountId": PARTNER_ACCOUNT,
                    "memberAbilities": ["CAN_QUERY"],
                    "paymentConfiguration": { "queryCompute": { "isResponsible": false } },
                    "displayName": "partner",
                }}
            }),
            expect_types: vec!["REMOVE_PAYER_CANDIDATE"],
        },
        TypesCase {
            case: "the collaboration's auto-approved change types are edited",
            creator_abilities: vec!["CAN_QUERY"],
            members: json!([]),
            change: json!({
                "specificationType": "COLLABORATION",
                "specification": { "collaboration": {
                    "autoApprovedChangeTypes": ["ADD_MEMBER"],
                }}
            }),
            expect_types: vec!["EDIT_AUTO_APPROVED_CHANGE_TYPES"],
        },
    ];

    for case in cases {
        let label = case.case;
        let expected_parties = {
            let mut parties: Vec<String> = vec![creator.to_string()];
            for member in case.members.as_array().expect("the members list") {
                parties.push(
                    member["accountId"]
                        .as_str()
                        .expect("a member account id")
                        .to_string(),
                );
            }
            parties.sort();
            parties
        };
        let collaboration_id = seeded_collaboration(
            &emulator,
            label,
            &case.creator_abilities,
            case.members.clone(),
            None,
        )
        .await;

        let (status, created) =
            create_change_request(&emulator, &collaboration_id, json!([case.change.clone()])).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateCollaborationChangeRequest 200, got {status} with {created}"
        );
        let request = &created["collaborationChangeRequest"];

        let changes = request["changes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: the request carries changes: {request}"));
        assert_eq!(
            changes.len(),
            1,
            "case {label:?}: expected one change, got {request}"
        );
        assert_eq!(
            types_of(&changes[0]),
            sorted(&case.expect_types),
            "case {label:?}: expected types {:?}, got {}",
            case.expect_types,
            changes[0]
        );
        assert_eq!(
            changes[0]["specificationType"], case.change["specificationType"],
            "case {label:?}: expected the specificationType the request sent, got {}",
            changes[0]
        );
        assert_eq!(
            changes[0]["specification"], case.change["specification"],
            "case {label:?}: expected the specification the request sent, got {}",
            changes[0]
        );

        let id = request["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the request carries an id: {request}"));
        assert!(
            is_model_uuid(id),
            "case {label:?}: expected a UUID-shaped id, got {id:?}"
        );
        assert_eq!(
            request["collaborationId"], json!(collaboration_id),
            "case {label:?}: expected the collaboration it was made against, got {request}"
        );
        assert_eq!(
            request["createTime"], request["updateTime"],
            "case {label:?}: a created request has not been updated yet, got {request}"
        );
        assert_eq!(
            request["status"], json!("PENDING"),
            "case {label:?}: a change request nobody auto-approves is PENDING, got {request}"
        );
        assert_eq!(
            request["isAutoApproved"], json!(false),
            "case {label:?}: the collaboration auto-approves nothing, got {request}"
        );
        assert_eq!(
            approvals_of(request),
            expected_parties
                .iter()
                .map(|account| (account.clone(), "PENDING".to_string()))
                .collect::<Vec<_>>(),
            "case {label:?}: expected one PENDING approval per party {expected_parties:?}, got {request}"
        );

        let (status, read) = get_change_request(&emulator, &collaboration_id, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetCollaborationChangeRequest 200, got {status} with {read}"
        );
        assert_eq!(
            read["collaborationChangeRequest"], *request,
            "case {label:?}: expected the stored request to read back unchanged, got {read}"
        );
    }
}

/// A change request whose every type the collaboration auto-approves is COMMITTED with
/// every approval APPROVED, and the change is APPLIED: asking for the same thing again
/// is then a request for nothing. Every value of the `AutoApprovedChangeType` catalog is
/// a row.
#[tokio::test]
async fn auto_approved_change_requests_are_committed_and_applied() {
    let emulator = Emulator::new();
    let creator = account_id();

    for auto_approved_type in ALL_AUTO_APPROVED_CHANGE_TYPES {
        // The member the row starts from, and the change that produces exactly
        // `auto_approved_type` against it.
        let (members, change, parties) = match auto_approved_type {
            "ADD_MEMBER" => (
                json!([]),
                member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"]),
                vec![creator.to_string()],
            ),
            "GRANT_RECEIVE_RESULTS_ABILITY" => (
                json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
                member_change(PARTNER_ACCOUNT, &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]),
                vec![creator.to_string(), PARTNER_ACCOUNT.to_string()],
            ),
            "REVOKE_RECEIVE_RESULTS_ABILITY" => (
                json!([declared_member(
                    PARTNER_ACCOUNT,
                    &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]
                )]),
                member_change(PARTNER_ACCOUNT, &["CAN_QUERY"]),
                vec![creator.to_string(), PARTNER_ACCOUNT.to_string()],
            ),
            "GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY" => (
                json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
                member_change(
                    PARTNER_ACCOUNT,
                    &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"],
                ),
                vec![creator.to_string(), PARTNER_ACCOUNT.to_string()],
            ),
            "REVOKE_EXPORT_QUERY_ANALYSIS_LOG_ABILITY" => (
                json!([declared_member(
                    PARTNER_ACCOUNT,
                    &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"]
                )]),
                member_change(PARTNER_ACCOUNT, &["CAN_QUERY"]),
                vec![creator.to_string(), PARTNER_ACCOUNT.to_string()],
            ),
            other => panic!("case {other:?}: the table has no row for this change type"),
        };
        let mut expected_approvals: Vec<(String, String)> = parties
            .iter()
            .map(|account| (account.clone(), "APPROVED".to_string()))
            .collect();
        expected_approvals.sort();

        let collaboration_id = seeded_collaboration(
            &emulator,
            auto_approved_type,
            &["CAN_QUERY"],
            members,
            Some(vec![auto_approved_type]),
        )
        .await;

        let (status, created) =
            create_change_request(&emulator, &collaboration_id, json!([change.clone()])).await;
        assert_eq!(
            status, 200,
            "case {auto_approved_type:?}: expected 200, got {status} with {created}"
        );
        let request = &created["collaborationChangeRequest"];
        assert_eq!(
            types_of(&request["changes"][0]),
            sorted(&[auto_approved_type]),
            "case {auto_approved_type:?}: expected exactly that change type, got {request}"
        );
        assert_eq!(
            request["isAutoApproved"], json!(true),
            "case {auto_approved_type:?}: the collaboration auto-approves this type, got {request}"
        );
        assert_eq!(
            request["status"], json!("COMMITTED"),
            "case {auto_approved_type:?}: an auto-approved request needs no decision, got {request}"
        );
        assert_eq!(
            approvals_of(request),
            expected_approvals,
            "case {auto_approved_type:?}: every approval of an auto-approved request is APPROVED, got {request}"
        );

        // The change was applied, so the collaboration now already holds what the
        // request asked for: the same request asks for nothing.
        let (status, again) =
            create_change_request(&emulator, &collaboration_id, json!([change])).await;
        assert_eq!(
            status, 400,
            "case {auto_approved_type:?}: expected the applied change to leave nothing to ask for, got {status} with {again}"
        );
        assert_eq!(
            error_code(&again),
            "ValidationException",
            "case {auto_approved_type:?}: expected ValidationException, got {again}"
        );
    }
}

/// A request is auto-approved only when the collaboration's setting covers EVERY type it
/// asks for, and a PENDING request applies nothing.
#[tokio::test]
async fn a_request_with_an_uncovered_type_stays_pending_and_applies_nothing() {
    let emulator = Emulator::new();

    // The collaboration auto-approves the grant but not the revoke; one change asking
    // for both is covered by neither.
    let collaboration_id = seeded_collaboration(
        &emulator,
        "one covered type and one uncovered type",
        &["CAN_QUERY"],
        json!([declared_member(
            PARTNER_ACCOUNT,
            &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]
        )]),
        Some(vec!["GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY"]),
    )
    .await;

    let change = member_change(
        PARTNER_ACCOUNT,
        &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"],
    );
    let (status, created) =
        create_change_request(&emulator, &collaboration_id, json!([change.clone()])).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {created}",
        "mixed covered and uncovered types"
    );
    let request = &created["collaborationChangeRequest"];
    assert_eq!(
        types_of(&request["changes"][0]),
        sorted(&[
            "GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
            "REVOKE_RECEIVE_RESULTS_ABILITY",
        ]),
        "case {:?}: expected both derived types, got {request}",
        "mixed covered and uncovered types"
    );
    assert_eq!(
        request["isAutoApproved"], json!(false),
        "case {:?}: one uncovered type is enough to need a decision, got {request}",
        "mixed covered and uncovered types"
    );
    assert_eq!(
        request["status"], json!("PENDING"),
        "case {:?}: expected PENDING, got {request}",
        "mixed covered and uncovered types"
    );

    // Nothing was applied, so the very same change can still be asked for.
    let (status, repeated) =
        create_change_request(&emulator, &collaboration_id, json!([change])).await;
    assert_eq!(
        status, 200,
        "case {:?}: a pending request applies nothing, so the change is still askable, got {status} with {repeated}",
        "mixed covered and uncovered types"
    );

    // And a PENDING edit of the collaboration's own settings leaves them untouched.
    let (status, edit) = create_change_request(
        &emulator,
        &collaboration_id,
        json!([{
            "specificationType": "COLLABORATION",
            "specification": { "collaboration": { "autoApprovedChangeTypes": ["ADD_MEMBER"] } }
        }]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {edit}",
        "pending edit of the auto-approval setting"
    );
    assert_eq!(
        edit["collaborationChangeRequest"]["status"], json!("PENDING"),
        "case {:?}: EDIT_AUTO_APPROVED_CHANGE_TYPES is not in the AutoApprovedChangeType catalog, got {edit}",
        "pending edit of the auto-approval setting"
    );
    let (status, collaboration) = emulator.get_collaboration(&collaboration_id).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected GetCollaboration 200, got {status} with {collaboration}",
        "pending edit of the auto-approval setting"
    );
    assert_eq!(
        collaboration["collaboration"]["autoApprovedChangeTypes"],
        json!(["GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY"]),
        "case {:?}: a pending request must not move the collaboration's setting, got {collaboration}",
        "pending edit of the auto-approval setting"
    );
}

/// A committed member change restates the membership that account already holds in the
/// collaboration: a membership reports the same grant from the member's side.
#[tokio::test]
async fn a_committed_member_change_restates_the_membership() {
    let emulator = Emulator::new();
    let collaboration_id = seeded_collaboration(
        &emulator,
        "a committed grant reaches the membership",
        &["CAN_QUERY"],
        json!([]),
        Some(vec!["GRANT_RECEIVE_RESULTS_ABILITY"]),
    )
    .await;

    let (status, membership) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateMembership 200, got {status} with {membership}",
        "a committed grant reaches the membership"
    );
    let membership_id = membership["membership"]["id"]
        .as_str()
        .expect("the created membership carries an id")
        .to_string();
    assert_eq!(
        membership["membership"]["memberAbilities"],
        json!(["CAN_QUERY"]),
        "case {:?}: the membership starts with the creator's declared abilities, got {membership}",
        "a committed grant reaches the membership"
    );

    let (status, created) = create_change_request(
        &emulator,
        &collaboration_id,
        json!([member_change(
            account_id(),
            &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]
        )]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {created}",
        "a committed grant reaches the membership"
    );
    assert_eq!(
        created["collaborationChangeRequest"]["status"], json!("COMMITTED"),
        "case {:?}: the collaboration auto-approves this grant, got {created}",
        "a committed grant reaches the membership"
    );

    let (status, read) = emulator.get_membership(&membership_id).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected GetMembership 200, got {status} with {read}",
        "a committed grant reaches the membership"
    );
    let mut abilities: Vec<String> = read["membership"]["memberAbilities"]
        .as_array()
        .expect("the membership carries abilities")
        .iter()
        .map(|value| value.as_str().expect("an ability").to_string())
        .collect();
    abilities.sort();
    assert_eq!(
        abilities,
        sorted(&["CAN_QUERY", "CAN_RECEIVE_RESULTS"]),
        "case {:?}: expected the committed grant on the membership, got {read}",
        "a committed grant reaches the membership"
    );
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    members: Value,
    changes: Value,
    expect_status: u16,
    expect_code: &'static str,
}

/// A change request that asks for nothing representable, or that describes itself
/// inconsistently, is refused and persists nothing.
#[tokio::test]
async fn unrepresentable_change_requests_are_refused() {
    let emulator = Emulator::new();

    let cases = vec![
        RefusalCase {
            case: "a member specification identical to the stored grant",
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            changes: json!([member_change(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            expect_status: 400,
            expect_code: "ValidationException",
        },
        RefusalCase {
            case: "an ability change the ChangeType catalog cannot name",
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            changes: json!([member_change(PARTNER_ACCOUNT, &["CAN_RUN_JOB"])]),
            expect_status: 400,
            expect_code: "ValidationException",
        },
        RefusalCase {
            case: "a specificationType that names the other union member",
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            changes: json!([{
                "specificationType": "COLLABORATION",
                "specification": { "member": {
                    "accountId": PARTNER_ACCOUNT,
                    "memberAbilities": ["CAN_QUERY", "CAN_RECEIVE_RESULTS"],
                    "displayName": "partner",
                }}
            }]),
            expect_status: 400,
            expect_code: "ValidationException",
        },
        RefusalCase {
            case: "two changes for the same account in one request",
            members: json!([declared_member(PARTNER_ACCOUNT, &["CAN_QUERY"])]),
            changes: json!([
                member_change(PARTNER_ACCOUNT, &["CAN_QUERY", "CAN_RECEIVE_RESULTS"]),
                member_change(
                    PARTNER_ACCOUNT,
                    &["CAN_QUERY", "CAN_EXPORT_QUERY_ANALYSIS_LOG"]
                ),
            ]),
            expect_status: 400,
            expect_code: "ValidationException",
        },
        RefusalCase {
            case: "an added member without the displayName MemberSpecification requires",
            members: json!([]),
            changes: json!([{
                "specificationType": "MEMBER",
                "specification": { "member": {
                    "accountId": NEWCOMER_ACCOUNT,
                    "memberAbilities": ["CAN_QUERY"],
                }}
            }]),
            expect_status: 400,
            expect_code: "ValidationException",
        },
        RefusalCase {
            case: "an auto-approval setting the collaboration already has",
            members: json!([]),
            changes: json!([{
                "specificationType": "COLLABORATION",
                "specification": { "collaboration": { "autoApprovedChangeTypes": [] } }
            }]),
            expect_status: 400,
            expect_code: "ValidationException",
        },
    ];

    for case in cases {
        let label = case.case;
        let collaboration_id = seeded_collaboration(
            &emulator,
            label,
            &["CAN_QUERY"],
            case.members.clone(),
            None,
        )
        .await;
        let (status, refused) =
            create_change_request(&emulator, &collaboration_id, case.changes.clone()).await;
        assert_eq!(
            status, case.expect_status,
            "case {label:?}: expected HTTP {}, got {status} with {refused}",
            case.expect_status
        );
        assert_eq!(
            error_code(&refused),
            case.expect_code,
            "case {label:?}: expected {}, got {refused}",
            case.expect_code
        );
    }

    // A collaboration this account is not a party to is refused the way every other
    // operation that takes a `collaborationIdentifier` refuses it.
    let unknown = "00000000-0000-4000-8000-000000000000";
    let (status, refused) = create_change_request(
        &emulator,
        unknown,
        json!([member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"])]),
    )
    .await;
    assert_eq!(
        status, 403,
        "case {:?}: expected HTTP 403, got {status} with {refused}",
        "a collaboration this account is not a party to"
    );
    assert_eq!(
        error_code(&refused),
        "AccessDeniedException",
        "case {:?}: expected AccessDeniedException, got {refused}",
        "a collaboration this account is not a party to"
    );
}

/// `GetCollaborationChangeRequest` reads a change request only under the collaboration it
/// was made against.
#[tokio::test]
async fn reading_a_change_request_is_scoped_to_its_collaboration() {
    let emulator = Emulator::new();
    let first = seeded_collaboration(
        &emulator,
        "the collaboration the request was made against",
        &["CAN_QUERY"],
        json!([]),
        None,
    )
    .await;
    let second = seeded_collaboration(
        &emulator,
        "another collaboration of the same account",
        &["CAN_QUERY"],
        json!([]),
        None,
    )
    .await;

    let (status, created) = create_change_request(
        &emulator,
        &first,
        json!([member_change(NEWCOMER_ACCOUNT, &["CAN_QUERY"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {created}",
        "scoped read"
    );
    let id = created["collaborationChangeRequest"]["id"]
        .as_str()
        .expect("the created request carries an id")
        .to_string();

    let cases: [(&str, &str, &str, u16, &str); 3] = [
        ("the request under its own collaboration", &first, &id, 200, ""),
        (
            "the request under another collaboration",
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
    ];
    for (label, collaboration_id, change_request_id, expect_status, expect_code) in cases {
        let (status, body) = get_change_request(&emulator, collaboration_id, change_request_id).await;
        assert_eq!(
            status, expect_status,
            "case {label:?}: expected HTTP {expect_status}, got {status} with {body}"
        );
        if !expect_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expect_code,
                "case {label:?}: expected {expect_code}, got {body}"
            );
        }
    }

    // A collaboration this account is not a party to hides its change requests behind
    // the same refusal every other collaboration-scoped operation gives.
    let (status, refused) =
        get_change_request(&emulator, "00000000-0000-4000-8000-000000000000", &id).await;
    assert_eq!(
        status, 403,
        "case {:?}: expected HTTP 403, got {status} with {refused}",
        "a collaboration this account is not a party to"
    );
    assert_eq!(
        error_code(&refused),
        "AccessDeniedException",
        "case {:?}: expected AccessDeniedException, got {refused}",
        "a collaboration this account is not a party to"
    );
}
