// SPDX-License-Identifier: CC0-1.0

//! Which Regions the account-global registry catalog WRITE is offered in, pinned against a
//! hand-written Region table rather than against the service's own decision function.
//!
//! The rule under test: `UnsupportedCommandException` ("The action isn't supported in this
//! Region", client, HTTP 400 — `smithy_query(query="service_info")` reports it under
//! `common_errors`) refuses a caller whose Region does not offer the action, so
//! `PutRegistryCatalogData` is served from EVERY Region this API is published in and is
//! refused only from a Region that carries no ECR Public endpoint at all. A served write
//! stores `displayName` verbatim into the account-singleton catalog and `GetRegistryCatalogData`
//! reports it whatever Region the reader signed for; a refused one leaves the stored name
//! alone.
//!
//! The sibling suite in `registry_catalog_write_regions.rs` drives the same rule through the
//! crate's `serves_region`, so it follows that function wherever it goes. This table is
//! written out by hand instead, from the Region list AWS publishes for this API's
//! `endpointPrefix` (`api.ecr-public`, `smithy_query(query="service_info")`): endpoints in
//! `us-east-1` and `us-west-2`, and none in any other Region. It spans both published
//! Regions in the letter cases a Region name arrives in, and six Regions that publish no ECR
//! Public endpoint, drawn from four partitions (commercial, China, GovCloud) and from the
//! commercial Regions a caller is most likely to be configured for.

use serde_json::json;

use crate::region_gate::{expected_refusal_message, REGION_SCOPED_OPERATION};
use crate::registry_catalog_write_regions::{
    display_name, expected_length_refusal, MAX_DISPLAY_NAME_LEN,
};
use crate::repository_image_lifecycle::{call_signed_for, service};

/// The Region ECR Public's own Region refusal directs a caller to: the primary Region of
/// this API, written out here so the wording is pinned against AWS's text rather than
/// against the crate constant the service formats it from.
const REFUSAL_REGION: &str = "us-east-1";

/// Every Region in the table, with whether this API is published in it.
///
/// Source: the Region list AWS publishes for `api.ecr-public` (two Regions); every other
/// Region name below carries no ECR Public endpoint. The letter case of a Region name is not
/// part of it, so both published Regions appear in upper and mixed case as well.
fn region_table() -> Vec<(&'static str, bool)> {
    vec![
        ("us-east-1", true),
        ("US-EAST-1", true),
        ("Us-East-1", true),
        ("us-west-2", true),
        ("US-WEST-2", true),
        ("Us-West-2", true),
        ("us-east-2", false),
        ("eu-central-1", false),
        ("ap-northeast-1", false),
        ("sa-east-1", false),
        ("cn-northwest-1", false),
        ("us-gov-east-1", false),
    ]
}

/// Display names inside the modelled `RegistryDisplayName` length
/// (`smithy_query(query="shape", shape="RegistryDisplayName")`: `length {min: 0, max: 100}`),
/// spanning length, script and byte width.
fn display_names() -> Vec<(&'static str, String)> {
    vec![
        ("the published lower bound: the empty name", String::new()),
        ("one ascii character", "r".to_string()),
        (
            "ascii words with punctuation",
            "ACME Containers, Inc. (eu)".to_string(),
        ),
        ("cjk text", "パブリックレジストリ".to_string()),
        (
            "the published upper bound in ascii",
            "u".repeat(MAX_DISPLAY_NAME_LEN),
        ),
        (
            "the published upper bound in multi-byte characters",
            "公".repeat(MAX_DISPLAY_NAME_LEN),
        ),
    ]
}

#[tokio::test]
async fn the_catalog_write_is_offered_in_every_region_this_api_is_published_in() {
    let router = service();
    let refusal_message = expected_refusal_message(REGION_SCOPED_OPERATION, REFUSAL_REGION);

    for (region, published) in region_table() {
        for (name_case, name) in display_names() {
            let case = format!("{name_case}, written from {region}");

            // What a refused write must leave in place. The read is not Region-scoped, so
            // it is signed for the Region under test.
            let (status, before) =
                call_signed_for(&router, "GetRegistryCatalogData", json!({}), region).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to be served, got {status} with {before}"
            );
            let held = display_name(&before, &case);

            let (status, written) = call_signed_for(
                &router,
                REGION_SCOPED_OPERATION,
                json!({"displayName": name}),
                region,
            )
            .await;

            if !published {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a Region with no ECR Public endpoint is refused, got \
                     {status} with {written}"
                );
                let wire_type = written["__type"].as_str().unwrap_or_default().to_string();
                assert!(
                    wire_type.contains("UnsupportedCommandException"),
                    "case {case:?}: expected UnsupportedCommandException, got {wire_type:?} in \
                     {written}"
                );
                let said = written["message"].as_str().unwrap_or_default().to_string();
                assert_eq!(
                    said, refusal_message,
                    "case {case:?}: expected {refusal_message:?}, got {said:?}"
                );
                let (status, after) =
                    call_signed_for(&router, "GetRegistryCatalogData", json!({}), region).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the read to be served, got {status} with {after}"
                );
                assert_eq!(
                    display_name(&after, &case),
                    held,
                    "case {case:?}: a refused write must leave the stored name alone, got \
                     {after}"
                );
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: a Region this API is published in serves the write, got \
                 {status} with {written}"
            );
            assert_eq!(
                display_name(&written, &case).as_deref(),
                Some(name.as_str()),
                "case {case:?}: the write must report the name it stored, got {written}"
            );

            // One catalog per account: the write is read back through the companion read
            // signed for a DIFFERENT Region than the one that wrote it.
            for (reader, _) in region_table() {
                let case = format!("{case}, read from {reader}");
                let (status, got) =
                    call_signed_for(&router, "GetRegistryCatalogData", json!({}), reader).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the read to be served, got {status} with {got}"
                );
                assert_eq!(
                    display_name(&got, &case).as_deref(),
                    Some(name.as_str()),
                    "case {case:?}: the read must report the stored name, got {got}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_name_past_the_modelled_bound_is_refused_from_every_published_region() {
    let router = service();
    let expected_message = expected_length_refusal("displayName", MAX_DISPLAY_NAME_LEN);

    // The modelled bound is a property of the member, so it answers in every Region that
    // serves the write — and it answers before anything is stored.
    for (region, published) in region_table() {
        if !published {
            continue;
        }
        let stored = format!("stored from {region}");
        let (status, written) = call_signed_for(
            &router,
            REGION_SCOPED_OPERATION,
            json!({"displayName": stored}),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"an in-range write from {region}\": expected 200, got {status} with {written}"
        );

        for (name_case, name) in [
            (
                "one ascii character past the bound",
                "u".repeat(MAX_DISPLAY_NAME_LEN + 1),
            ),
            (
                "one multi-byte character past the bound",
                "公".repeat(MAX_DISPLAY_NAME_LEN + 1),
            ),
            (
                "far past the bound",
                "u".repeat(MAX_DISPLAY_NAME_LEN * 7),
            ),
        ] {
            let case = format!("{name_case}, written from {region}");
            let (status, refused) = call_signed_for(
                &router,
                REGION_SCOPED_OPERATION,
                json!({"displayName": name}),
                region,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {refused}"
            );
            let wire_type = refused["__type"].as_str().unwrap_or_default().to_string();
            assert!(
                wire_type.contains("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {wire_type:?} in \
                 {refused}"
            );
            let said = refused["message"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                said, expected_message,
                "case {case:?}: expected {expected_message:?}, got {said:?}"
            );

            let (status, got) =
                call_signed_for(&router, "GetRegistryCatalogData", json!({}), region).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to be served, got {status} with {got}"
            );
            assert_eq!(
                display_name(&got, &case).as_deref(),
                Some(stored.as_str()),
                "case {case:?}: a refused write must leave the stored name alone, got {got}"
            );
        }
    }
}
