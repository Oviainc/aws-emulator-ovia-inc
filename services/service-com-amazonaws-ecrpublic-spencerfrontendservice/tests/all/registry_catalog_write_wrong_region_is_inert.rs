// SPDX-License-Identifier: CC0-1.0

//! A registry-catalog write signed for a Region that does not perform it is terminal and
//! inert, driven end to end over the wire.
//!
//! The rule under test: an account-global control action is performed in ONE Region of its
//! surface, so a request whose SigV4 credential scope names any other Region is refused
//! HTTP 400 with the service-common `UnsupportedCommandException` ("The action isn't
//! supported in this Region", client, httpError 400 —
//! `smithy_query(query="service_info")` reports it under `common_errors`) BEFORE the record
//! is touched, and the companion read still reports what the last accepted write stored.
//! Being reachable at a Region is not the same as the action being offered there: the
//! second Region AWS publishes an `api.ecr-public` endpoint in serves every
//! Region-independent operation of this service and is still refused this one.
//!
//! This suite is the sequence a client depends on — write the control value from the Region
//! that performs the action, have the wrong-Region write refused, read the control value
//! back from BOTH Regions — rather than the decision function (`registry_catalog_write_regions.rs`)
//! or the offered-Region table (`registry_catalog_write_published_regions.rs`). The table
//! spans the classes the rule generalises over: the other Region this API publishes an
//! endpoint in, Regions from four further partitions, and each in the letter cases a Region
//! name arrives in, crossed with control names of several lengths, scripts and byte widths.

use serde_json::json;
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::API_REGION;

use crate::registry_catalog_write_regions::{display_name, MAX_DISPLAY_NAME_LEN};
use crate::repository_image_lifecycle::{call_signed_for, service};

/// The operation whose Region is part of its contract.
const CATALOG_WRITE: &str = "PutRegistryCatalogData";

/// The companion read, which every Region serves.
const CATALOG_READ: &str = "GetRegistryCatalogData";

/// The refusal AWS answers a caller whose Region does not offer the action: fixed wording
/// whose variable parts are the operation the request named and the Region that performs
/// it. Written out here rather than imported, so this suite pins the text itself.
fn refusal_message(operation: &str) -> String {
    format!("{operation} command is only supported in us-east-1.")
}

/// Regions that do NOT perform this account-global action, each in the letter cases a
/// Region name arrives in: the other Region ECR Public publishes an endpoint in (reachable
/// for every Region-independent call, still refused this one), then Regions from the
/// commercial, China and GovCloud partitions that publish no endpoint at all.
fn regions_that_do_not_perform_the_write() -> Vec<String> {
    [
        "us-west-2",
        "us-east-2",
        "eu-west-3",
        "ap-south-1",
        "cn-north-1",
        "us-gov-west-1",
    ]
    .iter()
    .flat_map(|region| {
        let mut capitalized = region.to_string();
        capitalized[..1].make_ascii_uppercase();
        vec![region.to_string(), region.to_uppercase(), capitalized]
    })
    .collect()
}

/// Control names inside the modelled `RegistryDisplayName` length
/// (`smithy_query(query="shape", shape="RegistryDisplayName")`: `length {min: 0, max: 100}`),
/// spanning length, script and byte width.
fn control_names() -> Vec<(&'static str, String)> {
    vec![
        ("the published lower bound: the empty name", String::new()),
        ("one ascii character", "c".to_string()),
        ("ascii words with separators", "ctl team-7 registry".to_string()),
        ("cjk text", "管理用レジストリ".to_string()),
        ("astral-plane code points", "ctl 🚀".to_string()),
        (
            "the published upper bound in ascii",
            "c".repeat(MAX_DISPLAY_NAME_LEN),
        ),
    ]
}

#[tokio::test]
async fn a_write_from_a_region_that_does_not_perform_it_is_refused_and_changes_nothing() {
    let router = service();

    for (name_case, control) in control_names() {
        // The last accepted write: issued from the Region that performs the action.
        let case = format!("{name_case}, stored from {API_REGION}");
        let (status, written) = call_signed_for(
            &router,
            CATALOG_WRITE,
            json!({"displayName": control}),
            API_REGION,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the control write to be served, got {status} with {written}"
        );
        assert_eq!(
            display_name(&written, &case).as_deref(),
            Some(control.as_str()),
            "case {case:?}: the write must report the name it stored, got {written}"
        );

        for region in regions_that_do_not_perform_the_write() {
            let case = format!("{name_case}, write attempted from {region}");

            let (status, refused) = call_signed_for(
                &router,
                CATALOG_WRITE,
                json!({"displayName": format!("should-not-apply from {region}")}),
                &region,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from a Region that does not perform this \
                 action, got {status} with {refused}"
            );
            let wire_type = refused["__type"].as_str().unwrap_or_default().to_string();
            assert!(
                wire_type.contains("UnsupportedCommandException"),
                "case {case:?}: expected UnsupportedCommandException, got {wire_type:?} in \
                 {refused}"
            );
            let said = refused["message"].as_str().unwrap_or_default().to_string();
            let expected = refusal_message(CATALOG_WRITE);
            assert_eq!(
                said, expected,
                "case {case:?}: expected {expected:?}, got {said:?}"
            );

            // The refusal is pre-mutation: the account-global record still holds the
            // control value, whichever Region the read is signed for.
            for reader in [API_REGION, region.as_str()] {
                let case = format!("{case}, read back from {reader}");
                let (status, got) =
                    call_signed_for(&router, CATALOG_READ, json!({}), reader).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the read to be served, got {status} with {got}"
                );
                assert_eq!(
                    display_name(&got, &case).as_deref(),
                    Some(control.as_str()),
                    "case {case:?}: a refused write must leave the stored name alone, got {got}"
                );
            }
        }
    }
}
