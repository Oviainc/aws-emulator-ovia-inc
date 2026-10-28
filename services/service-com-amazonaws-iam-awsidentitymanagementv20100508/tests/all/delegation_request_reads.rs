// SPDX-License-Identifier: CC0-1.0

//! `GetDelegationRequest`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share.
//!
//! The rules being asserted:
//!
//! * the read reports the delegation request the account HOLDS under the identifier the
//!   request names, member for member, and refuses an identifier it holds none under as
//!   a missing entity;
//! * a permission check is performed only for a request that asks for one, and its
//!   result is what the CALLER's own identity-based policies say about the permissions
//!   that delegation request asks to have delegated — `UNSURE` wherever that cannot be
//!   decided from the documents the account holds;
//! * the read writes nothing.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: the
//! identifier's alphabet and both published length bounds, which members the stored
//! record carries, whether the account holds the request, what the request asks for, and
//! what the caller is permitted.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    DelegationPermission, DelegationRequest, PolicyParameter,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement, UNREADABLE_GRANT};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The permissions a delegation request asks to have delegated, as
/// `DelegationRequest$PermissionPolicy` carries them ("JSON content of the associated
/// permission policy of this delegation request"): TWO asks, so a caller covering one of
/// them is a row distinct from one covering both.
const ASKS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":"arn:aws:s3:::partner-drop/*"}]}"#;

/// The resource both asks of [`ASKS`] are taken on.
const ASKED_RESOURCE: &str = "arn:aws:s3:::partner-drop/*";

/// A one-statement document allowing this operation itself, which every signed row needs
/// before the read it drives reaches the permission check at all: IAM decides the call
/// against the caller's policies first.
fn may_read_delegation_requests() -> String {
    statement("Allow", "iam:GetDelegationRequest", "*")
}

/// The delegation request identifiers the suite spans.
///
/// Source: `delegationRequestIdType` on
/// `smithy_query(query="operation", operation="GetDelegationRequest")` — length 16..128,
/// pattern `^[\w-]+$` — so the rows carry the whole published alphabet (letters in both
/// cases, digits, `_`, `-`) and BOTH published length bounds.
fn delegation_request_ids() -> Vec<String> {
    vec![
        // The published minimum length.
        "0123456789abcdef".to_string(),
        // The published maximum length.
        "p".repeat(128),
        // The whole published alphabet, in a mixed case.
        "Partner_Delegation-7x".to_string(),
        // All digits, with a leading run of zeroes: an identifier is a handle, not a
        // number.
        "00000000000000012345".to_string(),
        // Hyphen-separated, as a minted handle often reads.
        "7a3f-9b1c-4d2e-8c55".to_string(),
    ]
}

/// Seed `request` into the service's OWN store, under the identifier it carries.
///
/// This is the state `CreateDelegationRequest` ("creates an IAM delegation request for
/// temporary access delegation") writes — the suite seeds it directly so a row can fix
/// every member and every instant a record carries, which a filing cannot (it mints the
/// identifier and reads the clock). The record goes into the service's own store rather
/// than into a second copy of it, which is where this read finds it.
pub(crate) fn seed(engine: &engine::InMemoryEngine, request: &DelegationRequest) {
    let store = state::service_state(engine).expect("the IAM store");
    let id = request
        .delegation_request_id
        .clone()
        .expect("a seeded delegation request carries its own identifier");
    store.lock().delegation_requests.insert(id, request.clone());
}

/// The delegation requests the account holds, read straight out of the store: what a
/// reporting operation must leave exactly as it found it.
pub(crate) fn held_delegation_requests(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store.lock().delegation_requests.keys().cloned().collect();
    held
}

/// One `GetDelegationRequest` call, presented as signed by `akid` when a row names one,
/// asking for a permission check when `check` names one.
async fn get(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    delegation_request_id: &str,
    check: Option<bool>,
) -> (u16, String) {
    let asked = check.map(|check| check.to_string());
    let mut params = vec![("DelegationRequestId", delegation_request_id)];
    if let Some(asked) = asked.as_deref() {
        params.push(("DelegationPermissionCheck", asked));
    }
    call_signed_by(router, akid, "GetDelegationRequest", &params).await
}

/// The instants the fixture records carry, as fixed values rather than the clock, so a
/// reported timestamp is comparable with the stored one.
const FIXTURE_CREATED_MILLIS: i64 = 1_767_225_600_000;
const FIXTURE_UPDATED_MILLIS: i64 = 1_767_312_000_000;
const FIXTURE_EXPIRES_MILLIS: i64 = 1_767_830_400_000;

/// The stored records the round-trip suite spans, for one identifier: a request carrying
/// the few members a filed request must have, and one carrying every member
/// `DelegationRequest` declares — so a member reported from the second row's record is
/// absent from the first's.
fn stored_records(delegation_request_id: &str) -> Vec<(&'static str, DelegationRequest)> {
    let sparse = DelegationRequest {
        delegation_request_id: Some(delegation_request_id.to_string()),
        description: Some("temporary access for a partner engagement".to_string()),
        requestor_id: Some(account_id().to_string()),
        // One of the values `stateType` declares
        // (`ops::types::STATE_TYPE_VALUES`); the published minimum
        // `SessionDuration` (range 300..43200).
        state: Some("UNASSIGNED".to_string()),
        session_duration: Some(300),
        create_date: Some(DateTime::from_millis(FIXTURE_CREATED_MILLIS)),
        ..DelegationRequest::default()
    };
    let full = DelegationRequest {
        delegation_request_id: Some(delegation_request_id.to_string()),
        owner_account_id: Some(account_id().to_string()),
        description: Some("full access review for the partner workflow".to_string()),
        request_message: Some("the earlier request was rejected for a missing boundary".to_string()),
        permissions: Some(DelegationPermission {
            policy_template_arn: Some(
                "arn:aws:iam::aws:policy-template/PartnerDropReadWrite".to_string(),
            ),
            parameters: Some(vec![PolicyParameter {
                name: Some("BucketName".to_string()),
                values: Some(vec!["partner-drop".to_string()]),
                // One of the two values `PolicyParameterTypeEnum` declares.
                r#type: Some("string".to_string()),
            }]),
        }),
        permission_policy: Some(ASKS.to_string()),
        role_permission_restriction_arns: None,
        owner_id: Some(format!("arn:aws:iam::{}:user/approver", account_id())),
        approver_id: Some(format!("arn:aws:iam::{}:user/approver", account_id())),
        state: Some("PENDING_APPROVAL".to_string()),
        expiration_time: Some(DateTime::from_millis(FIXTURE_EXPIRES_MILLIS)),
        requestor_id: Some(account_id().to_string()),
        requestor_name: Some("Partner Onboarding".to_string()),
        create_date: Some(DateTime::from_millis(FIXTURE_CREATED_MILLIS)),
        // The published maximum `SessionDuration`.
        session_duration: Some(43_200),
        redirect_url: Some("https://partner.example.com/delegation/return".to_string()),
        notes: Some("approved once the boundary was attached".to_string()),
        rejection_reason: Some("none: the request was approved".to_string()),
        only_send_by_owner: true,
        updated_time: Some(DateTime::from_millis(FIXTURE_UPDATED_MILLIS)),
    };
    vec![
        ("the members a filed request must have", sparse),
        ("every member the shape declares", full),
    ]
}

/// What each text or numeric leaf member of `stored` must be reported as: the value the
/// record carries, and NO element at all where it carries none.
fn expected_members(stored: &DelegationRequest) -> Vec<(&'static str, Vec<String>)> {
    let text = |value: &Option<String>| value.iter().cloned().collect::<Vec<String>>();
    let template = stored
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.policy_template_arn.clone());
    let parameter = stored
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.parameters.as_ref())
        .and_then(|parameters| parameters.first())
        .and_then(|parameter| parameter.name.clone());
    vec![
        ("DelegationRequestId", text(&stored.delegation_request_id)),
        ("OwnerAccountId", text(&stored.owner_account_id)),
        ("Description", text(&stored.description)),
        ("RequestMessage", text(&stored.request_message)),
        ("PermissionPolicy", text(&stored.permission_policy)),
        ("OwnerId", text(&stored.owner_id)),
        ("ApproverId", text(&stored.approver_id)),
        ("State", text(&stored.state)),
        ("RequestorId", text(&stored.requestor_id)),
        ("RequestorName", text(&stored.requestor_name)),
        ("Notes", text(&stored.notes)),
        ("RejectionReason", text(&stored.rejection_reason)),
        ("RedirectUrl", text(&stored.redirect_url)),
        ("PolicyTemplateArn", text(&template)),
        ("Name", text(&parameter)),
        (
            "SessionDuration",
            stored
                .session_duration
                .iter()
                .map(i32::to_string)
                .collect::<Vec<String>>(),
        ),
    ]
}

/// The instant the member `tag` reports, in milliseconds, or `None` when the response
/// carries no such member.
fn reported_instant(case: &str, body: &str, tag: &str) -> Option<i64> {
    let reported = elements(body, tag);
    assert!(
        reported.len() <= 1,
        "case {case:?}: expected at most one <{tag}>, got {reported:?}"
    );
    reported.first().map(|text| {
        let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {case:?}: expected an ISO-8601 {tag}, got {text:?}: {error}")
        });
        parsed
            .to_millis()
            .unwrap_or_else(|error| panic!("case {case:?}: {tag} {text:?}: {error}"))
    })
}

/// RULE: an identifier the account holds no delegation request under references "a
/// resource entity that does not exist", so the read is refused with the modelled
/// `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) whose message
/// describes the resource asked about — whether or not the request asked for a permission
/// check, and without reporting either member that carries one.
#[tokio::test]
async fn an_identifier_the_account_holds_nothing_under_is_refused_as_a_missing_entity() {
    for delegation_request_id in delegation_request_ids() {
        let (router, engine) = router_with_engine();
        // A row where the account holds ANOTHER delegation request, so what is asserted
        // is the lookup of the identifier the request named rather than whether the
        // account holds any request at all.
        let held = DelegationRequest {
            delegation_request_id: Some("held0123456789ab".to_string()),
            permission_policy: Some(ASKS.to_string()),
            ..DelegationRequest::default()
        };
        seed(&engine, &held);

        for check in [None, Some(false), Some(true)] {
            let case = format!("{delegation_request_id} check {check:?}");
            let (status, body) = get(&router, None, &delegation_request_id, check).await;
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
                format!("DelegationRequest not found: {delegation_request_id}"),
                "case {case:?}: the refusal must describe the delegation request asked \
                 about, got {:?}",
                element(&body, "Message")
            );
            for member in ["PermissionCheckStatus", "PermissionCheckResult"] {
                assert!(
                    elements(&body, member).is_empty(),
                    "case {case:?}: a refused read reports no {member}, got {:?}",
                    elements(&body, member)
                );
            }
        }

        // The refused reads wrote nothing: the account holds exactly the request it held
        // before.
        assert_eq!(
            held_delegation_requests(&engine),
            vec!["held0123456789ab".to_string()],
            "case {delegation_request_id:?}: a refused read persists nothing, got {:?}",
            held_delegation_requests(&engine)
        );
    }
}

/// RULE: the delegation request the account HOLDS under the identifier is reported as the
/// account holds it — every member it carries, and no member it does not — and a request
/// that did not ask for a permission check is answered without either member that
/// reports one.
#[tokio::test]
async fn a_held_delegation_request_is_reported_as_the_account_holds_it() {
    for delegation_request_id in delegation_request_ids() {
        for (label, stored) in stored_records(&delegation_request_id) {
            let (router, engine) = router_with_engine();
            seed(&engine, &stored);

            for check in [None, Some(false)] {
                let case = format!("{delegation_request_id} {label} check {check:?}");
                let (status, body) = get(&router, None, &delegation_request_id, check).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status}: {body}"
                );

                for (member, expected) in expected_members(&stored) {
                    assert_eq!(
                        elements(&body, member),
                        expected,
                        "case {case:?}: expected the stored {member}, got {:?}: {body}",
                        elements(&body, member)
                    );
                }
                for (member, expected) in [
                    ("CreateDate", stored.create_date),
                    ("UpdatedTime", stored.updated_time),
                    ("ExpirationTime", stored.expiration_time),
                ] {
                    assert_eq!(
                        reported_instant(&case, &body, member),
                        expected.map(|at| at.to_millis().expect("a representable instant")),
                        "case {case:?}: expected the stored {member}, got {:?}",
                        elements(&body, member)
                    );
                }
                // `OnlySendByOwner` is a boolean whose default is false, so the row that
                // stored `true` must report it and the row that stored false must not
                // report it as anything else.
                let flag = elements(&body, "OnlySendByOwner");
                if stored.only_send_by_owner {
                    assert_eq!(
                        flag,
                        vec!["true".to_string()],
                        "case {case:?}: expected the stored OnlySendByOwner, got {flag:?}"
                    );
                } else {
                    assert!(
                        flag.iter().all(|value| value == "false"),
                        "case {case:?}: the request was not stored as send-by-owner, got \
                         {flag:?}"
                    );
                }
                // No check was asked for, so no check was performed and neither member
                // that reports one is answered.
                for member in ["PermissionCheckStatus", "PermissionCheckResult"] {
                    assert!(
                        elements(&body, member).is_empty(),
                        "case {case:?}: a read that asked for no permission check reports \
                         no {member}, got {:?}",
                        elements(&body, member)
                    );
                }
            }

            // Reading a request neither created, moved nor dropped one.
            assert_eq!(
                held_delegation_requests(&engine),
                vec![delegation_request_id.clone()],
                "case {label:?}: a read persists nothing, got {:?}",
                held_delegation_requests(&engine)
            );
        }
    }
}

/// One row of the permission-check table: the case it is, the inline policies the
/// calling user holds, and the result the check must reach for them.
type CheckRow = (&'static str, Vec<(String, String)>, &'static str);

/// RULE: a request that asks for a permission check is answered with the check's status
/// and, with it, the result the CALLER's own identity-based policies give for the
/// permissions the delegation request asks to have delegated: allowed when they cover
/// every ask, denied when they do not or when one is explicitly denied.
#[tokio::test]
async fn a_permission_check_answers_from_the_callers_own_policies() {
    // Rows: a caller covering both asks exactly, one covering them through IAM's
    // wildcards, one covering only one of the two, one covering neither, and one covering
    // both with an explicit `Deny` on one — plus the caller whose grants are conditioned,
    // which cannot be decided here at all.
    let rows: Vec<CheckRow> = vec![
        (
            "both asks, named exactly",
            vec![
                ("read-delegation".to_string(), may_read_delegation_requests()),
                (
                    "cover-both".to_string(),
                    format!(
                        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":"{ASKED_RESOURCE}"}}]}}"#
                    ),
                ),
            ],
            "ALLOWED",
        ),
        (
            "both asks, through wildcards",
            vec![
                ("read-delegation".to_string(), may_read_delegation_requests()),
                ("cover-all".to_string(), statement("Allow", "s3:*", "*")),
            ],
            "ALLOWED",
        ),
        (
            "one of the two asks",
            vec![
                ("read-delegation".to_string(), may_read_delegation_requests()),
                (
                    "cover-one".to_string(),
                    statement("Allow", "s3:GetObject", ASKED_RESOURCE),
                ),
            ],
            "DENIED",
        ),
        (
            "neither ask",
            vec![("read-delegation".to_string(), may_read_delegation_requests())],
            "DENIED",
        ),
        (
            "both asks, one explicitly denied",
            vec![
                ("read-delegation".to_string(), may_read_delegation_requests()),
                ("cover-all".to_string(), statement("Allow", "s3:*", "*")),
                (
                    "deny-one".to_string(),
                    statement("Deny", "s3:PutObject", ASKED_RESOURCE),
                ),
            ],
            "DENIED",
        ),
        (
            "both asks, under a condition",
            vec![
                ("read-delegation".to_string(), may_read_delegation_requests()),
                (
                    "cover-both-conditioned".to_string(),
                    format!(
                        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":"{ASKED_RESOURCE}","Condition":{{"StringEquals":{{"aws:RequestedRegion":"us-east-1"}}}}}}]}}"#
                    ),
                ),
            ],
            "UNSURE",
        ),
    ];

    for (row, (case, policies, expected)) in rows.into_iter().enumerate() {
        let (router, engine) = router_with_engine();
        let delegation_request_id = format!("permissioncheck{row:0>3}");
        let stored = DelegationRequest {
            delegation_request_id: Some(delegation_request_id.clone()),
            permission_policy: Some(ASKS.to_string()),
            requestor_id: Some(account_id().to_string()),
            state: Some("ASSIGNED".to_string()),
            ..DelegationRequest::default()
        };
        seed(&engine, &stored);

        let user_name = format!("checker{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let (status, body) = get(
            &router,
            Some(&access_key_id),
            &delegation_request_id,
            Some(true),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to be served, got {status}: {body}"
        );
        // The check this emulator performs is a decision over the documents in force at
        // the instant of the call, so it has already answered by the time the response is
        // written.
        assert_eq!(
            element(&body, "PermissionCheckStatus"),
            "COMPLETE",
            "case {case:?}: expected a completed check, got {:?}: {body}",
            element(&body, "PermissionCheckStatus")
        );
        assert_eq!(
            element(&body, "PermissionCheckResult"),
            expected,
            "case {case:?}: expected {expected}, got {:?}: {body}",
            element(&body, "PermissionCheckResult")
        );
        // The request itself is reported beside the check, and the check left nothing
        // behind.
        assert_eq!(
            element(&body, "DelegationRequestId"),
            delegation_request_id,
            "case {case:?}: the check must not displace the request itself: {body}"
        );
        assert_eq!(
            held_delegation_requests(&engine),
            vec![delegation_request_id.clone()],
            "case {case:?}: a permission check persists nothing, got {:?}",
            held_delegation_requests(&engine)
        );
    }
}

/// RULE: where the evidence a permission check needs cannot be read — asks that do not
/// reduce to concrete permissions, or a caller holding a grant whose document the account
/// does not hold — the check reports that it is not possible to determine whether the
/// caller has all the permissions needed, rather than refusing the asks.
#[tokio::test]
async fn a_permission_check_that_cannot_be_decided_is_unsure() {
    // Rows: the asks a request carries, each one the reading cannot reduce to concrete
    // `(action, resource)` permissions.
    let undecidable_asks: Vec<(&str, Option<String>)> = vec![
        ("no permission policy at all", None),
        (
            "a permission policy that is not a policy document",
            Some("not a policy at all".to_string()),
        ),
        (
            "asks written as a complement",
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":"iam:*","Resource":"*"}]}"#
                    .to_string(),
            ),
        ),
        (
            "asks narrowed by a condition",
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*","Condition":{"Bool":{"aws:SecureTransport":"true"}}}]}"#
                    .to_string(),
            ),
        ),
        (
            "asks that grant nothing",
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"s3:GetObject","Resource":"*"}]}"#
                    .to_string(),
            ),
        ),
    ];

    for (row, (case, permission_policy)) in undecidable_asks.into_iter().enumerate() {
        let (router, engine) = router_with_engine();
        let delegation_request_id = format!("undecidableasks{row:0>3}");
        seed(
            &engine,
            &DelegationRequest {
                delegation_request_id: Some(delegation_request_id.clone()),
                permission_policy,
                ..DelegationRequest::default()
            },
        );

        // A caller that covers every permission it could possibly be asked for: what the
        // row turns on is the asks, not the caller.
        let access_key_id = principal(
            &router,
            case,
            &format!("undecidable{row}"),
            &[("allow-everything".to_string(), statement("Allow", "*", "*"))],
        )
        .await;
        let (status, body) = get(
            &router,
            Some(&access_key_id),
            &delegation_request_id,
            Some(true),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to be served, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "PermissionCheckResult"),
            "UNSURE",
            "case {case:?}: expected UNSURE, got {:?}: {body}",
            element(&body, "PermissionCheckResult")
        );
    }

    // And the same answer for a caller holding a grant this emulator cannot read: what it
    // grants is unknown here, so whether the caller covers the asks is unknown too.
    let (router, engine) = router_with_engine();
    let delegation_request_id = "unreadablegrant01";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(delegation_request_id.to_string()),
            permission_policy: Some(ASKS.to_string()),
            ..DelegationRequest::default()
        },
    );
    let user_name = "unreadable-checker";
    let access_key_id = principal(
        &router,
        "a grant the emulator cannot read",
        user_name,
        &[("read-delegation".to_string(), may_read_delegation_requests())],
    )
    .await;
    let (status, attached) = call(
        &router,
        "AttachUserPolicy",
        &[("UserName", user_name), ("PolicyArn", UNREADABLE_GRANT)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture AttachUserPolicy must succeed, got {status}: {attached}"
    );
    let (status, body) = get(
        &router,
        Some(&access_key_id),
        delegation_request_id,
        Some(true),
    )
    .await;
    assert_eq!(
        status, 200,
        "an unreadable grant must not refuse the read, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "PermissionCheckResult"),
        "UNSURE",
        "a grant this emulator cannot read leaves the check undecided, got {:?}: {body}",
        element(&body, "PermissionCheckResult")
    );

    // A credential no IAM user owns carries no identity-based policies at all — the
    // position this account's root credentials are in — so there is nothing its
    // permissions can fall short of.
    let (status, body) = get(&router, None, delegation_request_id, Some(true)).await;
    assert_eq!(
        status, 200,
        "an unsigned read must be served, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "PermissionCheckResult"),
        "ALLOWED",
        "a credential no IAM user owns covers every ask, got {:?}: {body}",
        element(&body, "PermissionCheckResult")
    );
}

/// RULE: the call is decided against the caller's identity-based policies before the
/// lookup answers, so a caller those policies do not permit is refused HTTP 403
/// `AccessDenied` — naming the delegation request it asked about — whether or not the
/// account holds that request.
#[tokio::test]
async fn a_caller_whose_policies_do_not_permit_the_read_is_refused() {
    let (router, engine) = router_with_engine();
    let held = "accessdenied01234";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(held.to_string()),
            permission_policy: Some(ASKS.to_string()),
            ..DelegationRequest::default()
        },
    );

    // Rows: the two ways IAM refuses a call — an explicit `Deny` about it, and no `Allow`
    // for it at all — each asked about a request the account holds and one it does not.
    for (row, (case, policies, reason)) in [
        (
            "an explicit deny",
            vec![
                ("read-delegation".to_string(), may_read_delegation_requests()),
                (
                    "deny-delegation".to_string(),
                    statement("Deny", "iam:GetDelegationRequest", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:ListUsers", "*"),
            )],
            "because no identity-based policy allows the iam:GetDelegationRequest action"
                .to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let user_name = format!("refused{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        for delegation_request_id in [held, "neverfiled0123456"] {
            let case = format!("{case} asking about {delegation_request_id}");
            let (status, body) =
                get(&router, Some(&access_key_id), delegation_request_id, None).await;
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
                "iam:GetDelegationRequest",
                &format!("delegation request {delegation_request_id}"),
                &reason,
            );
            // A caller that may not read this request learns that it may not, never
            // whether the request exists.
            assert!(
                elements(&body, "DelegationRequestId").is_empty(),
                "case {case:?}: a refused read reports no request, got {:?}",
                elements(&body, "DelegationRequestId")
            );
        }
    }
}

/// RULE: a reporting operation changes nothing a later caller sees, however many times it
/// is called — neither the requests the account holds nor the answer the next identical
/// call gets, with a permission check or without one.
#[tokio::test]
async fn reading_a_delegation_request_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let held = "repeatedreads0123";
    let stored = DelegationRequest {
        delegation_request_id: Some(held.to_string()),
        permission_policy: Some(ASKS.to_string()),
        state: Some("PENDING_APPROVAL".to_string()),
        session_duration: Some(3_600),
        create_date: Some(DateTime::from_millis(FIXTURE_CREATED_MILLIS)),
        ..DelegationRequest::default()
    };
    seed(&engine, &stored);

    // Rows: every branch the read takes — the held request with and without a check, and
    // an identifier the account holds nothing under. The `RequestId` of the response
    // envelope is per-call, so what is compared is every member the read itself answers
    // with.
    for (delegation_request_id, check) in [
        (held, None),
        (held, Some(true)),
        ("neverfiled0123456", Some(true)),
    ] {
        let case = format!("{delegation_request_id} check {check:?}");
        let mut answers: Vec<(u16, Vec<String>)> = Vec::new();
        for read in 0..3 {
            let (status, body) = get(&router, None, delegation_request_id, check).await;
            let answered: Vec<String> = [
                "DelegationRequestId",
                "State",
                "SessionDuration",
                "CreateDate",
                "PermissionPolicy",
                "PermissionCheckStatus",
                "PermissionCheckResult",
                "Code",
                "Message",
            ]
            .iter()
            .flat_map(|member| elements(&body, member))
            .collect();
            answers.push((status, answered));
            assert_eq!(
                answers[0], answers[read],
                "case {case:?}: read {read} answered {:?} where the first answered {:?}",
                answers[read], answers[0]
            );
        }
    }

    // And the store still holds exactly the one request that was seeded, unchanged.
    assert_eq!(
        held_delegation_requests(&engine),
        vec![held.to_string()],
        "a read persists nothing, got {:?}",
        held_delegation_requests(&engine)
    );
}
