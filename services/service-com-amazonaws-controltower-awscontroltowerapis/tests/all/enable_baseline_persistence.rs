// SPDX-License-Identifier: CC0-1.0

//! What `EnableBaseline` PERSISTS, read back end to end over HTTP.
//!
//! Drives the calls through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: an accepted
//! enablement is a resource computed from the request and kept, so the ARN the call
//! returned resolves through `GetEnabledBaseline` to an `EnabledBaseline` reporting the
//! baseline, the version, the target and the parameters that call carried — in the order
//! supplied — under its own `EnablementStatusSummary`, whose `lastOperationIdentifier` is
//! the `operationIdentifier` the call returned and whose status is `UNDER_CHANGE` until
//! the clock makes that operation due and `SUCCEEDED` after, and whose `tags` member is
//! what `ListTagsForResource` reports for that ARN. Reading the resource changes nothing a
//! later reader sees.
//!
//! The ARN is minted in the region the request was signed for, which is the second test
//! below: the credential scope names the region, so the resource a client is handed back
//! is addressable in the region it called.
//!
//! The rows come from the `enablements()` table this crate's `baseline_operations` suite
//! already drives the operation log with — several catalog baselines, each at an offered
//! version, with the parameter list absent, empty and carrying a declared key, and with
//! the tag map absent, empty and populated — so no value is written out by hand here.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::baseline_operations::{enable, enablements, get_operation, member, Enablement};
use crate::landing_zone_lifecycle::{call, call_in_region, past_the_settle_span, set_up_emulator, Answer};
use crate::resource_tags::{read_tags, reported_tags};

/// POST `/get-enabled-baseline` and hand back the answer plus its `enabledBaselineDetails`
/// member, which is what every assertion below reads.
async fn get_enabled_baseline(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await;
    let details = answer.get("enabledBaselineDetails").clone();
    (answer, details)
}

/// A member of a JSON object, as a `&str`.
fn text<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// The `parameters` of an `EnabledBaselineDetails` or of a request, as a list. An absent
/// member reads as the empty list, which is what "no parameters are applied" means, so a
/// row that omits `parameters` and a row that sends `[]` compare the same way.
fn parameter_list(value: &serde_json::Value) -> serde_json::Value {
    value
        .get("parameters")
        .cloned()
        .filter(|parameters| !parameters.is_null())
        .unwrap_or_else(|| serde_json::json!([]))
}

/// The `parameters` a row carries, in the form `GetEnabledBaseline` reports them.
fn row_parameters(row: &Enablement) -> serde_json::Value {
    row.parameters
        .clone()
        .unwrap_or_else(|| serde_json::json!([]))
}

/// The `tags` a row carries, as a key-ordered map, so a case compares SETS rather than
/// JSON object order. A row that omits the member carries no tags.
fn row_tags(row: &Enablement) -> BTreeMap<String, String> {
    row.tags
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .map(|members| {
            members
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: what `EnableBaseline` accepted is what a later reporting call reads — the ARN it
/// returned resolves to an `EnabledBaseline` carrying that call's baseline, version, target
/// and parameters, its operation identifier, and the tags it was given; the status is the
/// transient `UNDER_CHANGE` until the clock makes the enablement's operation due and
/// `SUCCEEDED` after, and no read changes any of it.
#[tokio::test]
async fn an_accepted_enablement_reports_back_every_member_it_accepted() {
    let router = set_up_emulator().await;
    let rows = enablements();
    let mut enabled_resources: Vec<(&'static str, String, String)> = Vec::new();

    for row in &rows {
        let enabled = enable(&router, row).await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: expected 200, got {} body {:?}",
            row.case, enabled.status, enabled.body
        );
        let arn = enabled.string("arn");
        let operation_identifier = enabled.string("operationIdentifier");

        let (read, details) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the returned ARN to resolve, got {} body {:?}",
            row.case, read.status, read.body
        );
        assert_eq!(
            (
                text(&details, "arn"),
                text(&details, "baselineIdentifier"),
                text(&details, "baselineVersion"),
                text(&details, "targetIdentifier"),
            ),
            (
                Some(arn.as_str()),
                Some(row.baseline_identifier),
                Some(row.baseline_version),
                Some(row.target_identifier),
            ),
            "case {:?}: expected the enablement to report what the call carried, got \
             {details:?}",
            row.case
        );
        assert_eq!(
            parameter_list(&details),
            row_parameters(row),
            "case {:?}: expected the applied parameters to be the ones supplied, in order, \
             got {details:?}",
            row.case
        );

        let summary = details
            .get("statusSummary")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            (
                text(&summary, "status"),
                text(&summary, "lastOperationIdentifier"),
            ),
            (Some("UNDER_CHANGE"), Some(operation_identifier.as_str())),
            "case {:?}: expected UNDER_CHANGE under the operation the call returned while \
             the enablement is in flight, got {summary:?}",
            row.case
        );

        // The tags the enablement was created with are the resource's tags, reported by
        // the tagging operation rather than invented by it.
        let tags = read_tags(&router, &arn).await;
        assert_eq!(
            tags.status, 200,
            "case {:?}: expected the enablement's tags to be readable, got {} body {:?}",
            row.case, tags.status, tags.body
        );
        assert_eq!(
            reported_tags(&tags),
            row_tags(row),
            "case {:?}: expected the tags the enablement was created with, got {:?}",
            row.case,
            tags.body
        );

        // A read reports the resource; it does not disturb it.
        let (_, reread) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            reread, details,
            "case {:?}: re-reading an enablement must report the same resource, got \
             {reread:?}",
            row.case
        );

        enabled_resources.push((row.case, arn, operation_identifier));
    }

    // The clock makes every enablement's operation due; nothing sleeps server-side, the
    // wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for ((case, arn, operation_identifier), row) in enabled_resources.iter().zip(&rows) {
        let (read, details) = get_enabled_baseline(&router, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the enablement to still resolve, got {} body {:?}",
            read.status, read.body
        );
        let summary = details
            .get("statusSummary")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            (
                text(&summary, "status"),
                text(&summary, "lastOperationIdentifier"),
            ),
            (Some("SUCCEEDED"), Some(operation_identifier.as_str())),
            "case {case:?}: expected SUCCEEDED under the same operation once the \
             enablement is due, got {summary:?}"
        );

        // Settling the status does not rewrite what the call persisted.
        assert_eq!(
            (
                text(&details, "baselineIdentifier"),
                text(&details, "baselineVersion"),
                text(&details, "targetIdentifier"),
            ),
            (
                Some(row.baseline_identifier),
                Some(row.baseline_version),
                Some(row.target_identifier),
            ),
            "case {case:?}: the persisted members must not change as the status does, got \
             {details:?}"
        );
        assert_eq!(
            parameter_list(&details),
            row_parameters(row),
            "case {case:?}: the applied parameters must not change as the status does, got \
             {details:?}"
        );

        // The operation the enablement names is the one the log holds for it.
        let (resolved, operation) = get_operation(&router, operation_identifier).await;
        assert_eq!(
            resolved.status, 200,
            "case {case:?}: expected the named operation to resolve, got {} body {:?}",
            resolved.status, resolved.body
        );
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "statusMessage"),
            ),
            (Some("ENABLE_BASELINE"), None),
            "case {case:?}: expected the record of this enable, with no failure message, \
             got {operation:?}"
        );
    }
}

/// RULE (the region side of the same rule): the resource is minted in the region the
/// request was signed for — the SigV4 credential scope names it — so the ARN a client is
/// handed back addresses the region it called, and resolves there.
#[tokio::test]
async fn an_enablement_is_minted_in_the_region_the_request_was_signed_for() {
    let row = enablements()
        .into_iter()
        .next()
        .expect("the enablement table has at least one row");

    // Rows differ along the only thing this rule generalises over: the signed region,
    // across three AWS partitions-worth of region names.
    for region in ["us-west-2", "eu-central-1", "ap-southeast-2"] {
        let router = set_up_emulator().await;
        let enabled = call_in_region(
            &router,
            region,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": row.baseline_identifier,
                "baselineVersion": row.baseline_version,
                "targetIdentifier": row.target_identifier,
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {region:?}: expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );

        let arn = enabled.string("arn");
        assert!(
            arn.starts_with(&format!("arn:aws:controltower:{region}:"))
                && arn.contains(":enabledbaseline/"),
            "case {region:?}: expected an EnabledBaseline ARN in the signed region, got \
             {arn:?}"
        );

        let read = call_in_region(
            &router,
            region,
            "/get-enabled-baseline",
            serde_json::json!({ "enabledBaselineIdentifier": arn }),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {region:?}: expected the minted ARN to resolve in that region, got {} \
             body {:?}",
            read.status, read.body
        );
        assert_eq!(
            text(read.get("enabledBaselineDetails"), "arn"),
            Some(arn.as_str()),
            "case {region:?}: expected the enablement to report its own ARN, got {:?}",
            read.body
        );
    }
}
