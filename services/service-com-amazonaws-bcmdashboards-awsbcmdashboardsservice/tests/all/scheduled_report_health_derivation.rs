// SPDX-License-Identifier: CC0-1.0

//! The one part of a scheduled report's health that is NOT re-derived from the store.
//!
//! The rule the sibling suite `scheduled_report_health_views` asserts is that a status a
//! service derives from the resources a stored record points at is recomputed from the
//! stored state every time it is reported. This suite asserts the other half of that
//! rule: a reason the record's last refresh established that the store holds nothing to
//! re-derive — a notification the account's default Amazon EventBridge event bus did not
//! take, `INTERNAL_FAILURE` (`smithy_query(query="shape",shape="StatusReason")`) — is
//! carried forward from the record, and is reported ALONGSIDE the reasons the store does
//! derive rather than instead of them.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};

use super::dashboard_lifecycle::{ok_call, router_without_event_bus};
use super::scheduled_report_execution::{
    create_report, listed_summary, stored_report, DASHBOARD_NOT_FOUND, UNHEALTHY,
};
use super::scheduled_report_execution_events::INTERNAL_FAILURE;
use super::scheduled_report_lifecycle::source_dashboard;

/// A delivery failure the last refresh recorded is reported by both views for as long as
/// the record carries it, and a reason the store derives joins it rather than replacing
/// it.
#[tokio::test]
async fn a_reason_the_store_cannot_rederive_is_carried_forward() {
    // No EventBridge in this emulator, so the notification an execution owes has nowhere
    // to land and the execution refreshes the health with INTERNAL_FAILURE.
    let router = router_without_event_bus();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "CarriedForwardSource").await;
    let arn = create_report(
        &router,
        &dashboard_arn,
        "Carried Forward Report",
        None,
        Some(vec![widget_id]),
    )
    .await;
    ok_call(&router, "ExecuteScheduledReport", &json!({"arn": arn})).await;

    // The configuration still resolves, so the only reason is the one the refresh
    // established.
    for (view, record) in [
        ("summary", listed_summary(&router, &arn).await),
        ("detail", stored_report(&router, &arn).await),
    ] {
        assert_eq!(
            record.pointer("/healthStatus/statusReasons"),
            Some(&json!([INTERNAL_FAILURE])),
            "case undelivered ({view}): expected statusReasons [{INTERNAL_FAILURE}], got {record}"
        );
        assert_eq!(
            record
                .pointer("/healthStatus/statusCode")
                .and_then(Value::as_str),
            Some(UNHEALTHY),
            "case undelivered ({view}): expected statusCode {UNHEALTHY}, got {record}"
        );
    }

    // Now the dashboard goes away too: the derived reason joins the carried one, in that
    // order, and neither replaces the other.
    ok_call(&router, "DeleteDashboard", &json!({"arn": dashboard_arn})).await;
    for (view, record) in [
        ("summary", listed_summary(&router, &arn).await),
        ("detail", stored_report(&router, &arn).await),
    ] {
        assert_eq!(
            record.pointer("/healthStatus/statusReasons"),
            Some(&json!([DASHBOARD_NOT_FOUND, INTERNAL_FAILURE])),
            "case undelivered and deleted ({view}): expected both reasons, got {record}"
        );
        assert!(
            record.pointer("/healthStatus/lastRefreshedAt").is_some(),
            "case undelivered and deleted ({view}): the refresh instant survives, got {record}"
        );
    }
}
