// SPDX-License-Identifier: CC0-1.0

//! How this service CLASSIFIES the Region-ceiling refusal, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so the route, the status code, the
//! error code and the serialized body are all exercised, not just a handler
//! body.
//!
//! RULE: a refusal for a CEILING on how many records the store may hold — here
//! the number of Regions it holds records for, a bound that no wait and no retry
//! clears — is answered by EVERY call that would cross it with the quota error
//! the service models, HTTP 402 `ServiceQuotaExceededException`, naming the
//! ceiling that was reached; never with `ThrottlingException` (HTTP 429), which
//! models a RATE that was exceeded and which an SDK's default retry policy
//! retries. A call refused that way stores nothing, so the Region it named stays
//! a Region the store holds nothing for, and the Regions the store does hold go
//! on serving.
//!
//! The rows differ along what the rule generalises over: WHICH call arrives at
//! the full store — every operation of this service that can give a Region its
//! first record (`CreateWorkspace`, `CreateMetadataTransferJob` and
//! `UpdatePricingPlan`) — and, for the plan change, the request's own members,
//! one row per value of the model's `PricingMode` enum
//! (`ops::types::PRICING_MODE_VALUES`), bundled and pay-as-you-go alike.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::PRICING_MODE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    pricing_mode_is_bundled, region_ceiling_message, MAX_REGIONS, PRICING_MODE_BASIC,
    UPDATE_REASON_DEFAULT,
};

use crate::harness::{Emulator, Resp};

/// The workspace id every row uses; a create is refused for the ceiling before
/// the id is looked at, so one id serves every Region here.
const WORKSPACE: &str = "wsCeilingProbe";

/// A bundle name a bundled plan buys. One name is enough: the rule under test is
/// about the refusal class, and `pricing_plan_update.rs` is the suite about what
/// a plan change records.
const BUNDLE: &str = "TwinMaker-Basic";

/// One call that would give the Region it is signed for its FIRST record, as the
/// request a client sends.
struct Crossing {
    /// The operation, for the case label of every assertion.
    call: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// Every call of this service that can give a Region its first record, with one
/// row per modelled `PricingMode` for the plan change.
fn crossing_rows() -> Vec<Crossing> {
    let mut rows = vec![
        Crossing {
            call: "CreateWorkspace".to_string(),
            method: "POST",
            path: format!("/workspaces/{WORKSPACE}"),
            body: Some(json!({})),
        },
        Crossing {
            call: "CreateMetadataTransferJob".to_string(),
            method: "POST",
            path: "/metadata-transfer-jobs".to_string(),
            body: Some(json!({
                "metadataTransferJobId": "jobCeilingProbe",
                "sources": [{"type": "iotsitewise"}],
                "destination": {"type": "iotsitewise"},
            })),
        },
    ];
    // The mode catalog is the model's own enum rather than a list written here,
    // so a mode added to it is covered without this file moving.
    for pricing_mode in PRICING_MODE_VALUES {
        let body = if pricing_mode_is_bundled(pricing_mode) {
            json!({"pricingMode": pricing_mode, "bundleNames": [BUNDLE]})
        } else {
            json!({"pricingMode": pricing_mode})
        };
        rows.push(Crossing {
            call: format!("UpdatePricingPlan {pricing_mode}"),
            method: "POST",
            path: "/pricingplan".to_string(),
            body: Some(body),
        });
    }
    rows
}

/// `CreateWorkspace` signed for `region`.
async fn create_workspace_in(emulator: &Emulator, region: &str, workspace_id: &str) -> Resp {
    emulator
        .call_signed_for(
            Some(region),
            "POST",
            &format!("/workspaces/{workspace_id}"),
            Some(json!({})),
        )
        .await
}

/// An emulator whose store holds records for as many Regions as it may, each
/// through the create path rather than through the store behind it.
async fn emulator_at_the_region_ceiling() -> Emulator {
    let emulator = Emulator::new();
    for index in 0..MAX_REGIONS {
        let region = format!("emulator-region-{index}");
        let created = create_workspace_in(&emulator, &region, WORKSPACE).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the workspace filling the store must be created, got {created:?}"
        );
    }
    emulator
}

/// Assert that `region` holds nothing a caller put there: no workspace, no
/// metadata transfer job, and the pricing plan an untouched Region reports.
async fn assert_region_holds_nothing(emulator: &Emulator, case: &str, region: &str) {
    let workspaces = emulator
        .call_signed_for(Some(region), "POST", "/workspaces-list", Some(json!({})))
        .await;
    assert_eq!(
        workspaces.member("workspaceSummaries").as_array(),
        Some(&Vec::new()),
        "case {case:?}: expected the refused call to store no workspace, got {workspaces:?}"
    );

    let jobs = emulator
        .call_signed_for(
            Some(region),
            "POST",
            "/metadata-transfer-jobs-list",
            Some(json!({"sourceType": "iotsitewise", "destinationType": "iotsitewise"})),
        )
        .await;
    assert_eq!(
        jobs.member("metadataTransferJobSummaries").as_array(),
        Some(&Vec::new()),
        "case {case:?}: expected the refused call to store no transfer job, got {jobs:?}"
    );

    let plan = emulator
        .call_signed_for(Some(region), "GET", "/pricingplan", None)
        .await;
    let current = plan.member("currentPricingPlan").clone();
    assert_eq!(
        (
            current["pricingMode"].as_str(),
            current["updateReason"].as_str()
        ),
        (Some(PRICING_MODE_BASIC), Some(UPDATE_REASON_DEFAULT)),
        "case {case:?}: expected the refused call to leave the default plan, got {plan:?}"
    );
}

/// Every call that would give a further Region its first record is refused as
/// the QUOTA breach it is — HTTP 402 `ServiceQuotaExceededException` naming the
/// ceiling — and never as the rate error an SDK would retry, and it stores
/// nothing.
#[tokio::test]
async fn every_call_crossing_the_region_ceiling_is_refused_as_a_quota_breach() {
    let emulator = emulator_at_the_region_ceiling().await;

    for (index, row) in crossing_rows().into_iter().enumerate() {
        // A Region of its own per row, so no row can be served out of a store
        // another row's refusal might have left behind.
        let region = format!("emulator-region-beyond-{index}");
        let case = format!("{} in {region}", row.call);
        let refused = emulator
            .call_signed_for(Some(&region), row.method, &row.path, row.body.clone())
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (402, Some("ServiceQuotaExceededException")),
            "case {case:?}: expected the modelled quota refusal, got {refused:?}"
        );
        assert_eq!(
            refused.member("message").as_str(),
            Some(region_ceiling_message().as_str()),
            "case {case:?}: expected the refusal to name the ceiling it reached, got {refused:?}"
        );
        assert_region_holds_nothing(&emulator, &case, &region).await;

        // The condition the refusal reports does not clear on its own, so a
        // client that retries the same call reads the same refusal rather than a
        // success — which is why the class must not be the retryable one.
        let retried = emulator
            .call_signed_for(Some(&region), row.method, &row.path, row.body.clone())
            .await;
        assert_eq!(
            (retried.status, retried.error_type.as_deref()),
            (402, Some("ServiceQuotaExceededException")),
            "case {case:?}: expected a retry to read the same refusal, got {retried:?}"
        );
    }
}

/// The refusal is about the CEILING and not about the operation: the very same
/// calls are served in a Region the full store already holds records for, and a
/// plan change there is read back by `GetPricingPlan`.
#[tokio::test]
async fn the_same_calls_are_served_in_a_region_the_full_store_already_holds() {
    let emulator = emulator_at_the_region_ceiling().await;
    let held = "emulator-region-0";

    for row in crossing_rows() {
        let case = format!("{} in {held}", row.call);
        // Each row is sent under an id of its own so a second row is not refused
        // for the first row's record.
        let path = row.path.replace(WORKSPACE, "wsHeldProbe");
        let served = emulator
            .call_signed_for(Some(held), row.method, &path, row.body.clone())
            .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected a held Region to go on serving, got {served:?}"
        );
        if row.path == "/pricingplan" {
            let requested = row.body.as_ref().expect("a plan change carries a body")
                ["pricingMode"]
                .as_str()
                .expect("the request names a pricingMode")
                .to_string();
            let read = emulator
                .call_signed_for(Some(held), "GET", "/pricingplan", None)
                .await;
            // A change that buys the account less than it had is billed from the
            // next cycle, so the plan it names may be the PENDING one; either
            // way the plan the Region reports is the plan this call recorded.
            let reported = [
                read.member("currentPricingPlan")["pricingMode"]
                    .as_str()
                    .map(str::to_string),
                read.member("pendingPricingPlan")["pricingMode"]
                    .as_str()
                    .map(str::to_string),
            ];
            assert!(
                reported.iter().flatten().any(|mode| *mode == requested),
                "case {case:?}: expected {requested:?} to be the plan the Region reports, \
                 got {read:?}"
            );
        }
    }
}
