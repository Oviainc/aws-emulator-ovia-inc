// SPDX-License-Identifier: CC0-1.0

//! The health gate every writer of a stored billing view shares, end to end
//! over the awsJson1_0 wire.
//!
//! One rule: an operation whose model declares
//! `BillingViewHealthStatusException` and writes a stored billing view refuses
//! the request with that error (HTTP 400) — and writes nothing — whenever the
//! view it would change carries a `BillingViewStatus` other than `HEALTHY`,
//! whichever members the request carries.
//!
//! The rows span what the rule generalises over: every writing operation the
//! model declares the error for (`UpdateBillingView`, `AssociateSourceViews`,
//! `DisassociateSourceViews`), each of the non-`HEALTHY` `BillingViewStatus`
//! values (`CREATING`, `UNHEALTHY`, `UPDATING` — the `BillingViewStatus` enum
//! from `smithy_query(query="shape",shape="BillingViewStatus")` minus
//! `HEALTHY`), and, for the update, which members the request carries (name,
//! description, the data filter expression, all of them, none of them).
//!
//! Every row ends with the same request against the same view once it is
//! `HEALTHY` again: the control that shows the refusal came from the gate and
//! not from the request, and that the write the gate held back then lands and is
//! visible through `GetBillingView`.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns; the
//! store-side health move and the view creation are `source_view_disassociation.rs`'s.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, emulator_with_engine, error_code, PRIMARY_ARN};
use crate::source_view_disassociation::{create_view, set_health_status};

/// Every `BillingViewStatus` the model declares except `HEALTHY` — the states
/// the modeled error calls "a state other than HEALTHY"
/// (`smithy_query(query="shape",shape="BillingViewStatus")` =
/// HEALTHY, UNHEALTHY, CREATING, UPDATING).
const NOT_HEALTHY: [&str; 3] = ["CREATING", "UNHEALTHY", "UPDATING"];

/// One row: a writing operation, the members its request carries beside `arn`,
/// and what the `GetBillingView` projection must report once the same request
/// goes through against a `HEALTHY` view.
struct Row {
    case: &'static str,
    operation: &'static str,
    members: Value,
    /// `billingView` members the successful control call must have written,
    /// as (member name, value).
    written: Vec<(&'static str, Value)>,
}

fn filter_expression() -> Value {
    json!({ "dimensions": { "key": "LINKED_ACCOUNT", "values": ["000000000000"] } })
}

/// The two source views each row's view is created holding.
fn held() -> Vec<String> {
    vec![
        crate::source_view_disassociation::source_arn("health-a"),
        crate::source_view_disassociation::source_arn("health-b"),
    ]
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "UpdateBillingView, name only",
            operation: "UpdateBillingView",
            members: json!({ "name": "renamed" }),
            written: vec![("name", json!("renamed"))],
        },
        Row {
            case: "UpdateBillingView, description only",
            operation: "UpdateBillingView",
            members: json!({ "description": "a new description" }),
            written: vec![("description", json!("a new description"))],
        },
        Row {
            case: "UpdateBillingView, data filter expression only",
            operation: "UpdateBillingView",
            members: json!({ "dataFilterExpression": filter_expression() }),
            written: vec![("dataFilterExpression", filter_expression())],
        },
        Row {
            case: "UpdateBillingView, every member at once",
            operation: "UpdateBillingView",
            members: json!({
                "name": "all three",
                "description": "all three too",
                "dataFilterExpression": filter_expression(),
            }),
            written: vec![
                ("name", json!("all three")),
                ("description", json!("all three too")),
                ("dataFilterExpression", filter_expression()),
            ],
        },
        Row {
            case: "UpdateBillingView, no member beside the arn",
            operation: "UpdateBillingView",
            members: json!({}),
            written: vec![],
        },
        Row {
            case: "AssociateSourceViews, one more source",
            operation: "AssociateSourceViews",
            members: json!({ "sourceViews": [PRIMARY_ARN] }),
            written: vec![("sourceViewCount", json!(3))],
        },
        Row {
            case: "DisassociateSourceViews, one held source",
            operation: "DisassociateSourceViews",
            members: json!({ "sourceViews": [held()[0]] }),
            written: vec![("sourceViewCount", json!(1))],
        },
    ]
}

/// The whole persisted projection of `arn`, as `GetBillingView` reports it.
async fn stored_view(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, got) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the read-back to answer 200, got {status} {got:?}"
    );
    got["billingView"].clone()
}

#[tokio::test]
async fn a_view_that_is_not_healthy_refuses_every_write() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let arn = create_view(&router, case, &format!("health-{index}"), &held()).await;
        let mut request = row.members.clone();
        request["arn"] = json!(arn);

        for status_code in NOT_HEALTHY {
            set_health_status(&engine, &arn, status_code);
            let before = stored_view(&router, case, &arn).await;

            let (code, body) = call(&router, row.operation, &request).await;
            assert_eq!(
                (code, error_code(&body).as_str()),
                (400, "BillingViewHealthStatusException"),
                "case {case:?} at {status_code:?}: expected 400 \
                 BillingViewHealthStatusException, got {code} {body:?}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(status_code)
                        && message.contains("HEALTHY")),
                "case {case:?} at {status_code:?}: the refusal names the status that \
                 blocked it, got {:?}",
                body["message"]
            );

            let after = stored_view(&router, case, &arn).await;
            assert_eq!(
                after, before,
                "case {case:?} at {status_code:?}: a refused write leaves the stored \
                 view untouched, got {after:?}"
            );
        }

        // The control: the very same request against the very same view, once it
        // is HEALTHY again.
        set_health_status(&engine, &arn, "HEALTHY");
        let (code, body) = call(&router, row.operation, &request).await;
        assert_eq!(
            code, 200,
            "case {case:?} at \"HEALTHY\": expected the write to go through, got {code} {body:?}"
        );
        let stored = stored_view(&router, case, &arn).await;
        for (member, expected) in &row.written {
            assert_eq!(
                &stored[member], expected,
                "case {case:?} at \"HEALTHY\": expected {member} to be {expected:?}, got {:?}",
                stored[member]
            );
        }
    }
}

#[tokio::test]
async fn the_update_the_gate_refused_is_not_applied_later() {
    // The gate refuses rather than queues: a view that was UNHEALTHY while the
    // update was attempted still carries its created attributes once it is
    // HEALTHY again, and no instant on it moved.
    let (router, engine) = emulator_with_engine();
    let arn = create_view(&router, "not queued", "health-not-queued", &held()).await;
    let created = stored_view(&router, "not queued", &arn).await;

    set_health_status(&engine, &arn, "UNHEALTHY");
    let (code, body) = call(
        &router,
        "UpdateBillingView",
        &json!({ "arn": arn, "name": "never stored", "dataFilterExpression": filter_expression() }),
    )
    .await;
    assert_eq!(
        (code, error_code(&body).as_str()),
        (400, "BillingViewHealthStatusException"),
        "case \"not queued\": expected 400 BillingViewHealthStatusException, got {code} {body:?}"
    );

    set_health_status(&engine, &arn, "HEALTHY");
    let stored = stored_view(&router, "not queued", &arn).await;
    assert_eq!(
        stored["name"], created["name"],
        "case \"not queued\": the refused name must not appear, got {:?}",
        stored["name"]
    );
    assert!(
        stored["dataFilterExpression"].is_null(),
        "case \"not queued\": the refused expression must not appear, got {:?}",
        stored["dataFilterExpression"]
    );
    assert!(
        stored["updatedAt"].is_null() && stored["viewDefinitionLastUpdatedAt"].is_null(),
        "case \"not queued\": a refused update moves no instant, got {stored:?}"
    );
}
