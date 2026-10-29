// SPDX-License-Identifier: CC0-1.0

//! `UpdateDelegationRequest`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket sees.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.
//!
//! The rules being asserted:
//!
//! * an identifier the account holds NO delegation request under references a resource
//!   entity that does not exist, so the call is refused with the modelled
//!   `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) whose message
//!   describes the request asked about, and nothing is filed under that identifier — the
//!   read that follows refuses it too;
//! * for a request the account HOLDS, the call writes the additional information it
//!   carried and takes the request to the `PENDING_APPROVAL` state, which is what the read
//!   of that identifier reports afterwards, leaving every member the update is not about
//!   as the account held it;
//! * a caller whose identity-based policies do not allow the action is refused, and a
//!   refused call writes nothing.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: the
//! identifier's alphabet and both published length bounds, whether the call carries notes
//! and what they are (both published length bounds, the published alphabet), the state
//! the request was in before the call — every value `stateType` declares — and what the
//! caller is permitted.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    DelegationRequest, STATE_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::delegation_request_reads::{held_delegation_requests, seed};
use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router, router_with_engine,
};

/// The published bounds of `notesType`, the type `Notes` is carried through: "len 0..500"
/// (`smithy_query(query="operation", operation="UpdateDelegationRequest")`).
const MIN_NOTES_CHARACTERS: usize = 0;
const MAX_NOTES_CHARACTERS: usize = 500;

/// The published minimum length of `delegationRequestIdType`, the type the identifier is
/// carried through: "len 16..128, pattern `^[\w-]+$`" (same `smithy_query` call).
const MIN_ID_CHARACTERS: usize = 16;

/// The state a request the account has just had filed for it carries, which every row
/// that files one starts from: a filing with no target account is "a request that can be
/// associated by any account" (`CreateDelegationRequest$OwnerAccountId`), one of the
/// values `stateType` declares.
const STATE_UNASSIGNED: &str = "UNASSIGNED";

/// The state this operation takes a delegation request to: "when the delegation request
/// is updated, it reaches the `PENDING_APPROVAL` state"
/// (`smithy_query(query="doc", shape="UpdateDelegationRequest")`).
const STATE_PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// The delegation request identifiers the not-found suite spans.
///
/// Source: `delegationRequestIdType` on `smithy_query(query="operation",
/// operation="UpdateDelegationRequest")` — length 16..128, pattern `^[\w-]+$` — so the
/// rows carry the whole published alphabet (letters in both cases, digits, `_`, `-`) and
/// BOTH published length bounds.
fn delegation_request_ids() -> Vec<String> {
    vec![
        // The published minimum length.
        "fedcba9876543210".to_string(),
        // The published maximum length.
        "q".repeat(128),
        // The whole published alphabet, in a mixed case.
        "Partner_Delegation-9z".to_string(),
        // All digits, with a leading run of zeroes: an identifier is a handle, not a
        // number.
        "00000000000000098765".to_string(),
        // The form the issue's reproduction builds: a prefix and a hex body.
        "sim2real-behav-0123456789abcdef0123456789abcdef".to_string(),
    ]
}

/// The notes a row of the round-trip table supplies, and what the read must report
/// afterwards.
///
/// Rows: no `Notes` member at all, both published length bounds of `notesType`, and a
/// value spanning the published alphabet (`^[\u0009\u000A\u000D\u0020-\u007E\u00A1-\u00FF]*$`:
/// tab, the printable ASCII range and the Latin-1 range above `\u00A0`).
fn notes_rows() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("no notes at all", None),
        (
            "the published minimum length",
            Some("n".repeat(MIN_NOTES_CHARACTERS)),
        ),
        (
            "a plain sentence",
            Some("approved once the boundary was attached".to_string()),
        ),
        (
            "the published maximum length",
            Some("n".repeat(MAX_NOTES_CHARACTERS)),
        ),
        (
            "the published alphabet",
            Some("caf\u{e9} \u{bf}ok?\t ~!@#$%^&*()_+ \u{ff}".to_string()),
        ),
    ]
}

/// One `UpdateDelegationRequest` call, presented as signed by `akid` when a row names
/// one, carrying `Notes` only when the row supplies them.
async fn update(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    delegation_request_id: &str,
    notes: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("DelegationRequestId", delegation_request_id)];
    if let Some(notes) = notes {
        params.push(("Notes", notes));
    }
    call_signed_by(router, akid, "UpdateDelegationRequest", &params).await
}

/// One `GetDelegationRequest` call for `delegation_request_id`: the implemented read the
/// round trip is proved through.
async fn get(router: &MultiServiceRouter, delegation_request_id: &str) -> (u16, String) {
    call(
        router,
        "GetDelegationRequest",
        &[("DelegationRequestId", delegation_request_id)],
    )
    .await
}

/// File a delegation request under the workflow id `workflow` and answer the identifier
/// IAM minted for it: the fixture every round-trip row starts from, driven through the
/// implemented `CreateDelegationRequest` rather than seeded, so what is updated is a
/// record this emulator wrote.
async fn file(router: &MultiServiceRouter, case: &str, workflow: &str) -> String {
    let (status, body) = call(
        router,
        "CreateDelegationRequest",
        &[
            ("Description", "temporary access for a partner engagement"),
            (
                "NotificationChannel",
                "arn:aws:sns:us-east-1:123456789012:partner-updates",
            ),
            (
                "Permissions.PolicyTemplateArn",
                "arn:aws:iam::aws:policy-template/PartnerDropReadWrite",
            ),
            ("RequestorWorkflowId", workflow),
            // The published minimum `SessionDuration` (range 300..43200).
            ("SessionDuration", "300"),
            ("OnlySendByOwner", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateDelegationRequest must succeed, got {status}: {body}"
    );
    element(&body, "DelegationRequestId")
}

/// The members this operation does NOT write, which the read must report unchanged across
/// an update: everything the filing stored but the notes, the state and the last-updated
/// instant.
const UNTOUCHED_MEMBERS: &[&str] = &[
    "DelegationRequestId",
    "OwnerAccountId",
    "Description",
    "RequestMessage",
    "PermissionPolicy",
    "OwnerId",
    "ApproverId",
    "RequestorId",
    "RequestorName",
    "RejectionReason",
    "RedirectUrl",
    "PolicyTemplateArn",
    "SessionDuration",
    "OnlySendByOwner",
    "CreateDate",
    "ExpirationTime",
];

/// The instant the member `tag` reports, in seconds, and `None` when the response carries
/// no such member.
fn reported_instant(case: &str, body: &str, tag: &str) -> Option<i64> {
    let reported = elements(body, tag);
    assert!(
        reported.len() <= 1,
        "case {case:?}: expected at most one <{tag}>, got {reported:?}"
    );
    reported.first().map(|text| {
        DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected an ISO-8601 {tag}, got {text:?}: {error}")
            })
            .secs()
    })
}

/// RULE: an identifier the account holds no delegation request under references "a
/// resource entity that does not exist", so the update is refused with the modelled
/// `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) whose message
/// describes the request asked about — whether or not the call carried notes — and the
/// refusal files nothing under that identifier, so the read of it is refused the same way
/// afterwards.
#[tokio::test]
async fn an_identifier_the_account_holds_nothing_under_is_refused_as_a_missing_entity() {
    for delegation_request_id in delegation_request_ids() {
        let (router, engine) = router_with_engine();
        // A row where the account holds ANOTHER delegation request, so what is asserted
        // is the lookup of the identifier the request named rather than whether the
        // account holds any request at all.
        let held = DelegationRequest {
            delegation_request_id: Some("held9876543210ab".to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        };
        seed(&engine, &held);

        for (label, notes) in notes_rows() {
            let case = format!("{delegation_request_id} with {label}");
            let (status, body) =
                update(&router, None, &delegation_request_id, notes.as_deref()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                format!("Delegation request not found: {delegation_request_id}"),
                "case {case:?}: the refusal must describe the delegation request asked \
                 about, got {:?}",
                element(&body, "Message")
            );
        }

        // The refused updates filed nothing: the account holds exactly the request it
        // held before, and the read of the rejected identifier is still a missing entity.
        assert_eq!(
            held_delegation_requests(&engine),
            vec!["held9876543210ab".to_string()],
            "case {delegation_request_id:?}: a refused update persists nothing, got {:?}",
            held_delegation_requests(&engine)
        );
        let (status, body) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 404,
            "case {delegation_request_id:?}: the read of the rejected identifier must \
             still be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {delegation_request_id:?}: expected NoSuchEntity from the read, got \
             {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE: for a request the account holds, the call writes the notes it carried and takes
/// the request to the `PENDING_APPROVAL` state — which is what the read of that
/// identifier reports afterwards, with the last-updated instant moved and every member
/// the update is not about left as the account held it.
#[tokio::test]
async fn an_update_is_what_the_read_reports() {
    let router = router();

    for (row, (label, notes)) in notes_rows().into_iter().enumerate() {
        let case = format!("update with {label}");
        let delegation_request_id = file(&router, &case, &format!("workflow-update-{row}")).await;

        let (status, before) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the filed request must be readable, got {status}: {before}"
        );
        assert_eq!(
            element(&before, "State"),
            STATE_UNASSIGNED,
            "case {case:?}: a filing with no target account is unassigned, got {:?}",
            element(&before, "State")
        );
        let filed_at = reported_instant(&case, &before, "CreateDate")
            .unwrap_or_else(|| panic!("case {case:?}: the filed request must report a CreateDate"));

        let (status, body) = update(&router, None, &delegation_request_id, notes.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be served, got {status}: {body}"
        );

        let (status, after) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the updated request must be readable, got {status}: {after}"
        );
        // The notes the call carried are what the read reports; a call carrying none
        // leaves the request with the notes it had, which for a freshly filed request is
        // no notes at all.
        assert_eq!(
            elements(&after, "Notes"),
            notes.iter().cloned().collect::<Vec<String>>(),
            "case {case:?}: expected the notes the update carried, got {:?}: {after}",
            elements(&after, "Notes")
        );
        assert_eq!(
            element(&after, "State"),
            STATE_PENDING_APPROVAL,
            "case {case:?}: an updated request reaches PENDING_APPROVAL, got {:?}",
            element(&after, "State")
        );
        let updated_at = reported_instant(&case, &after, "UpdatedTime")
            .unwrap_or_else(|| panic!("case {case:?}: an updated request reports an UpdatedTime"));
        assert!(
            updated_at >= filed_at,
            "case {case:?}: expected the last-updated instant at or after the filing's \
             {filed_at}, got {updated_at}"
        );
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                elements(&after, member),
                elements(&before, member),
                "case {case:?}: the update must leave {member} as the account held it, \
                 got {:?} after {:?}",
                elements(&after, member),
                elements(&before, member)
            );
        }
    }
}

/// RULE: the notes a request carries are ONE value of `notesType`, so a later update
/// states them again rather than appending to them, and an update that carries none
/// leaves the notes the account holds alone — while each of them takes the request to
/// `PENDING_APPROVAL` again.
#[tokio::test]
async fn a_later_update_states_the_notes_again() {
    let router = router();
    let case = "repeated updates";
    let delegation_request_id = file(&router, case, "workflow-update-repeated").await;

    // Rows: notes written onto a request that had none, notes REPLACED by a shorter
    // value, notes replaced by the published maximum, and an update carrying none at all
    // — which leaves the previous value standing.
    let longest = "n".repeat(MAX_NOTES_CHARACTERS);
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        (
            "the first notes",
            Some("waiting on the boundary"),
            "waiting on the boundary",
        ),
        ("a shorter replacement", Some("approved"), "approved"),
        ("the published maximum", Some(longest.as_str()), longest.as_str()),
        ("no notes at all", None, longest.as_str()),
    ];

    for (label, carried, expected) in rows {
        let case = format!("{case}: {label}");
        let (status, body) = update(&router, None, &delegation_request_id, carried).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be served, got {status}: {body}"
        );
        let (status, after) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the updated request must be readable, got {status}: {after}"
        );
        assert_eq!(
            elements(&after, "Notes"),
            vec![expected.to_string()],
            "case {case:?}: expected the notes the account now holds, got {:?}",
            elements(&after, "Notes")
        );
        assert_eq!(
            element(&after, "State"),
            STATE_PENDING_APPROVAL,
            "case {case:?}: an updated request reaches PENDING_APPROVAL, got {:?}",
            element(&after, "State")
        );
    }
}

/// RULE: the update is what takes a delegation request to `PENDING_APPROVAL`, whatever
/// state it was in before — so the rows are every value `stateType` declares, read from
/// the model's own value set (`ops::types::STATE_TYPE_VALUES`) rather than from the
/// states one workflow happens to pass through.
#[tokio::test]
async fn an_update_takes_a_request_in_any_state_to_pending_approval() {
    for (row, state) in STATE_TYPE_VALUES.iter().enumerate() {
        let (router, engine) = router_with_engine();
        let delegation_request_id = format!("statetransition{row:0>3}");
        seed(
            &engine,
            &DelegationRequest {
                delegation_request_id: Some(delegation_request_id.clone()),
                description: Some("a request in a state the update moves".to_string()),
                requestor_id: Some(account_id().to_string()),
                state: Some((*state).to_string()),
                notes: Some("the notes the request already carried".to_string()),
                ..DelegationRequest::default()
            },
        );

        let case = format!("a request in {state}");
        let (status, body) = update(
            &router,
            None,
            &delegation_request_id,
            Some("re-stated for approval"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be served, got {status}: {body}"
        );

        let (status, after) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the updated request must be readable, got {status}: {after}"
        );
        assert_eq!(
            element(&after, "State"),
            STATE_PENDING_APPROVAL,
            "case {case:?}: an updated request reaches PENDING_APPROVAL, got {:?}",
            element(&after, "State")
        );
        assert_eq!(
            elements(&after, "Notes"),
            vec!["re-stated for approval".to_string()],
            "case {case:?}: expected the notes the update carried, got {:?}",
            elements(&after, "Notes")
        );
        assert_eq!(
            element(&after, "Description"),
            "a request in a state the update moves",
            "case {case:?}: the update must leave the description alone, got {:?}",
            element(&after, "Description")
        );
    }
}

/// RULE: a value outside a bound its member declares is refused at the boundary with HTTP
/// 400 `ValidationError` naming the member and the bound — one past each published bound,
/// so the modelled refusal is what answers it rather than this operation's not-found
/// refusal, and nothing is written.
#[tokio::test]
async fn a_value_past_a_published_bound_is_refused_before_the_lookup() {
    let (router, engine) = router_with_engine();
    let delegation_request_id = "bounded0123456789";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(delegation_request_id.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    let short_id = "s".repeat(MIN_ID_CHARACTERS - 1);
    let long_notes = "n".repeat(MAX_NOTES_CHARACTERS + 1);
    let rows: Vec<(&str, &str, Option<&str>, String)> = vec![
        (
            "an identifier one character under the published minimum",
            short_id.as_str(),
            None,
            format!(
                "1 validation error detected: Value at 'delegationRequestId' failed to \
                 satisfy constraint: Member must have length greater than or equal to \
                 {MIN_ID_CHARACTERS}"
            ),
        ),
        (
            "notes one character over the published maximum",
            delegation_request_id,
            Some(long_notes.as_str()),
            format!(
                "1 validation error detected: Value at 'notes' failed to satisfy \
                 constraint: Member must have length less than or equal to \
                 {MAX_NOTES_CHARACTERS}"
            ),
        ),
    ];

    for (case, id, notes, expected) in rows {
        let (status, body) = update(&router, None, id, notes).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            element(&body, "Message")
        );
    }

    // The refused calls wrote nothing: the request the account holds is untouched.
    let (status, after) = get(&router, delegation_request_id).await;
    assert_eq!(
        status, 200,
        "the held request must still be readable, got {status}: {after}"
    );
    assert_eq!(
        element(&after, "State"),
        STATE_UNASSIGNED,
        "a refused call must not move the state, got {:?}",
        element(&after, "State")
    );
    assert!(
        elements(&after, "Notes").is_empty(),
        "a refused call must not write notes, got {:?}",
        elements(&after, "Notes")
    );
}

/// RULE: IAM decides the call against the caller's identity-based policies before it
/// touches the request, so a caller no policy allows the action for is refused with HTTP
/// 403 `AccessDenied` naming the principal, the action and the request — whether or not
/// the account holds that request — and the refused call writes nothing.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_call_is_refused() {
    let (router, engine) = router_with_engine();
    let held = "accessdenied01234";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(held.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    // Rows: a caller whose only grant is for ANOTHER action, and one explicitly denied
    // this action by a second statement.
    let rows = vec![
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:GetDelegationRequest", "*"),
            )],
            "because no identity-based policy allows the iam:UpdateDelegationRequest action"
                .to_string(),
        ),
        (
            "an explicit deny over the allow",
            vec![
                (
                    "allow".to_string(),
                    statement("Allow", "iam:UpdateDelegationRequest", "*"),
                ),
                (
                    "deny".to_string(),
                    statement("Deny", "iam:UpdateDelegationRequest", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
    ];

    for (row, (case, policies, reason)) in rows.into_iter().enumerate() {
        let user_name = format!("updater{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        for delegation_request_id in [held, "neverfiled0123456"] {
            let case = format!("{case} updating {delegation_request_id}");
            let (status, body) = update(
                &router,
                Some(&access_key_id),
                delegation_request_id,
                Some("a note the caller may not write"),
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}: {body}",
                error_code(&body)
            );
            assert_access_denied_message(
                &case,
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                "iam:UpdateDelegationRequest",
                &format!("delegation request {delegation_request_id}"),
                &reason,
            );
        }
    }

    // A refused caller changed nothing: the request the account holds carries the state
    // it was seeded with and no notes.
    let (status, after) = get(&router, held).await;
    assert_eq!(
        status, 200,
        "the held request must still be readable, got {status}: {after}"
    );
    assert_eq!(
        element(&after, "State"),
        STATE_UNASSIGNED,
        "a refused caller must not move the state, got {:?}",
        element(&after, "State")
    );
    assert!(
        elements(&after, "Notes").is_empty(),
        "a refused caller must not write notes, got {:?}",
        elements(&after, "Notes")
    );
    assert_eq!(
        held_delegation_requests(&engine),
        vec![held.to_string()],
        "a refused update files nothing, got {:?}",
        held_delegation_requests(&engine)
    );
}
