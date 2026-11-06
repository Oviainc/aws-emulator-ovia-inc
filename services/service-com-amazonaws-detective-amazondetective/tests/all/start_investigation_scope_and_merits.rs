// SPDX-License-Identifier: CC0-1.0

//! `StartInvestigation` reads its scope window as the modelled `date-time`, and is then
//! judged on its merits.
//!
//! The rules under test:
//!
//! * `ScopeStartTime` and `ScopeEndTime` both carry `@timestampFormat("date-time")`
//!   (`smithy_query(query="operation",operation="StartInvestigation")`), so on this
//!   service's restJson1 wire their value is the ISO-8601 string every SDK sends for a
//!   datetime argument ("The value is an UTC ISO8601 formatted string. For example,
//!   2021-08-18T16:35:56.284Z") — in every rendering RFC 3339 admits, not one fixed one.
//! * A request carrying a well-formed window is then answered on its merits: HTTP 200
//!   with an `InvestigationId` (modelled length 21..21, pattern `^[0-9]+$`), or the
//!   modelled refusal the request earns — `ResourceNotFoundException` (404) for a graph
//!   the service does not hold, `ValidationException` (400) for an `EntityArn` naming
//!   neither of the two kinds Detective investigates, and `ValidationException` (400) for
//!   a window that is not a period at all.
//!
//! No row may be answered with a timestamp-parse refusal: that is the defect these cases
//! pin shut, so each failure row also asserts the message is about the merit that failed.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE, INVESTIGATION_ID_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    assert_iso8601, call, create_graph_ok, emulator, error_code, error_message, ACCOUNT, REGION,
};

/// A well-formed `GraphArn` (the modelled pattern) that no `CreateGraph` ever minted.
fn never_created_graph_arn() -> String {
    format!(
        "arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}",
        "b".repeat(32)
    )
}

/// One `StartInvestigation` call.
async fn start_investigation(
    router: &MultiServiceRouter,
    graph_arn: &str,
    entity_arn: &str,
    scope_start: &str,
    scope_end: &str,
) -> (u16, Value) {
    call(
        router,
        "/investigations/startInvestigation",
        &json!({
            "GraphArn": graph_arn,
            "EntityArn": entity_arn,
            "ScopeStartTime": scope_start,
            "ScopeEndTime": scope_end,
        }),
    )
    .await
}

/// One row of the accepted-window table: a scope window, written in one of the
/// renderings an ISO-8601 `date-time` admits.
struct WindowCase {
    name: &'static str,
    scope_start: &'static str,
    scope_end: &'static str,
}

/// The table. The rule generalises over the RENDERING of the two instants and over the
/// period they name, so the rows differ along the fractional-second precision (none,
/// milliseconds as the model's own example writes them, microseconds), the era (the
/// epoch itself, a recent window, one past the 32-bit second rollover) and the length of
/// the window (a millisecond, an hour, a day, years).
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            name: "whole-seconds-one-day",
            scope_start: "2021-08-18T16:35:56Z",
            scope_end: "2021-08-19T16:35:56Z",
        },
        WindowCase {
            // The model's own example rendering.
            name: "milliseconds",
            scope_start: "2021-08-18T16:35:56.284Z",
            scope_end: "2021-08-18T17:35:56.284Z",
        },
        WindowCase {
            name: "microseconds",
            scope_start: "2024-05-01T12:00:00.123456Z",
            scope_end: "2024-05-01T13:00:00Z",
        },
        WindowCase {
            name: "one-millisecond-window",
            scope_start: "2024-05-01T12:00:00.000Z",
            scope_end: "2024-05-01T12:00:00.001Z",
        },
        WindowCase {
            name: "at-the-epoch",
            scope_start: "1970-01-01T00:00:00Z",
            scope_end: "1970-01-01T00:00:01Z",
        },
        WindowCase {
            // Past the 32-bit second rollover, which an epoch-integer reader would also
            // have to handle.
            name: "past-the-2038-rollover",
            scope_start: "2038-01-19T03:14:07Z",
            scope_end: "2038-01-19T03:14:08Z",
        },
        WindowCase {
            name: "years-long-window",
            scope_start: "2019-12-31T23:59:59Z",
            scope_end: "2024-01-01T00:00:00Z",
        },
    ]
}

#[tokio::test]
async fn an_investigation_starts_for_every_iso8601_rendering_of_its_scope_window() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let entity_arn = format!("arn:aws:iam::{ACCOUNT}:role/auditor");
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in window_cases() {
        let name = case.name;
        let (status, answered) = start_investigation(
            &router,
            &graph_arn,
            &entity_arn,
            case.scope_start,
            case.scope_end,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200, got {status} {answered:?}"
        );
        let investigation_id = answered
            .get("InvestigationId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: no InvestigationId in {answered:?}"))
            .to_string();
        assert_eq!(
            investigation_id.len(),
            INVESTIGATION_ID_LEN,
            "case {name:?}: expected a {INVESTIGATION_ID_LEN}-character id, got {investigation_id:?}"
        );
        assert!(
            investigation_id.bytes().all(|b| b.is_ascii_digit()),
            "case {name:?}: expected an all-digit id, got {investigation_id:?}"
        );
        assert!(
            minted.insert(investigation_id.clone()),
            "case {name:?}: id {investigation_id:?} was minted twice"
        );

        // The round trip: the window the read reports is the one the start stored,
        // compared to the second so the assertion does not depend on how a fraction is
        // rendered back.
        let (status, got) = call(
            &router,
            "/investigations/getInvestigation",
            &json!({ "GraphArn": graph_arn, "InvestigationId": investigation_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetInvestigation expected 200, got {status} {got:?}"
        );
        for (member, sent) in [
            ("ScopeStartTime", case.scope_start),
            ("ScopeEndTime", case.scope_end),
        ] {
            let rendered = assert_iso8601(&format!("{name}/{member}"), got.get(member));
            assert_eq!(
                &rendered[..19],
                &sent[..19],
                "case {name:?}: expected {member} {sent:?}, got {rendered:?}"
            );
        }
    }
}

/// One row of the refusal table: what is wrong with the request, and the modelled answer
/// it earns.
struct RefusalCase {
    name: &'static str,
    /// `false` names a well-formed graph ARN the service does not hold.
    graph_is_stored: bool,
    entity_arn: String,
    scope_start: &'static str,
    scope_end: &'static str,
    status: u16,
    code: &'static str,
}

fn refusal_cases(account: &str) -> Vec<RefusalCase> {
    let valid_start = "2024-05-01T08:00:00Z";
    let valid_end = "2024-05-01T12:00:00Z";
    vec![
        // The graph is the resource the request names: one the service does not hold is
        // the modelled 404, with the fixed graph-not-found message.
        RefusalCase {
            name: "never-created-graph",
            graph_is_stored: false,
            entity_arn: format!("arn:aws:iam::{account}:role/auditor"),
            scope_start: valid_start,
            scope_end: valid_end,
            status: 404,
            code: "ResourceNotFoundException",
        },
        // "Detective supports IAM user ARNs and IAM role ARNs": the rows differ along
        // what else the ARN names.
        RefusalCase {
            name: "entity-is-a-group",
            graph_is_stored: true,
            entity_arn: format!("arn:aws:iam::{account}:group/admins"),
            scope_start: valid_start,
            scope_end: valid_end,
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "entity-is-a-bucket",
            graph_is_stored: true,
            entity_arn: "arn:aws:s3:::example-bucket".to_string(),
            scope_start: valid_start,
            scope_end: valid_end,
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "entity-is-a-nameless-user",
            graph_is_stored: true,
            entity_arn: format!("arn:aws:iam::{account}:user/"),
            scope_start: valid_start,
            scope_end: valid_end,
            status: 400,
            code: "ValidationException",
        },
        // The scope members are the bounds of one interval: a window that runs backwards
        // or has no duration names no period to investigate. The rows differ along how
        // far it is inverted and by how little.
        RefusalCase {
            name: "window-inverted-by-hours",
            graph_is_stored: true,
            entity_arn: format!("arn:aws:iam::{account}:role/auditor"),
            scope_start: valid_end,
            scope_end: valid_start,
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "window-inverted-by-one-millisecond",
            graph_is_stored: true,
            entity_arn: format!("arn:aws:iam::{account}:user/alice"),
            scope_start: "2024-05-01T12:00:00.001Z",
            scope_end: "2024-05-01T12:00:00.000Z",
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "window-of-no-duration",
            graph_is_stored: true,
            entity_arn: format!("arn:aws:iam::{account}:user/alice"),
            scope_start: valid_start,
            scope_end: valid_start,
            status: 400,
            code: "ValidationException",
        },
    ]
}

#[tokio::test]
async fn a_well_formed_scope_window_is_refused_only_on_the_merits_of_the_request() {
    let router = emulator();
    let stored = create_graph_ok(&router, REGION, &json!({})).await;
    let absent = never_created_graph_arn();

    for case in refusal_cases(ACCOUNT) {
        let name = case.name;
        let graph_arn = if case.graph_is_stored {
            stored.as_str()
        } else {
            absent.as_str()
        };
        let (status, answered) = start_investigation(
            &router,
            graph_arn,
            &case.entity_arn,
            case.scope_start,
            case.scope_end,
        )
        .await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected {}, got {status} {answered:?}",
            case.status
        );
        assert_eq!(
            error_code(&answered),
            case.code,
            "case {name:?}: expected {}, got {answered:?}",
            case.code
        );
        let message = error_message(&answered);
        // The request never died in the timestamp reader: the refusal is about the
        // merit that failed.
        assert!(
            !message.to_ascii_lowercase().contains("timestamp"),
            "case {name:?}: expected a refusal about the request's merits, got {message:?}"
        );
        if case.graph_is_stored {
            assert_ne!(
                message, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE,
                "case {name:?}: the graph was found, got {message:?}"
            );
        } else {
            assert_eq!(
                message, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE,
                "case {name:?}: expected the investigation surface's fixed \
                 graph-not-found message, got {message:?}"
            );
        }
    }
}

/// The sentence this operation refuses a window that is not a period at all with. Fixed
/// text: it names the condition and interpolates neither instant of the request, so one
/// row's instants can never appear in it.
const INVALID_WINDOW_MESSAGE: &str =
    "Invalid investigation window: End time is before or equal to start time.";

#[tokio::test]
async fn a_window_that_is_not_a_period_is_refused_in_the_same_words_whatever_its_instants() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let entity_arn = format!("arn:aws:iam::{ACCOUNT}:user/alice");

    // The rows differ along HOW the window fails to be a period — inverted by years, by
    // a day, by a single millisecond, and of no duration at all — and along where the
    // window sits in time, so the one sentence is pinned rather than one row's instants.
    let rows = [
        ("inverted-by-years", "2024-05-01T12:00:00Z", "2019-05-01T12:00:00Z"),
        ("inverted-by-a-day", "2024-05-02T12:00:00Z", "2024-05-01T12:00:00Z"),
        (
            "inverted-by-one-millisecond",
            "2024-05-01T12:00:00.001Z",
            "2024-05-01T12:00:00.000Z",
        ),
        ("no-duration", "2021-08-18T16:35:56.284Z", "2021-08-18T16:35:56.284Z"),
        ("no-duration-at-the-epoch", "1970-01-01T00:00:00Z", "1970-01-01T00:00:00Z"),
    ];
    for (case, scope_start, scope_end) in rows {
        let (status, answered) =
            start_investigation(&router, &graph_arn, &entity_arn, scope_start, scope_end).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: got {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message, INVALID_WINDOW_MESSAGE,
            "case {case:?}: expected the fixed refusal sentence, got {message:?}"
        );
        assert!(
            !message.contains(scope_start) && !message.contains(scope_end),
            "case {case:?}: the refusal echoes neither instant back, got {message:?}"
        );
    }
}
