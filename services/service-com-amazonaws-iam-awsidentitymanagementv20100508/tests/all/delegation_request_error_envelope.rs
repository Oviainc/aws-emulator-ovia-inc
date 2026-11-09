// SPDX-License-Identifier: CC0-1.0

//! The `<RequestId>` an awsQuery error document carries, held across the DELEGATION-REQUEST
//! operation family, end to end over the wire.
//!
//! RULE under test: an awsQuery error response is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId>…</RequestId></ErrorResponse>`
//! — the `<RequestId>` is a SIBLING of `<Error>` and a property of the RESPONSE rather than
//! of any error shape, so it is present, exactly once and after `</Error>`, on every failure
//! this service answers, whatever the operation, the HTTP status or the error code. The id's
//! VALUE is minted per request, so it is not part of the requirement: what a row reads back
//! is its presence, its position, its 8-4-4-4-12 form and that no two calls share one.
//!
//! `tests/all/error_envelope_request_id.rs` holds the same rule along the LAYER axis (a
//! handler's modelled error, a refusal the centralized walk raised, the scaffold's 501, a
//! request naming no modelled action) and owns the form check [`is_request_id`] this suite
//! shares; `tests/all/error_request_id_across_operations.rs` holds it across the user,
//! group, role, profile, certificate and policy families. Neither spans the
//! delegation-request family, whose operations carry the account's delegation workflow —
//! which is the axis here: a per-operation regression on that family would show up as one
//! row failing while both of those suites still pass.
//!
//! The rows differ along what the rule generalises over: the operation called (six of
//! them), whether the identifier addresses a record the account holds, the HTTP status (400,
//! 403, 404) and the error code (`ValidationError`, `AccessDenied`, `NoSuchEntity`) — and
//! whether the failure was decided by the centralized walk, by IAM's own authorization of
//! its API, or by a handler's lookup. No single row is the specification.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is what a
//! client on the socket sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns and every suite in this crate shares, and the signing principal and its policy
//! documents are `tests/all/identity_policy_enforcement.rs`'s.

use std::collections::BTreeSet;

use crate::error_envelope_request_id::is_request_id;
use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// An identifier inside `delegationRequestIdType`'s published bounds (length 16..128,
/// pattern `^[\w-]+$`, from `smithy_query(query="operation",
/// operation="AcceptDelegationRequest")`) that nothing was ever filed under, so every
/// handler row resolves no record.
const NEVER_FILED: &str = "neverfiled-0123456789";

/// The published minimum length of `delegationRequestIdType`, so one character under it is
/// what the centralized walk refuses before any handler runs.
const MIN_ID_CHARACTERS: usize = 16;

/// The published minimum of `CreateDelegationRequest$SessionDuration` (range 300..43200),
/// so one second under it is outside the bound.
const MIN_SESSION_DURATION: u32 = 300;

/// The published maximum of `ListDelegationRequests$MaxItems` (range 1..1000), so one above
/// it is outside the bound.
const MAX_PAGE_SIZE: u32 = 1000;

/// One row: the call to make, who signs it, and the status and error code the operation
/// answers it with.
struct Case {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    /// `false` for a call the emulator's own credentials sign, `true` for one presented as
    /// signed by the fixture principal that is allowed nothing.
    signed: bool,
    status: u16,
    code: &'static str,
}

/// The filing parameters a `CreateDelegationRequest` row needs, with `SessionDuration` set
/// to `duration` — every other member inside its published bounds, so only the duration can
/// decide the call.
fn filing(duration: u32) -> Vec<(String, String)> {
    vec![
        param("Description", "temporary access for a partner engagement"),
        param(
            "NotificationChannel",
            "arn:aws:sns:us-east-1:123456789012:partner-updates",
        ),
        param(
            "Permissions.PolicyTemplateArn",
            "arn:aws:iam::aws:policy-template/PartnerDropReadWrite",
        ),
        param("RequestorWorkflowId", "workflow-envelope"),
        param("SessionDuration", &duration.to_string()),
    ]
}

/// The rows: one per delegation-request operation and error class this service answers.
fn rows() -> Vec<Case> {
    vec![
        // 404 `NoSuchEntity`, reached from four different handlers' lookups.
        Case {
            case: "an acceptance naming an identifier the account holds nothing under",
            action: "AcceptDelegationRequest",
            params: vec![param("DelegationRequestId", NEVER_FILED)],
            signed: false,
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "a read naming an identifier the account holds nothing under",
            action: "GetDelegationRequest",
            params: vec![param("DelegationRequestId", NEVER_FILED)],
            signed: false,
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "an update naming an identifier the account holds nothing under",
            action: "UpdateDelegationRequest",
            params: vec![
                param("DelegationRequestId", NEVER_FILED),
                param("Notes", "notes the refused call must not record"),
            ],
            signed: false,
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "an association naming an identifier the account holds nothing under",
            action: "AssociateDelegationRequest",
            params: vec![param("DelegationRequestId", NEVER_FILED)],
            signed: false,
            status: 404,
            code: "NoSuchEntity",
        },
        // 400 `ValidationError`, decided by the centralized walk before any handler ran, on
        // a length bound and on a range bound.
        Case {
            case: "an acceptance whose identifier is one character under the published \
                   minimum",
            action: "AcceptDelegationRequest",
            params: vec![param(
                "DelegationRequestId",
                &"s".repeat(MIN_ID_CHARACTERS - 1),
            )],
            signed: false,
            status: 400,
            code: "ValidationError",
        },
        Case {
            case: "a filing whose session duration is one second under the published minimum",
            action: "CreateDelegationRequest",
            params: filing(MIN_SESSION_DURATION - 1),
            signed: false,
            status: 400,
            code: "ValidationError",
        },
        Case {
            case: "a listing whose page size is one above the published maximum",
            action: "ListDelegationRequests",
            params: vec![param("MaxItems", &(MAX_PAGE_SIZE + 1).to_string())],
            signed: false,
            status: 400,
            code: "ValidationError",
        },
        // 403 `AccessDenied`, decided by IAM's authorization of its own API before the
        // handler's lookup.
        Case {
            case: "an acceptance by a caller no identity-based policy allows the action for",
            action: "AcceptDelegationRequest",
            params: vec![param("DelegationRequestId", NEVER_FILED)],
            signed: true,
            status: 403,
            code: "AccessDenied",
        },
        Case {
            case: "a read by a caller no identity-based policy allows the action for",
            action: "GetDelegationRequest",
            params: vec![param("DelegationRequestId", NEVER_FILED)],
            signed: true,
            status: 403,
            code: "AccessDenied",
        },
    ]
}

/// RULE: every awsQuery error document a delegation-request operation answers carries one
/// `<RequestId>` beside its `<Error>`, whatever the operation, the status or the code — and
/// a distinct one per call.
#[tokio::test]
async fn every_delegation_request_error_document_names_the_request_it_answers() {
    let router = router();
    // A principal holding one grant for an unrelated action, so the rows it signs are
    // refused by the authorization decision rather than by a credential failure.
    let allowed_nothing = principal(
        &router,
        "a caller allowed only an unrelated action",
        "delegation-envelope-caller",
        &[(
            "unrelated".to_string(),
            statement("Allow", "iam:ListUsers", "*"),
        )],
    )
    .await;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for Case {
        case,
        action,
        params,
        signed,
        status: expected_status,
        code,
    } in rows()
    {
        let akid = signed.then_some(allowed_nothing.as_str());
        let (status, body) = call_signed_by(&router, akid, action, &borrowed(&params)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({action}): expected HTTP {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?} ({action}): expected the code {code:?}, got {body}"
        );
        // The `<Error>` block itself is unchanged by the envelope: the sender
        // classification, the code and a message are all still inside it.
        assert_eq!(
            element(&body, "Type"),
            "Sender",
            "case {case:?} ({action}): expected a client failure classified Sender, got {body}"
        );
        assert!(
            !element(&body, "Message").is_empty(),
            "case {case:?} ({action}): expected the error to describe itself, got {body}"
        );
        // `element` is itself the "exactly one" assertion: an error envelope names the
        // request once, never twice.
        let request_id = element(&body, "RequestId");
        assert!(
            is_request_id(&request_id),
            "case {case:?} ({action}): expected the document to name the request, got \
             {request_id:?} in {body}"
        );
        let error_end = body
            .find("</Error>")
            .unwrap_or_else(|| panic!("case {case:?} ({action}): no <Error> element in {body}"));
        let named = body.find("<RequestId>").unwrap_or_else(|| {
            panic!("case {case:?} ({action}): no <RequestId> element in {body}")
        });
        assert!(
            named > error_end,
            "case {case:?} ({action}): expected <RequestId> beside <Error> rather than inside \
             it, got {body}"
        );
        // `seen.insert` is itself the freshness assertion: an id answered twice fails the
        // row it was answered to.
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?} ({action}): expected a fresh id per call, got {request_id:?} again"
        );
    }
}

/// RULE: the id names the RESPONSE, not the request's content — so the same refused call
/// made again is answered with another id, and a SUCCESSFUL delegation-request call names
/// the request exactly once, in its own `<ResponseMetadata>`, with no second element added.
#[tokio::test]
async fn each_answer_on_the_family_names_its_own_request() {
    let router = router();

    // The same refused call, three times: three ids.
    let mut refused = Vec::new();
    for attempt in 0..3 {
        let (status, body) = call(
            &router,
            "AcceptDelegationRequest",
            &[("DelegationRequestId", NEVER_FILED)],
        )
        .await;
        assert_eq!(
            status, 404,
            "attempt {attempt}: expected HTTP 404, got {status}: {body}"
        );
        refused.push(element(&body, "RequestId"));
    }
    let distinct: BTreeSet<&String> = refused.iter().collect();
    assert_eq!(
        distinct.len(),
        refused.len(),
        "expected one id per call, got {refused:?}"
    );

    // A served call on the same family: one id, and it is the success envelope's own.
    let (status, body) = call(&router, "ListDelegationRequests", &[]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {body}");
    let named = elements(&body, "RequestId");
    assert_eq!(
        named.len(),
        1,
        "expected exactly one <RequestId> in a success envelope, got {named:?}: {body}"
    );
    assert!(
        is_request_id(&named[0]),
        "expected the success envelope to name the request, got {:?}",
        named[0]
    );
    assert!(
        !body.contains("<Error>"),
        "expected a served call to carry no error block, got {body}"
    );
}
