// SPDX-License-Identifier: CC0-1.0

//! The DASHBOARD read surface, under every `X-Amz-Target` prefix form a client writes.
//!
//! `tests/all/target_prefix_dispatch.rs` owns the dispatch rule itself — the operation
//! is the segment after the LAST dot of the target — over the trail lifecycle and over
//! targets that name no operation of this service. This suite adds only what it does
//! not cover: the pair this issue is about, `CreateDashboard` and the `GetDashboard`
//! that reads it back, so a dashboard read answers its modelled response, and its
//! modelled `ResourceNotFoundException`, to the target an AWS SDK actually sends
//! rather than the dispatcher's unmodelled 501.
//!
//! The emulator, the request builder and the assertions are the crate's shared
//! harness (`tests/all/emulator.rs`); only the cases live here.

use serde_json::{json, Value};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

use super::emulator::{Emulator, HOME_REGION, TARGET_PREFIX};

/// The `X-Amz-Target` prefix forms this service answers, each `(case, prefix)`.
///
/// Source: the model's service shape id (`smithy_query(query="service_info")` ->
/// `CloudTrail_20131101`) is the last segment of all three; the first is the fully
/// qualified prefix every AWS SDK sends for this service.
const TARGET_PREFIXES: [(&str, &str); 3] = [
    ("the fully qualified prefix an AWS SDK sends", TARGET_PREFIX),
    ("the bare service shape name", "CloudTrail_20131101."),
    (
        "one namespace label before the shape name",
        "cloudtrail.CloudTrail_20131101.",
    ),
];

/// CloudTrail's own refusal text for a dashboard identifier that names nothing,
/// written out here as the recorded wire text rather than read from the service's
/// constant, so the service text cannot drift silently.
const RECORDED_NOT_FOUND_MESSAGE: &str = "No matching dashboard found.";

#[tokio::test]
async fn the_service_constant_is_the_recorded_not_found_text() {
    assert_eq!(
        state::DASHBOARD_NOT_FOUND_MESSAGE, RECORDED_NOT_FOUND_MESSAGE,
        "the not-found refusal must keep CloudTrail's own recorded text, got {:?}",
        state::DASHBOARD_NOT_FOUND_MESSAGE
    );
}

#[tokio::test]
async fn every_target_prefix_form_serves_the_dashboard_read_surface() {
    for (form, prefix) in TARGET_PREFIXES {
        // One emulator per row, so the rows cannot see each other's dashboards.
        let emulator = Emulator::new();
        let name = "ct-dispatch-dashboard";

        // The create, and the read that reports the stored dashboard back.
        let created = emulator
            .call_with_target(
                &format!("{prefix}CreateDashboard"),
                HOME_REGION,
                &json!({"Name": name}),
            )
            .await;
        let created = created.expect_ok(form).clone();
        assert_eq!(
            created["Name"],
            json!(name),
            "case {form}: CreateDashboard did not report the dashboard it stored, got \
             {created:?}"
        );

        let fetched = emulator
            .call_with_target(
                &format!("{prefix}GetDashboard"),
                HOME_REGION,
                &json!({"DashboardId": name}),
            )
            .await;
        let fetched = fetched.expect_ok(form).clone();
        assert_eq!(
            (
                &fetched["DashboardArn"],
                &fetched["Type"],
                &fetched["Status"],
            ),
            (
                &created["DashboardArn"],
                &created["Type"],
                &json!(state::DASHBOARD_STATUS_CREATING),
            ),
            "case {form}: GetDashboard did not report the stored dashboard, got {fetched:?}"
        );

        // A dashboard nobody created answers this operation's own modelled failure,
        // with CloudTrail's fixed not-found text, under the same prefix.
        let missing = emulator
            .call_with_target(
                &format!("{prefix}GetDashboard"),
                HOME_REGION,
                &json!({"DashboardId": "ct-dispatch-ghost"}),
            )
            .await;
        missing.expect_error(form, 400, "ResourceNotFoundException");
        assert_eq!(
            missing.body["Message"],
            Value::String(RECORDED_NOT_FOUND_MESSAGE.to_string()),
            "case {form}: expected CloudTrail's not-found text, got {missing:?}"
        );
    }
}
