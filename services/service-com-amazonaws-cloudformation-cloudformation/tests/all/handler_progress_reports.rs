// SPDX-License-Identifier: CC0-1.0

//! `RecordHandlerProgress` over the awsQuery wire: the progress a resource handler reports
//! for one handler invocation.
//!
//! RULE: a progress report is about the ONE handler invocation its `BearerToken` names in
//! the Region the request was signed for, and is accepted only when that invocation can
//! take it — the `CurrentOperationStatus` it asserts has to be the status the invocation
//! holds, and the `OperationStatus` it reports has to be that same status again (the
//! operation is `@idempotent`) or one that comes after it in `PENDING` → `IN_PROGRESS` →
//! `SUCCESS`/`FAILED`, so a terminal status has nothing legal after it. An accepted report
//! leaves the invocation holding the reported status and answers 200 with no members; every
//! other report — a token that names no invocation (`BearerToken '<token>' is invalid`), a
//! `CurrentOperationStatus` that is not the status held, a status that cannot follow it — is
//! refused with HTTP 400 `ValidationError` and writes nothing.
//!
//! The rows differ along what the rule generalises over: the status the invocation holds,
//! whether the report asserts a current status and whether that assertion is right, which
//! status is reported, the direction between the two, which Region the report is signed
//! for, and which of the report's optional members it carries.
//!
//! The store's WRITER is a handler INVOCATION, which nothing in this emulator performs: a
//! stack's resources are created by calling the owning service's own API rather than by
//! running an extension's handler package, so no bearer token is issued here (see
//! `state::handler_progress`) and the invocations below are seeded straight into the store —
//! the same device `hook_result_invocations.rs` uses for records whose writers are not
//! available. Every REPORT goes through the real request path (form-encoded body in, XML
//! envelope out), so what is asserted is what a client sees.

use service_com_amazonaws_cloudformation_cloudformation::state::handler_progress::{
    operation_status, StoredHandlerInvocation, MAX_HANDLER_INVOCATIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, member, router_with_engine, store, xml_text, REGION};

/// The Region no seeded invocation of the table lives in: a bearer token names an
/// invocation only in the Region the report was signed for.
const OTHER_REGION: &str = "us-west-2";

/// A `BearerToken` for the invocation of one row, in the modelled `ClientToken`
/// `@length(1..128)`: built from the row's index so a row names its own record.
fn bearer_token(index: usize) -> String {
    format!("handler-invocation-{index:04}")
}

/// One `RecordHandlerProgress` call, as an SDK sends it.
async fn report(
    router: &MultiServiceRouter,
    region: &str,
    token: &str,
    asserted: Option<&str>,
    reported: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("BearerToken", token),
        member("OperationStatus", reported),
    ];
    if let Some(asserted) = asserted {
        members.push(member("CurrentOperationStatus", asserted));
    }
    members.extend_from_slice(extra);
    call_in(router, region, "RecordHandlerProgress", &members).await
}

/// One row of the table: the invocation a report names, what the report says, and whether
/// the invocation can take it.
#[derive(Debug, Clone, Copy)]
struct Case {
    case: &'static str,
    /// The `OperationStatus` the seeded invocation holds, or `None` when nothing is seeded
    /// for the row's token — which is every token in an emulator that issues none.
    recorded: Option<&'static str>,
    /// The `CurrentOperationStatus` the report asserts, where it carries one.
    asserted: Option<&'static str>,
    /// The `OperationStatus` the report reports.
    reported: &'static str,
    /// Whether the invocation can take the report.
    accepted: bool,
}

/// The refusal every rejected report answers with, which is this service's own wire code
/// for a request it refuses on input grounds.
const REFUSAL_CODE: &str = "ValidationError";

#[tokio::test]
async fn a_report_is_judged_against_the_status_its_invocation_holds() {
    let cases = [
        Case {
            case: "pending, reported in progress",
            recorded: Some(operation_status::PENDING),
            asserted: None,
            reported: operation_status::IN_PROGRESS,
            accepted: true,
        },
        Case {
            case: "pending asserted, reported in progress",
            recorded: Some(operation_status::PENDING),
            asserted: Some(operation_status::PENDING),
            reported: operation_status::IN_PROGRESS,
            accepted: true,
        },
        Case {
            case: "pending, reported straight to success",
            recorded: Some(operation_status::PENDING),
            asserted: None,
            reported: operation_status::SUCCESS,
            accepted: true,
        },
        Case {
            case: "pending asserted, reported straight to failed",
            recorded: Some(operation_status::PENDING),
            asserted: Some(operation_status::PENDING),
            reported: operation_status::FAILED,
            accepted: true,
        },
        Case {
            case: "in progress, reported in progress again",
            recorded: Some(operation_status::IN_PROGRESS),
            asserted: Some(operation_status::IN_PROGRESS),
            reported: operation_status::IN_PROGRESS,
            accepted: true,
        },
        Case {
            case: "in progress, reported success",
            recorded: Some(operation_status::IN_PROGRESS),
            asserted: None,
            reported: operation_status::SUCCESS,
            accepted: true,
        },
        Case {
            case: "in progress asserted, reported failed",
            recorded: Some(operation_status::IN_PROGRESS),
            asserted: Some(operation_status::IN_PROGRESS),
            reported: operation_status::FAILED,
            accepted: true,
        },
        Case {
            case: "in progress, reported back as pending",
            recorded: Some(operation_status::IN_PROGRESS),
            asserted: None,
            reported: operation_status::PENDING,
            accepted: false,
        },
        Case {
            case: "success, reported in progress again",
            recorded: Some(operation_status::SUCCESS),
            asserted: Some(operation_status::SUCCESS),
            reported: operation_status::IN_PROGRESS,
            accepted: false,
        },
        Case {
            case: "success, reported success again",
            recorded: Some(operation_status::SUCCESS),
            asserted: None,
            reported: operation_status::SUCCESS,
            accepted: true,
        },
        Case {
            case: "success, reported failed instead",
            recorded: Some(operation_status::SUCCESS),
            asserted: Some(operation_status::SUCCESS),
            reported: operation_status::FAILED,
            accepted: false,
        },
        Case {
            case: "failed, reported in progress again",
            recorded: Some(operation_status::FAILED),
            asserted: None,
            reported: operation_status::IN_PROGRESS,
            accepted: false,
        },
        Case {
            case: "failed, reported failed again",
            recorded: Some(operation_status::FAILED),
            asserted: Some(operation_status::FAILED),
            reported: operation_status::FAILED,
            accepted: true,
        },
        Case {
            case: "failed, reported success instead",
            recorded: Some(operation_status::FAILED),
            asserted: None,
            reported: operation_status::SUCCESS,
            accepted: false,
        },
        Case {
            case: "pending, asserted as in progress",
            recorded: Some(operation_status::PENDING),
            asserted: Some(operation_status::IN_PROGRESS),
            reported: operation_status::IN_PROGRESS,
            accepted: false,
        },
        Case {
            case: "success, asserted as in progress",
            recorded: Some(operation_status::SUCCESS),
            asserted: Some(operation_status::IN_PROGRESS),
            reported: operation_status::SUCCESS,
            accepted: false,
        },
        Case {
            case: "in progress, asserted as pending",
            recorded: Some(operation_status::IN_PROGRESS),
            asserted: Some(operation_status::PENDING),
            reported: operation_status::SUCCESS,
            accepted: false,
        },
        Case {
            case: "token names no invocation",
            recorded: None,
            asserted: None,
            reported: operation_status::IN_PROGRESS,
            accepted: false,
        },
        Case {
            case: "token names no invocation, success asserted",
            recorded: None,
            asserted: Some(operation_status::SUCCESS),
            reported: operation_status::IN_PROGRESS,
            accepted: false,
        },
        Case {
            case: "token names no invocation, success reported",
            recorded: None,
            asserted: None,
            reported: operation_status::SUCCESS,
            accepted: false,
        },
    ];

    let (router, engine) = router_with_engine();
    // Every row's invocation is seeded under its own token, so one row's accepted report
    // cannot be what the next row is judged against.
    for (index, case) in cases.iter().enumerate() {
        if let Some(recorded) = case.recorded {
            store(&engine)
                .lock()
                .record_handler_invocation(StoredHandlerInvocation::new(
                    REGION,
                    bearer_token(index),
                    recorded,
                ));
        }
    }

    for (index, case) in cases.iter().enumerate() {
        let token = bearer_token(index);
        let (status, body) = report(
            &router,
            REGION,
            &token,
            case.asserted,
            case.reported,
            &[],
        )
        .await;

        if case.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the report to be accepted with HTTP 200, got {status} {body}",
                case.case
            );
            assert!(
                xml_text(&body, "Code").is_none(),
                "case {:?}: an accepted report answers no error code, got {body}",
                case.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: expected the report to be refused with HTTP 400, got {status} {body}",
                case.case
            );
            let code = xml_text(&body, "Code");
            assert_eq!(
                code.as_deref(),
                Some(REFUSAL_CODE),
                "case {:?}: expected the refusal code {REFUSAL_CODE}, got {code:?} in {body}",
                case.case
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            // What the refusal has to SAY: the member and the token it is about for a token
            // that names nothing, and both statuses it is between for a check or a
            // transition the invocation cannot take.
            let expected_fragments: Vec<String> = match (case.recorded, case.asserted) {
                (None, _) => vec![format!("BearerToken '{token}' is invalid")],
                (Some(recorded), Some(asserted)) if asserted != recorded => {
                    vec![recorded.to_string(), asserted.to_string()]
                }
                (Some(recorded), _) => vec![recorded.to_string(), case.reported.to_string()],
            };
            for fragment in &expected_fragments {
                assert!(
                    message.contains(fragment.as_str()),
                    "case {:?}: expected the refusal message to name {fragment:?}, got \
                     {message:?}",
                    case.case
                );
            }
        }

        // What the invocation holds now: the reported status where the report was accepted,
        // the status it already held where it was refused — and still nothing at all for a
        // token that named no invocation, since a refused report writes nothing.
        let held = store(&engine)
            .lock()
            .handler_invocations
            .iter()
            .find(|invocation| invocation.is_addressed_by(REGION, &token))
            .map(|invocation| invocation.operation_status.clone());
        let expected = if case.accepted {
            Some(case.reported.to_string())
        } else {
            case.recorded.map(str::to_string)
        };
        assert_eq!(
            held, expected,
            "case {:?}: expected the invocation to hold {expected:?} after the report, got \
             {held:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_accepted_report_is_what_the_next_report_is_judged_against() {
    let (router, engine) = router_with_engine();
    let token = bearer_token(0);
    store(&engine)
        .lock()
        .record_handler_invocation(StoredHandlerInvocation::new(
            REGION,
            &token,
            operation_status::PENDING,
        ));

    // One report at a time, each judged against what the previous one left behind: this is
    // the only read of a recorded report a client has, since the answer carries no members.
    let steps: &[(&str, Option<&str>, &str, u16)] = &[
        (
            "the first report moves the pending invocation on",
            Some(operation_status::PENDING),
            operation_status::IN_PROGRESS,
            200,
        ),
        (
            "the status it held before is no longer the current one",
            Some(operation_status::PENDING),
            operation_status::IN_PROGRESS,
            400,
        ),
        (
            "the status the accepted report left is the current one",
            Some(operation_status::IN_PROGRESS),
            operation_status::IN_PROGRESS,
            200,
        ),
        (
            "and is what the invocation finishes from",
            Some(operation_status::IN_PROGRESS),
            operation_status::SUCCESS,
            200,
        ),
        (
            "a finished invocation takes no further report",
            Some(operation_status::SUCCESS),
            operation_status::IN_PROGRESS,
            400,
        ),
        (
            "not even one that asserts the status it no longer holds",
            Some(operation_status::IN_PROGRESS),
            operation_status::SUCCESS,
            400,
        ),
    ];
    for (case, asserted, reported, expected_status) in steps {
        let (status, body) = report(&router, REGION, &token, *asserted, reported, &[]).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
    }

    let held = store(&engine)
        .lock()
        .handler_invocations
        .iter()
        .find(|invocation| invocation.is_addressed_by(REGION, &token))
        .map(|invocation| invocation.operation_status.clone());
    assert_eq!(
        held.as_deref(),
        Some(operation_status::SUCCESS),
        "case the finished invocation: expected it to hold {}, got {held:?}",
        operation_status::SUCCESS
    );
}

/// One row of the table that holds the report's other members to changing nothing: what it
/// reports, and which of those members it carries.
struct OtherMembers {
    case: &'static str,
    reported: &'static str,
    extra: Vec<(String, String)>,
}

#[tokio::test]
async fn the_reports_other_members_change_nothing_about_the_outcome() {
    let (router, engine) = router_with_engine();
    // `StatusMessage` (@length(0..1024)), `ErrorCode` (a `HandlerErrorCode`),
    // `ResourceModel` (@length(1..16384)) and `ClientRequestToken` (@length(1..128),
    // @pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")) are all "reserved for use by the
    // CloudFormation CLI": a report carrying any of them is judged exactly as one carrying
    // none.
    let cases = [
        OtherMembers {
            case: "a status message beside an in-progress report",
            reported: operation_status::IN_PROGRESS,
            extra: vec![member("StatusMessage", "Creating the resource")],
        },
        OtherMembers {
            case: "an error code beside a failed report",
            reported: operation_status::FAILED,
            extra: vec![
                member("ErrorCode", "NotFound"),
                member("StatusMessage", "The resource was not found"),
            ],
        },
        OtherMembers {
            case: "a resource model and a client request token beside a success",
            reported: operation_status::SUCCESS,
            extra: vec![
                member("ResourceModel", "{\"Name\":\"the-resource\"}"),
                member("ClientRequestToken", "a0b1c2d3-request"),
            ],
        },
        OtherMembers {
            case: "every one of them at once",
            reported: operation_status::SUCCESS,
            extra: vec![
                member("StatusMessage", "Created"),
                member("ErrorCode", "InternalFailure"),
                member("ResourceModel", "{}"),
                member("ClientRequestToken", "another-request"),
            ],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let token = bearer_token(index);
        store(&engine)
            .lock()
            .record_handler_invocation(StoredHandlerInvocation::new(
                REGION,
                &token,
                operation_status::IN_PROGRESS,
            ));
        let (status, body) = report(
            &router,
            REGION,
            &token,
            Some(operation_status::IN_PROGRESS),
            case.reported,
            &case.extra,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the report to be accepted with HTTP 200, got {status} {body}",
            case.case
        );
        let held = store(&engine)
            .lock()
            .handler_invocations
            .iter()
            .find(|invocation| invocation.is_addressed_by(REGION, &token))
            .map(|invocation| invocation.operation_status.clone());
        assert_eq!(
            held.as_deref(),
            Some(case.reported),
            "case {:?}: expected the invocation to hold {}, got {held:?}",
            case.case,
            case.reported
        );
    }
}

#[tokio::test]
async fn a_token_names_an_invocation_only_in_its_own_region() {
    let (router, engine) = router_with_engine();
    let token = bearer_token(0);
    store(&engine)
        .lock()
        .record_handler_invocation(StoredHandlerInvocation::new(
            OTHER_REGION,
            &token,
            operation_status::IN_PROGRESS,
        ));

    let (status, body) = report(
        &router,
        REGION,
        &token,
        Some(operation_status::IN_PROGRESS),
        operation_status::SUCCESS,
        &[],
    )
    .await;
    assert_eq!(
        status, 400,
        "case a token of another Region: expected HTTP 400, got {status} {body}"
    );
    let held = store(&engine)
        .lock()
        .handler_invocations
        .iter()
        .find(|invocation| invocation.is_addressed_by(OTHER_REGION, &token))
        .map(|invocation| invocation.operation_status.clone());
    assert_eq!(
        held.as_deref(),
        Some(operation_status::IN_PROGRESS),
        "case a token of another Region: the invocation there must be untouched, got {held:?}"
    );

    // The SAME report, signed for the Region the invocation belongs to, is the one it takes.
    let (status, body) = report(
        &router,
        OTHER_REGION,
        &token,
        Some(operation_status::IN_PROGRESS),
        operation_status::SUCCESS,
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case the invocation's own Region: expected HTTP 200, got {status} {body}"
    );
}

#[tokio::test]
async fn the_store_keeps_a_bounded_number_of_invocations() {
    let (router, engine) = router_with_engine();
    // One past the bound: the oldest record is reclaimed, so the token that named it names
    // no invocation any more while the newest still does.
    let seeded = MAX_HANDLER_INVOCATIONS + 1;
    for index in 0..seeded {
        store(&engine)
            .lock()
            .record_handler_invocation(StoredHandlerInvocation::new(
                REGION,
                bearer_token(index),
                operation_status::IN_PROGRESS,
            ));
    }
    assert_eq!(
        store(&engine).lock().handler_invocations.len(),
        MAX_HANDLER_INVOCATIONS,
        "case seeding one past the bound: the store keeps at most {MAX_HANDLER_INVOCATIONS} \
         invocations"
    );

    for (case, index, expected_status) in [
        ("the reclaimed oldest invocation", 0, 400),
        ("the oldest invocation still held", 1, 200),
        ("the newest invocation", seeded - 1, 200),
    ] {
        let (status, body) = report(
            &router,
            REGION,
            &bearer_token(index),
            Some(operation_status::IN_PROGRESS),
            operation_status::SUCCESS,
            &[],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
    }
}
