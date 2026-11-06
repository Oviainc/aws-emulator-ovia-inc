// SPDX-License-Identifier: CC0-1.0

//! `GetEnabledControl`'s `targetRegions` against the landing zone's governance, end to
//! end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees.
//!
//! The rule under test, asserted over a TABLE of governance changes rather than one
//! case: the regions an enabled control reports are the regions the landing zone
//! governs NOW — the model's `Region` says "the expected Regions are based on the
//! Regions that are governed by the landing zone" — so every deployment of a different
//! `governedRegions` list moves an already-enabled control's `targetRegions` with it,
//! and a change an `UpdateLandingZone` is still deploying has not moved them yet. With
//! no governed region to read, the only region an enablement names is the one its own
//! request was signed for.
//!
//! The region names are ordinary AWS Region names; what each row varies is the SHAPE of
//! the governance change (extend, narrow, replace, reorder, empty, a sequence of them,
//! and governance arriving only after the enablement), not the particular regions.

use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{control, details, enable, entries, ou};
use crate::landing_zone_lifecycle::{
    call, call_in_region, emulator, manifest_governing, past_the_settle_span,
    set_up_then_decommission, SIGNED_REGION,
};

/// A landing zone version the catalog offers, for the rows whose subject is the manifest
/// rather than the version.
const VERSION: &str = "3.3";

/// One row of [`target_regions_follow_the_landing_zones_current_governance`].
struct GovernanceCase {
    case: &'static str,
    /// The `governedRegions` of the landing zone deployed BEFORE the control is enabled, or
    /// `None` to enable the control with no landing zone deployed — an account set up and
    /// decommissioned again, which is the only way a control enables with none, since
    /// Control Tower enables one only for an account it has been set up in.
    before: Option<&'static [&'static str]>,
    /// The `governedRegions` each later deployment governs, in order.
    after: &'static [&'static [&'static str]],
}

/// The governance table.
fn governance_cases() -> Vec<GovernanceCase> {
    vec![
        GovernanceCase {
            case: "a region added to governance after the enablement",
            before: Some(&["us-west-2"]),
            after: &[&["us-west-2", "eu-west-1"]],
        },
        GovernanceCase {
            case: "governance narrowed to one of the regions it held",
            before: Some(&["us-west-2", "eu-west-1", "ap-south-1"]),
            after: &[&["eu-west-1"]],
        },
        GovernanceCase {
            case: "governance replaced by an entirely different set",
            before: Some(&["us-west-2"]),
            after: &[&["eu-central-1", "ca-central-1"]],
        },
        GovernanceCase {
            case: "the same regions governed in another order",
            before: Some(&["us-east-1", "us-east-2"]),
            after: &[&["us-east-2", "us-east-1"]],
        },
        GovernanceCase {
            case: "governance emptied: no region governed at all",
            before: Some(&["eu-west-1"]),
            after: &[&[]],
        },
        GovernanceCase {
            case: "no landing zone at enable time, then one governing another region",
            before: None,
            after: &[&["eu-central-1"]],
        },
        GovernanceCase {
            case: "one governance change after another",
            before: Some(&["us-west-2"]),
            after: &[
                &["us-west-2", "eu-west-1"],
                &["ap-northeast-1"],
                &["us-west-2", "ap-northeast-1", "eu-west-1"],
            ],
        },
    ]
}

/// The `targetRegions` a control must report while `governed` is the deployed governed
/// set: that set in its manifest's order, or — with nothing governed — the region the
/// enabling request was signed for.
fn expected_regions(governed: &[&str], enablement_region: &str) -> Vec<serde_json::Value> {
    let names: Vec<&str> = if governed.is_empty() {
        vec![enablement_region]
    } else {
        governed.to_vec()
    };
    names
        .into_iter()
        .map(|name| serde_json::json!({ "name": name }))
        .collect()
}

/// Assert the control at `arn` reports exactly `expected` as its `targetRegions`.
async fn assert_regions(
    router: &MultiServiceRouter,
    case: &str,
    when: &str,
    arn: &str,
    expected: &[serde_json::Value],
) {
    let detail = details(router, arn).await;
    let actual = entries(
        detail
            .get("targetRegions")
            .unwrap_or(&serde_json::Value::Null),
    );
    assert_eq!(
        actual, expected,
        "case {case:?} {when}: expected target regions {expected:?}, got {actual:?}"
    );
}

/// Create the account's one landing zone governing `governed`, answering its ARN.
async fn create_landing_zone(
    router: &MultiServiceRouter,
    case: &str,
    governed: &[&str],
) -> String {
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({ "version": VERSION, "manifest": manifest_governing(governed) }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: create-landingzone governing {governed:?} expected 200, got {} \
         body {:?}",
        created.status, created.body
    );
    created.string("arn")
}

/// Start an `UpdateLandingZone` that re-governs `governed`.
async fn update_governance(
    router: &MultiServiceRouter,
    case: &str,
    landing_zone: &str,
    governed: &[&str],
) {
    let updated = call(
        router,
        "/update-landingzone",
        serde_json::json!({
            "landingZoneIdentifier": landing_zone,
            "version": VERSION,
            "manifest": manifest_governing(governed),
        }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: update-landingzone governing {governed:?} expected 200, got {} \
         body {:?}",
        updated.status, updated.body
    );
}

/// Assert `GetLandingZone` reports `governed` as the DEPLOYED governed set, so a row that
/// passes is one where the governance really moved.
async fn assert_deployed_governance(
    router: &MultiServiceRouter,
    case: &str,
    landing_zone: &str,
    governed: &[&str],
) {
    let answer = call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": landing_zone }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: get-landingzone expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    let deployed = answer
        .get("landingZone")
        .get("manifest")
        .and_then(|manifest| manifest.get("governedRegions"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    assert_eq!(
        deployed,
        serde_json::json!(governed),
        "case {case:?}: expected the deployed manifest to govern {governed:?}, got \
         {deployed:?}"
    );
}

/// RULE: an enabled control's `targetRegions` are the regions the landing zone governs at
/// the moment it is read, so a governance change extends or withdraws an existing
/// enablement — and a change still being deployed has not moved it yet.
#[tokio::test]
async fn target_regions_follow_the_landing_zones_current_governance() {
    for row in governance_cases() {
        let router = emulator();

        let mut landing_zone: Option<String> = None;
        let mut governed: Vec<&str> = Vec::new();
        match row.before {
            Some(before) => {
                landing_zone = Some(create_landing_zone(&router, row.case, before).await);
                governed = before.to_vec();
                tokio::time::sleep(past_the_settle_span()).await;
            }
            None => set_up_then_decommission(&router, row.case).await,
        }

        let (arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou("1010"),
            }),
        )
        .await;
        assert_regions(
            &router,
            row.case,
            "at enable time",
            &arn,
            &expected_regions(&governed, SIGNED_REGION),
        )
        .await;

        for step in row.after {
            match landing_zone.clone() {
                // The account has no landing zone yet, so this governance arrives with
                // the landing zone itself: `CreateLandingZone` deploys the manifest it
                // carries.
                None => {
                    landing_zone = Some(create_landing_zone(&router, row.case, step).await);
                    tokio::time::sleep(past_the_settle_span()).await;
                }
                // An update deploys asynchronously, so until the clock makes it due the
                // control still reports the governance actually deployed.
                Some(existing) => {
                    update_governance(&router, row.case, &existing, step).await;
                    assert_regions(
                        &router,
                        row.case,
                        "while the governance change is still deploying",
                        &arn,
                        &expected_regions(&governed, SIGNED_REGION),
                    )
                    .await;
                    tokio::time::sleep(past_the_settle_span()).await;
                }
            }

            governed = step.to_vec();
            let deployed_in = landing_zone
                .clone()
                .expect("a landing zone is deployed by now");
            assert_deployed_governance(&router, row.case, &deployed_in, &governed).await;
            assert_regions(
                &router,
                row.case,
                "once the governance change is deployed",
                &arn,
                &expected_regions(&governed, SIGNED_REGION),
            )
            .await;
        }
    }
}

/// RULE: with no governed region to read, the region an enablement names is the one the
/// ENABLING request was signed for — not the reader's, and not one a later read invents.
#[tokio::test]
async fn with_nothing_governed_the_region_is_the_enabling_requests_own() {
    // The enabling request's region, and the region the later reads are signed for.
    let cases = [
        ("the same region enables and reads", "eu-west-1", "eu-west-1"),
        ("another region reads", "eu-central-1", "us-west-2"),
        ("the reader's region governs nothing either", "ap-south-1", "us-east-2"),
    ];

    for (case, enabling_region, reading_region) in cases {
        let router = emulator();
        set_up_then_decommission(&router, case).await;
        let enabled = call_in_region(
            &router,
            enabling_region,
            "/enable-control",
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou("2020"),
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: enable-control in {enabling_region} expected 200, got {} \
             body {:?}",
            enabled.status, enabled.body
        );
        let arn = enabled.string("arn");

        let read = call_in_region(
            &router,
            reading_region,
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": arn }),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: get-enabled-control in {reading_region} expected 200, got {} \
             body {:?}",
            read.status, read.body
        );
        let actual = entries(read.get("enabledControlDetails").get("targetRegions").unwrap_or(
            &serde_json::Value::Null,
        ));
        let expected = vec![serde_json::json!({ "name": enabling_region })];
        assert_eq!(
            actual, expected,
            "case {case:?}: expected target regions {expected:?}, got {actual:?}"
        );
    }
}
