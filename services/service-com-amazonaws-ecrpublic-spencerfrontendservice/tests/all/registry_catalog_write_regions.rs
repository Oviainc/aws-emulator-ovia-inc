// SPDX-License-Identifier: CC0-1.0

//! The account-singleton registry catalog write, driven end to end from every Region a
//! caller can reach this endpoint as.
//!
//! The rule under test: `PutRegistryCatalogData` is the one ECR Public call whose Region is
//! part of its contract — this account-global control action is performed in a single
//! Region (`state::API_REGION`), so a `displayName` within the modelled
//! `RegistryDisplayName` length
//! (`smithy_query(query="shape", shape="RegistryDisplayName")`: `length {min: 0, max: 100}`)
//! submitted from that Region is stored verbatim and read back by `GetRegistryCatalogData`
//! signed for any Region, while a request signed for ANY other Region — including the
//! second Region this API publishes an endpoint in — is refused HTTP 400
//! `UnsupportedCommandException` and stores nothing, and one whose `displayName` is past
//! the modelled length is refused HTTP 400 `InvalidParameterException` naming the member
//! and the modelled bound, before the stored value is touched.
//!
//! The tables below span the classes the rule generalises over: every Region a caller can
//! reach this endpoint as — both Regions ECR Public publishes an endpoint in and Regions
//! from five other partitions — each in the letter cases a Region name arrives in, crossed
//! with display names at the published lower bound, inside the range, at the published upper
//! bound and one past it, in several scripts and byte widths.

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{serves_region, API_REGION};

use crate::region_gate::{
    every_region, every_region_spelling, expected_refusal_message, region_spellings,
    REGION_SCOPED_OPERATION,
};
use crate::repository_image_lifecycle::{call_signed_for, service};

/// The modelled upper bound on `RegistryDisplayName`
/// (`smithy_query(query="shape", shape="RegistryDisplayName")`: `length {min: 0, max: 100}`).
pub(crate) const MAX_DISPLAY_NAME_LEN: usize = 100;

/// The clause ECR Public states for a member outside its modelled length: fixed wording
/// whose variable parts are the member the request carried and the bound the model declares
/// for it (`src/service.rs`, `clause`).
pub(crate) fn expected_length_refusal(member: &str, max: usize) -> String {
    format!(
        "Invalid parameter at '{member}' failed to satisfy constraint: \
         'Member must have length less than or equal to {max}'"
    )
}

/// The `displayName` a response reports: `None` when the member is absent.
pub(crate) fn display_name(body: &Value, case: &str) -> Option<String> {
    let catalog = &body["registryCatalogData"];
    assert!(
        catalog.is_object(),
        "case {case:?}: expected a registryCatalogData object, got {body}"
    );
    match &catalog["displayName"] {
        Value::Null => None,
        Value::String(name) => Some(name.clone()),
        other => panic!("case {case:?}: expected displayName to be a string, got {other}"),
    }
}

/// Display names inside the modelled range, spanning length, script and byte width: the
/// published lower bound, ascii, latin-1, cjk and astral-plane text, and the published
/// upper bound in one-byte and in multi-byte characters.
fn in_range_display_names() -> Vec<(&'static str, String)> {
    vec![
        ("the published lower bound: the empty name", String::new()),
        ("a single ascii character", "a".to_string()),
        (
            "ascii words with separators",
            "Emulator Public Registry (team-7)".to_string(),
        ),
        ("latin-1 accents", "Café ACME".to_string()),
        ("cjk text", "日本語のレジストリ".to_string()),
        ("astral-plane code points", "containers 🚀🐳".to_string()),
        (
            "the published upper bound: 100 ascii characters",
            "n".repeat(MAX_DISPLAY_NAME_LEN),
        ),
        (
            "the published upper bound in multi-byte characters",
            "名".repeat(MAX_DISPLAY_NAME_LEN),
        ),
    ]
}

/// Display names one or more code points past the modelled bound, in one-byte and
/// multi-byte characters.
fn over_bound_display_names() -> Vec<(&'static str, String)> {
    vec![
        (
            "one ascii character past the bound",
            "n".repeat(MAX_DISPLAY_NAME_LEN + 1),
        ),
        (
            "one multi-byte character past the bound",
            "名".repeat(MAX_DISPLAY_NAME_LEN + 1),
        ),
        ("far past the bound", "n".repeat(MAX_DISPLAY_NAME_LEN * 10)),
    ]
}

#[tokio::test]
async fn the_catalog_write_is_served_for_the_region_that_performs_it_and_refused_for_every_other() {
    let router = service();
    let refusal_message = expected_refusal_message(REGION_SCOPED_OPERATION, API_REGION);

    // Every Region, in every letter case, crossed with every in-range display name: the
    // write answers 200 and echoes what it stored for the one Region that performs it, and
    // is refused for every other — published endpoint or not — without storing anything.
    for region in every_region_spelling() {
        let region = region.as_str();
        for (name_case, name) in in_range_display_names() {
            let case = format!("{name_case}, written from {region}");
            // What a refused write must leave in place: whatever the catalog holds now.
            let (status, before) =
                call_signed_for(&router, "GetRegistryCatalogData", json!({}), API_REGION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to be served, got {status} with {before}"
            );
            let held = display_name(&before, &case);

            let (status, written) = call_signed_for(
                &router,
                "PutRegistryCatalogData",
                json!({"displayName": name}),
                region,
            )
            .await;
            if serves_region(region) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the write to be served, got {status} with {written}"
                );
                assert_eq!(
                    display_name(&written, &case).as_deref(),
                    Some(name.as_str()),
                    "case {case:?}: the write must report the name it stored, got {written}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a Region that does not perform the \
                 write, got {status} with {written}"
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

            // The refusal is pre-mutation: the account-global record still holds what the
            // last accepted write left there.
            let (status, after) =
                call_signed_for(&router, "GetRegistryCatalogData", json!({}), API_REGION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to be served, got {status} with {after}"
            );
            assert_eq!(
                display_name(&after, &case),
                held,
                "case {case:?}: a refused write must leave the stored name alone, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_written_from_the_serving_region_is_read_back_from_every_other() {
    let router = service();

    // One catalog per account: what the write stored is what a read signed for ANY Region
    // reports — the read is served from this one endpoint whichever Region the client that
    // reaches it is configured for.
    let readers = every_region();
    for (name_case, name) in in_range_display_names() {
        let case = format!("{name_case}, written from {API_REGION}");
        let (status, written) = call_signed_for(
            &router,
            "PutRegistryCatalogData",
            json!({"displayName": name}),
            API_REGION,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write to be served, got {status} with {written}"
        );

        for reader in &readers {
            for spelling in region_spellings(reader) {
                let case = format!("{case}, read from {spelling}");
                let (status, got) =
                    call_signed_for(&router, "GetRegistryCatalogData", json!({}), &spelling).await;
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
async fn a_name_past_the_modelled_bound_is_refused_and_stores_nothing() {
    let router = service();

    // The last accepted write is what every refused write must leave in place.
    let stored = "n".repeat(MAX_DISPLAY_NAME_LEN);
    let (status, written) = call_signed_for(
        &router,
        "PutRegistryCatalogData",
        json!({"displayName": stored}),
        API_REGION,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the write at the published bound\": expected 200, got {status} with {written}"
    );

    let expected_message = expected_length_refusal("displayName", MAX_DISPLAY_NAME_LEN);
    // The bound is a property of the member, so it is asserted on the Region that performs
    // the write: the modelled constraint is what refuses the request there.
    for region in region_spellings(API_REGION) {
        let region = region.as_str();
        for (name_case, name) in over_bound_display_names() {
            let case = format!("{name_case}, written from {region}");
            let (status, refused) = call_signed_for(
                &router,
                "PutRegistryCatalogData",
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
            let actual = refused["message"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                actual, expected_message,
                "case {case:?}: expected {expected_message:?}, got {actual:?}"
            );

            // The refusal happens before anything is stored, so the catalog still carries
            // the last accepted name — whichever Region the read is signed for.
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
