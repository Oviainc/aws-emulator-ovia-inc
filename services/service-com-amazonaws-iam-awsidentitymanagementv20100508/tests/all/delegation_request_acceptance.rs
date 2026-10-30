// SPDX-License-Identifier: CC0-1.0

//! `AcceptDelegationRequest`, end to end over the awsQuery wire.
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
//! * for a request the account HOLDS, the acceptance takes it to the `ACCEPTED` state and
//!   records the identity that called the operation, which is what the read of that
//!   identifier reports afterwards, leaving every member the acceptance is not about as
//!   the account held it;
//! * a caller whose identity-based policies do not allow the action is refused, and a
//!   refused call writes nothing.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: the
//! identifier's alphabet and both published length bounds, the state the request was in
//! before the call — every value `stateType` declares — which identity drives the call,
//! and what that caller is permitted.

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

/// The published bounds of `delegationRequestIdType`, the type the identifier is carried
/// through: "len 16..128, pattern `^[\w-]+$`"
/// (`smithy_query(query="operation", operation="AcceptDelegationRequest")`).
const MIN_ID_CHARACTERS: usize = 16;
const MAX_ID_CHARACTERS: usize = 128;

/// The state a request the account has just had filed for it carries when the filing named
/// no target account: "a request that can be associated by any account"
/// (`CreateDelegationRequest$OwnerAccountId`), one of the values `stateType` declares.
const STATE_UNASSIGNED: &str = "UNASSIGNED";

/// The state this operation takes a delegation request to: "accepts a delegation request,
/// granting the requested temporary access. Once the delegation request is accepted, it is
/// eligible to send the exchange token to the partner"
/// (`smithy_query(query="doc", shape="AcceptDelegationRequest")`), one of the values
/// `stateType` declares.
const STATE_ACCEPTED: &str = "ACCEPTED";

/// The delegation request identifiers the not-found suite spans.
///
/// Source: `delegationRequestIdType` on `smithy_query(query="operation",
/// operation="AcceptDelegationRequest")` — length 16..128, pattern `^[\w-]+$` — so the
/// rows carry the whole published alphabet (letters in both cases, digits, `_`, `-`) and
/// BOTH published length bounds.
fn delegation_request_ids() -> Vec<String> {
    vec![
        // The published minimum length.
        "abcdef0123456789".to_string(),
        // The published maximum length.
        "z".repeat(MAX_ID_CHARACTERS),
        // The whole published alphabet, in a mixed case.
        "Partner_Delegation-4k".to_string(),
        // All digits, with a leading run of zeroes: an identifier is a handle, not a
        // number.
        "00000000000000055555".to_string(),
        // Hyphen-separated, as a minted handle often reads.
        "1b2c-3d4e-5f60-7a89".to_string(),
    ]
}

/// One `AcceptDelegationRequest` call, presented as signed by `akid` when a row names one.
async fn accept(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    delegation_request_id: &str,
) -> (u16, String) {
    call_signed_by(
        router,
        akid,
        "AcceptDelegationRequest",
        &[("DelegationRequestId", delegation_request_id)],
    )
    .await
}

/// One `GetDelegationRequest` call for `delegation_request_id`: the implemented read the
/// round trip is proved through.
async fn get(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    delegation_request_id: &str,
) -> (u16, String) {
    call_signed_by(
        router,
        akid,
        "GetDelegationRequest",
        &[("DelegationRequestId", delegation_request_id)],
    )
    .await
}

/// File a delegation request under the workflow id `workflow`, targeted at `owner` when a
/// row names one, and answer the identifier IAM minted for it: the fixture every round-trip
/// row starts from, driven through the implemented `CreateDelegationRequest` rather than
/// seeded, so what is accepted is a record this emulator wrote.
async fn file(
    router: &MultiServiceRouter,
    case: &str,
    workflow: &str,
    owner: Option<&str>,
) -> String {
    let mut params = vec![
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
    ];
    if let Some(owner) = owner {
        params.push(("OwnerAccountId", owner));
    }
    let (status, body) = call(router, "CreateDelegationRequest", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateDelegationRequest must succeed, got {status}: {body}"
    );
    element(&body, "DelegationRequestId")
}

/// The members this operation does NOT write, which the read must report unchanged across
/// an acceptance: everything the filing stored but the state, the approving identity and
/// the last-updated instant.
const UNTOUCHED_MEMBERS: &[&str] = &[
    "DelegationRequestId",
    "OwnerAccountId",
    "Description",
    "RequestMessage",
    "PermissionPolicy",
    "OwnerId",
    "RequestorId",
    "RequestorName",
    "Notes",
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
/// resource entity that does not exist", so the acceptance is refused with the modelled
/// `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) whose message describes
/// the request asked about, and the refusal files nothing under that identifier, so the
/// read of it is refused the same way afterwards.
#[tokio::test]
async fn an_identifier_the_account_holds_nothing_under_is_refused_as_a_missing_entity() {
    for delegation_request_id in delegation_request_ids() {
        let (router, engine) = router_with_engine();
        // A row where the account holds ANOTHER delegation request, so what is asserted is
        // the lookup of the identifier the request named rather than whether the account
        // holds any request at all.
        let held = DelegationRequest {
            delegation_request_id: Some("held1234567890ab".to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        };
        seed(&engine, &held);

        let (status, body) = accept(&router, None, &delegation_request_id).await;
        assert_eq!(
            status, 404,
            "case {delegation_request_id:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {delegation_request_id:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("DelegationRequest not found: {delegation_request_id}"),
            "case {delegation_request_id:?}: the refusal must describe the delegation \
             request asked about, got {:?}",
            element(&body, "Message")
        );

        // The refused acceptance filed nothing: the account holds exactly the request it
        // held before, and the read of the refused identifier is still a missing entity.
        assert_eq!(
            held_delegation_requests(&engine),
            vec!["held1234567890ab".to_string()],
            "case {delegation_request_id:?}: a refused acceptance persists nothing, got {:?}",
            held_delegation_requests(&engine)
        );
        let (status, body) = get(&router, None, &delegation_request_id).await;
        assert_eq!(
            status, 404,
            "case {delegation_request_id:?}: the read of the refused identifier must still \
             be refused, got {status}: {body}"
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

/// RULE: a delegation request the identifier is held under is case-SENSITIVE — the
/// identifier is a minted handle rather than an IAM entity name, so a spelling that
/// differs only in case names no request the account holds and is refused as a missing
/// entity.
#[tokio::test]
async fn an_identifier_differing_only_in_case_is_a_missing_entity() {
    let (router, engine) = router_with_engine();
    let held = "CasedHandle-0123AB";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(held.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    for case in [held.to_lowercase(), held.to_uppercase()] {
        let (status, body) = accept(&router, None, &case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("DelegationRequest not found: {case}"),
            "case {case:?}: the refusal must quote the identifier the request carried, \
             got {:?}",
            element(&body, "Message")
        );
    }

    // The identifier the account DOES hold is served, and it is the only one.
    let (status, body) = accept(&router, None, held).await;
    assert_eq!(
        status, 200,
        "case {held:?}: expected the acceptance to be served, got {status}: {body}"
    );
}

/// RULE: for a request the account holds, the acceptance takes it to the `ACCEPTED` state —
/// which is what the read of that identifier reports afterwards, with the last-updated
/// instant moved and every member the acceptance is not about left as the account held it.
#[tokio::test]
async fn an_acceptance_is_what_the_read_reports() {
    let router = router();

    // Rows: a request filed with no target account and one filed targeted at this account,
    // the two filings `CreateDelegationRequest` distinguishes, so the acceptance is
    // asserted over both states a freshly filed request can be in.
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("a request targeted at no account", None, "UNASSIGNED"),
        (
            "a request targeted at this account",
            Some(account_id()),
            "ASSIGNED",
        ),
    ];

    for (row, (label, owner, before_state)) in rows.into_iter().enumerate() {
        let case = format!("accepting {label}");
        let delegation_request_id =
            file(&router, &case, &format!("workflow-accept-{row}"), owner).await;

        let (status, before) = get(&router, None, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the filed request must be readable, got {status}: {before}"
        );
        assert_eq!(
            element(&before, "State"),
            before_state,
            "case {case:?}: expected the filed request in {before_state}, got {:?}",
            element(&before, "State")
        );
        let filed_at = reported_instant(&case, &before, "CreateDate")
            .unwrap_or_else(|| panic!("case {case:?}: the filed request must report a CreateDate"));

        let (status, body) = accept(&router, None, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the acceptance to be served, got {status}: {body}"
        );

        let (status, after) = get(&router, None, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the accepted request must be readable, got {status}: {after}"
        );
        assert_eq!(
            element(&after, "State"),
            STATE_ACCEPTED,
            "case {case:?}: an accepted request reaches ACCEPTED, got {:?}",
            element(&after, "State")
        );
        let accepted_at = reported_instant(&case, &after, "UpdatedTime").unwrap_or_else(|| {
            panic!("case {case:?}: an accepted request reports an UpdatedTime")
        });
        assert!(
            accepted_at >= filed_at,
            "case {case:?}: expected the last-updated instant at or after the filing's \
             {filed_at}, got {accepted_at}"
        );
        // An unsigned call carries no access key the account holds an IAM user for, so
        // there is no identity to record as the approver.
        assert!(
            elements(&after, "ApproverId").is_empty(),
            "case {case:?}: a call by a credential that names no IAM user records no \
             approving identity, got {:?}",
            elements(&after, "ApproverId")
        );
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                elements(&after, member),
                elements(&before, member),
                "case {case:?}: the acceptance must leave {member} as the account held it, \
                 got {:?} after {:?}",
                elements(&after, member),
                elements(&before, member)
            );
        }
    }
}

/// RULE: the acceptance is what takes a delegation request to `ACCEPTED`, whatever state it
/// was in before — so the rows are every value `stateType` declares, read from the model's
/// own value set (`ops::types::STATE_TYPE_VALUES`) rather than from the states one workflow
/// happens to pass through.
#[tokio::test]
async fn an_acceptance_takes_a_request_in_any_state_to_accepted() {
    for (row, state) in STATE_TYPE_VALUES.iter().enumerate() {
        let (router, engine) = router_with_engine();
        let delegation_request_id = format!("acceptfromstate{row:0>3}");
        seed(
            &engine,
            &DelegationRequest {
                delegation_request_id: Some(delegation_request_id.clone()),
                description: Some("a request in a state the acceptance moves".to_string()),
                requestor_id: Some(account_id().to_string()),
                state: Some((*state).to_string()),
                notes: Some("the notes the request already carried".to_string()),
                ..DelegationRequest::default()
            },
        );

        let case = format!("a request in {state}");
        let (status, body) = accept(&router, None, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the acceptance to be served, got {status}: {body}"
        );

        let (status, after) = get(&router, None, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the accepted request must be readable, got {status}: {after}"
        );
        assert_eq!(
            element(&after, "State"),
            STATE_ACCEPTED,
            "case {case:?}: an accepted request reaches ACCEPTED, got {:?}",
            element(&after, "State")
        );
        assert_eq!(
            elements(&after, "Notes"),
            vec!["the notes the request already carried".to_string()],
            "case {case:?}: the acceptance must leave the notes alone, got {:?}",
            elements(&after, "Notes")
        );
        assert_eq!(
            element(&after, "Description"),
            "a request in a state the acceptance moves",
            "case {case:?}: the acceptance must leave the description alone, got {:?}",
            element(&after, "Description")
        );
    }
}

/// RULE: "at the time of acceptance, IAM records the details and the state of the identity
/// that called this API", so the identity the record reports as the approver is the CALLER
/// of the acceptance — a different caller records a different identity, and the record
/// keeps the one the last acceptance was made by.
#[tokio::test]
async fn the_accepting_identity_is_what_the_record_reports() {
    let router = router();
    let grants = vec![
        (
            "accept".to_string(),
            statement("Allow", "iam:AcceptDelegationRequest", "*"),
        ),
        (
            "read".to_string(),
            statement("Allow", "iam:GetDelegationRequest", "*"),
        ),
    ];

    // Rows: two distinct principals accepting the SAME request one after the other, so
    // what is asserted is that the recorded identity follows the caller rather than being
    // one fixed value.
    let delegation_request_id = file(&router, "recorded approver", "workflow-approver", None).await;
    for user_name in ["approver-one", "approver-two"] {
        let access_key_id = principal(&router, user_name, user_name, &grants).await;
        let (status, body) = accept(&router, Some(&access_key_id), &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: expected the acceptance to be served, got {status}: {body}"
        );

        let (status, after) = get(&router, Some(&access_key_id), &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: the accepted request must be readable, got {status}: {after}"
        );
        assert_eq!(
            element(&after, "ApproverId"),
            format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            "case {user_name:?}: expected the calling identity as the approver, got {:?}",
            element(&after, "ApproverId")
        );
        assert_eq!(
            element(&after, "State"),
            STATE_ACCEPTED,
            "case {user_name:?}: an accepted request reaches ACCEPTED, got {:?}",
            element(&after, "State")
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
    let delegation_request_id = "bounded012345678901";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(delegation_request_id.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    let short_id = "s".repeat(MIN_ID_CHARACTERS - 1);
    let long_id = "l".repeat(MAX_ID_CHARACTERS + 1);
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "an identifier one character under the published minimum",
            short_id.as_str(),
            format!(
                "1 validation error detected: Value at 'delegationRequestId' failed to \
                 satisfy constraint: Member must have length greater than or equal to \
                 {MIN_ID_CHARACTERS}"
            ),
        ),
        (
            "an identifier one character over the published maximum",
            long_id.as_str(),
            format!(
                "1 validation error detected: Value at 'delegationRequestId' failed to \
                 satisfy constraint: Member must have length less than or equal to \
                 {MAX_ID_CHARACTERS}"
            ),
        ),
    ];

    for (case, id, expected) in rows {
        let (status, body) = accept(&router, None, id).await;
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
    let (status, after) = get(&router, None, delegation_request_id).await;
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
}

/// RULE: IAM decides the call against the caller's identity-based policies before it
/// touches the request, so a caller no policy allows the action for is refused with HTTP
/// 403 `AccessDenied` naming the principal, the action and the request — whether or not the
/// account holds that request — and the refused call writes nothing.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_call_is_refused() {
    let (router, engine) = router_with_engine();
    let held = "accessdenied98765";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(held.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    // Rows: a caller whose only grant is for ANOTHER action, and one explicitly denied this
    // action by a second statement.
    let rows = vec![
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:GetDelegationRequest", "*"),
            )],
            "because no identity-based policy allows the iam:AcceptDelegationRequest action"
                .to_string(),
        ),
        (
            "an explicit deny over the allow",
            vec![
                (
                    "allow".to_string(),
                    statement("Allow", "iam:AcceptDelegationRequest", "*"),
                ),
                (
                    "deny".to_string(),
                    statement("Deny", "iam:AcceptDelegationRequest", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
    ];

    for (row, (case, policies, reason)) in rows.into_iter().enumerate() {
        let user_name = format!("accepter{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        for delegation_request_id in [held, "neverfiled9876543"] {
            let case = format!("{case} accepting {delegation_request_id}");
            let (status, body) = accept(&router, Some(&access_key_id), delegation_request_id).await;
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
                "iam:AcceptDelegationRequest",
                &format!("delegation request {delegation_request_id}"),
                &reason,
            );
        }
    }

    // A refused caller changed nothing: the request the account holds carries the state it
    // was seeded with, no approving identity, and it is still the only request held.
    let (status, after) = get(&router, None, held).await;
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
        elements(&after, "ApproverId").is_empty(),
        "a refused caller must not record an approving identity, got {:?}",
        elements(&after, "ApproverId")
    );
    assert_eq!(
        held_delegation_requests(&engine),
        vec![held.to_string()],
        "a refused acceptance files nothing, got {:?}",
        held_delegation_requests(&engine)
    );
}
