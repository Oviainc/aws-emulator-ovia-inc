// SPDX-License-Identifier: CC0-1.0

//! The account-singleton registry catalog write, driven end to end from every Region a
//! caller can reach this endpoint as.
//!
//! The rule under test: on this emulator as deployed by default — one endpoint standing in
//! for whichever Region a caller reaches it as, the operator having declared none
//! (`state::API_REGION_ENV`) — `PutRegistryCatalogData` is served whatever Region its
//! request names, and a `displayName` within the modelled `RegistryDisplayName` length
//! (`smithy_query(query="shape", shape="RegistryDisplayName")`: `length {min: 0, max: 100}`)
//! is stored verbatim and read back by `GetRegistryCatalogData` signed for any Region, while
//! one past that length is refused with HTTP 400 `InvalidParameterException` naming the
//! member and the modelled bound, before the stored value is touched. (The refusal leg of a
//! deployment that DOES declare which endpoint it stands in for is pinned by
//! `region_gate.rs`.)
//!
//! The tables below span the classes the rule generalises over: every Region a caller can
//! reach this endpoint as — both Regions ECR Public publishes an endpoint in and Regions
//! from five other partitions — each in the letter cases a Region name arrives in, crossed
//! with display names at the published lower bound, inside the range, at the published upper
//! bound and one past it, in several scripts and byte widths.

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::API_REGION;

use crate::region_gate::{every_region, every_region_spelling, region_spellings};
use crate::repository_image_lifecycle::{call_signed_for, service};

/// The modelled upper bound on `RegistryDisplayName`
/// (`smithy_query(query="shape", shape="RegistryDisplayName")`: `length {min: 0, max: 100}`).
const MAX_DISPLAY_NAME_LEN: usize = 100;

/// The clause ECR Public states for a member outside its modelled length: fixed wording
/// whose variable parts are the member the request carried and the bound the model declares
/// for it (`src/service.rs`, `clause`).
fn expected_length_refusal(member: &str, max: usize) -> String {
    format!(
        "Invalid parameter at '{member}' failed to satisfy constraint: \
         'Member must have length less than or equal to {max}'"
    )
}

/// The `displayName` a response reports: `None` when the member is absent.
fn display_name(body: &Value, case: &str) -> Option<String> {
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
async fn the_catalog_write_is_served_for_every_region_a_caller_can_reach_this_endpoint_as() {
    let router = service();

    // Every Region, in every letter case, crossed with every in-range display name: the
    // write answers 200 and echoes what it stored.
    for region in every_region_spelling() {
        let region = region.as_str();
        for (name_case, name) in in_range_display_names() {
            let case = format!("{name_case}, written from {region}");
            let (status, written) = call_signed_for(
                &router,
                "PutRegistryCatalogData",
                json!({"displayName": name}),
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the write to be served, got {status} with {written}"
            );
            assert_eq!(
                display_name(&written, &case).as_deref(),
                Some(name.as_str()),
                "case {case:?}: the write must report the name it stored, got {written}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_written_from_one_region_is_read_back_from_every_other() {
    let router = service();

    // One catalog per account: what a write signed for one Region stored is what a read
    // signed for ANY other Region reports — both legs are served from this one endpoint
    // whichever Region the client that reaches it is configured for.
    let writers = every_region();
    let readers = every_region();
    for writer in writers {
        let name = format!("registry of {writer}");
        let case = format!("a name written from {writer}");
        let (status, written) = call_signed_for(
            &router,
            "PutRegistryCatalogData",
            json!({"displayName": name}),
            writer,
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
    for region in every_region_spelling() {
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
