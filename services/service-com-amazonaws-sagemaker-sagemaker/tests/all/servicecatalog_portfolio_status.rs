// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the account-level Service Catalog portfolio setting:
//! `EnableSagemakerServicecatalogPortfolio`, `DisableSagemakerServicecatalogPortfolio`
//! and `GetSagemakerServicecatalogPortfolioStatus`.
//!
//! The rule under test is not one recorded call but the whole class: the status a Get
//! reports is the member of `SagemakerServicecatalogStatus` the account's LAST toggle
//! named — `Disabled` for an account that has toggled neither — a Get never changes it,
//! and a repeated toggle is a success that changes nothing. The table below spans every
//! toggle sequence up to length three, so the duplicate, the alternation and the
//! never-toggled account are all rows of it.
//!
//! Requests go over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`)
//! through the same router the production binary builds, on the shared [`Emulator`]
//! harness this crate's lifecycle suite defines.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::Emulator;

/// Every value the modelled `SagemakerServicecatalogStatus` enum accepts, taken from
/// the service's own generated catalogue
/// (`ops::types::SAGEMAKER_SERVICECATALOG_STATUS_VALUES`, which mirrors
/// `smithy_query(query="shape",shape="SagemakerServicecatalogStatus")`: Disabled | Enabled).
const STATUS_VALUES: &[&str] =
    service_com_amazonaws_sagemaker_sagemaker::ops::types::SAGEMAKER_SERVICECATALOG_STATUS_VALUES;

/// The two toggles, each with the status it leaves behind.
const TOGGLES: [(&str, &str); 2] = [
    ("EnableSagemakerServicecatalogPortfolio", "Enabled"),
    ("DisableSagemakerServicecatalogPortfolio", "Disabled"),
];

/// The status an account that has never toggled the portfolio reports: Service Catalog
/// is off until `EnableSagemakerServicecatalogPortfolio` turns it on.
const NEVER_TOGGLED: &str = "Disabled";

/// Every sequence of toggles of length `0..=max_len`, as indices into [`TOGGLES`].
fn toggle_sequences(max_len: u32) -> Vec<Vec<usize>> {
    let mut sequences = Vec::new();
    for len in 0..=max_len {
        for mask in 0..(1u32 << len) {
            sequences.push(
                (0..len)
                    .map(|bit| usize::from((mask >> bit) & 1 == 1))
                    .collect(),
            );
        }
    }
    sequences
}

/// The `Status` member of a `GetSagemakerServicecatalogPortfolioStatus` body.
fn status_of(body: &Value) -> String {
    body.get("Status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The sequence as a case label, e.g. `["Enable", "Disable"]`.
fn label(sequence: &[usize]) -> Vec<&'static str> {
    sequence
        .iter()
        .map(|&toggle| TOGGLES[toggle].0)
        .collect::<Vec<_>>()
}

#[tokio::test]
async fn the_portfolio_status_is_the_member_the_last_toggle_named() {
    for sequence in toggle_sequences(3) {
        let case = label(&sequence);
        let emulator = Emulator::new();
        let mut expected = NEVER_TOGGLED;

        for &toggle in &sequence {
            let (operation, leaves) = TOGGLES[toggle];
            let (status, body) = emulator.call(operation, &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} must succeed, got HTTP {status} with {body}"
            );
            // Both toggles model an empty output, so the body carries no member.
            assert!(
                body.is_null() || body == json!({}),
                "case {case:?}: {operation} models an empty output, got {body}"
            );
            expected = leaves;
        }

        // Three consecutive reads: the reported status is the last toggle's member, it
        // is always a member of the modelled enum, and reading never moves it — so the
        // second and third reads see exactly what the first one did.
        for read in 0..3 {
            let body = emulator
                .ok("GetSagemakerServicecatalogPortfolioStatus", &json!({}))
                .await;
            let actual = status_of(&body);
            assert!(
                STATUS_VALUES.contains(&actual.as_str()),
                "case {case:?} read {read}: Status must be one of {STATUS_VALUES:?}, got {actual:?}"
            );
            assert_eq!(
                actual, expected,
                "case {case:?} read {read}: expected Status {expected:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_toggle_is_observable_by_the_very_next_call() {
    // The modelled status enum has no transient member, so neither toggle owes the clock
    // a later transition: a Get issued immediately after a toggle already reports it.
    let emulator = Emulator::new();
    for (operation, leaves) in TOGGLES.into_iter().cycle().take(4) {
        let (status, body) = emulator.call(operation, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {operation}: must succeed, got HTTP {status} with {body}"
        );
        let actual = status_of(
            &emulator
                .ok("GetSagemakerServicecatalogPortfolioStatus", &json!({}))
                .await,
        );
        assert_eq!(
            actual, leaves,
            "case {operation}: the next call must report Status {leaves:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_portfolio_setting_is_account_level_and_takes_no_input() {
    // The three operations model empty inputs, so a body carrying an unmodelled member
    // is still a well-formed request, and the setting is shared by every caller of the
    // one emulator rather than being keyed by anything in the request.
    let emulator = Emulator::new();
    let body = json!({ "Unmodelled": "ignored" });

    for (operation, leaves) in TOGGLES {
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            status, 200,
            "case {operation} with an extra member: expected HTTP 200, got {status} with {response}"
        );

        let actual = status_of(
            &emulator
                .ok("GetSagemakerServicecatalogPortfolioStatus", &body)
                .await,
        );
        assert_eq!(
            actual, leaves,
            "case {operation} with an extra member: expected Status {leaves:?}, got {actual:?}"
        );
    }
}
