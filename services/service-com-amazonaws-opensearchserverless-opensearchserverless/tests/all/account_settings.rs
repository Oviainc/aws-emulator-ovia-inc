// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless **account settings**: `GetAccountSettings` and the
//! `UpdateAccountSettings` write that proves what it reports, end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: `GetAccountSettings` reports the account's stored OCU capacity
//! limits — the documented defaults until an update writes them, then exactly the integer
//! values the last `UpdateAccountSettings` named, with a limit that update did not name
//! left at its stored value — and reporting them changes nothing a later caller sees.
//!
//! The tables below span what the rule generalises over: both `CapacityLimits` members,
//! each named alone, named together and omitted, values from the published minimum up to
//! the largest the modelled `integer` carries, the published bound and one past it on each
//! member, and the Region the call is signed for (the settings belong to the account, not
//! to one Region).

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, call_in, emulator};

/// The capacity limits an account carries before anything configures it, from AWS's
/// "Managing capacity limits for Amazon OpenSearch Serverless" (the page
/// `CapacityLimits`' own documentation links): 10 OCUs for indexing and 10 for search.
const DEFAULT_MAX_INDEXING: i64 = 10;
const DEFAULT_MAX_SEARCH: i64 = 10;

/// The least capacity either limit may be set to, from the model's own `@range(min: 2)` on
/// `IndexingCapacityValue` and `SearchCapacityValue`
/// (`smithy_query(query="shape", shape="IndexingCapacityValue,SearchCapacityValue")`).
const MIN_CAPACITY: i64 = 2;

/// Regions to call in, so a row proves the settings are the ACCOUNT's rather than one
/// Region's. Any Region the request may be signed for serves; these four are the ones this
/// crate's collection suite already spans.
const REGIONS: [&str; 4] = ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"];

/// The `capacityLimits` pair `answer` reports under `path`, as `(indexing, search)`.
fn limits_of(answer: &Value, path: &str) -> (Option<i64>, Option<i64>) {
    let limits = at(answer, path);
    (
        at(limits, "maxIndexingCapacityInOCU").as_i64(),
        at(limits, "maxSearchCapacityInOCU").as_i64(),
    )
}

/// `GetAccountSettings`' reported limits, asserting the read was accepted.
async fn get_limits(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> (Option<i64>, Option<i64>) {
    get_limits_in(router, REGIONS[0], case).await
}

/// [`get_limits`], signed for `region`.
async fn get_limits_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    case: &str,
) -> (Option<i64>, Option<i64>) {
    let (status, code, answer) = call_in(router, region, "GetAccountSettings", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case}: GetAccountSettings in {region} expected 200, got {status} {code} with \
         {answer}"
    );
    limits_of(&answer, "accountSettingsDetail.capacityLimits")
}

/// Set the account's limits to `body`, asserting the update was accepted, and answer the
/// limits it reports.
async fn update_limits(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    body: Value,
) -> (Option<i64>, Option<i64>) {
    let (status, code, answer) = call(router, "UpdateAccountSettings", body).await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateAccountSettings expected 200, got {status} {code} with {answer}"
    );
    limits_of(&answer, "accountSettingsDetail.capacityLimits")
}

// ---------------------------------------------------------------------------
// GetAccountSettings: the account's stored limits, reported without changing them.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_account_settings_reports_the_stored_limits_and_changes_nothing() {
    let router = emulator();

    // An account nobody configured carries the documented defaults, and every repeat of
    // the read answers them again: a report is not a write.
    for probe in 0..3 {
        let case = format!("GetAccountSettings(probe {probe}, nothing written)");
        assert_eq!(
            get_limits(&router, &case).await,
            (Some(DEFAULT_MAX_INDEXING), Some(DEFAULT_MAX_SEARCH)),
            "case {case}: expected the documented defaults \
             ({DEFAULT_MAX_INDEXING}, {DEFAULT_MAX_SEARCH})"
        );
    }

    // The same after a write, so the read tracks the store rather than a constant, and
    // still after an unrelated write to this service's state (a collection), which the
    // settings neither bound nor are consumed by.
    let written = (MIN_CAPACITY + 4, MIN_CAPACITY + 10);
    update_limits(
        &router,
        "settings written",
        json!({ "capacityLimits": {
            "maxIndexingCapacityInOCU": written.0,
            "maxSearchCapacityInOCU": written.1,
        }}),
    )
    .await;
    let (status, code, answer) = call(
        &router,
        "CreateCollection",
        json!({ "name": "settings-probe", "type": "SEARCH" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection(beside the account settings): expected 200, got {status} {code} \
         with {answer}"
    );
    for probe in 0..3 {
        let case = format!("GetAccountSettings(probe {probe}, after a write and a create)");
        assert_eq!(
            get_limits(&router, &case).await,
            (Some(written.0), Some(written.1)),
            "case {case}: expected the written limits {written:?}"
        );
    }

    // The settings belong to the account, so the Region the call is signed for selects
    // nothing: every Region reports the one stored pair.
    for region in REGIONS {
        let case = format!("GetAccountSettings(in {region})");
        assert_eq!(
            get_limits_in(&router, region, &case).await,
            (Some(written.0), Some(written.1)),
            "case {case}: the account's settings must not depend on the Region"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateAccountSettings: every limit the request names is stored exactly, and one it
// omits keeps its stored value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_account_settings_stores_each_named_limit_exactly() {
    let router = emulator();

    // (case, capacityLimits the request names, expected stored pair afterwards). The rows
    // differ along what the rule generalises over: which members are named, and the value
    // each carries — the published minimum, an asymmetric pair, a four-digit value, and
    // the largest value the modelled `integer` holds, whose digits must survive the round
    // trip unrounded.
    let cases: Vec<(&str, Value, (i64, i64))> = vec![
        (
            "both members at the published minimum",
            json!({
                "maxIndexingCapacityInOCU": MIN_CAPACITY,
                "maxSearchCapacityInOCU": MIN_CAPACITY,
            }),
            (MIN_CAPACITY, MIN_CAPACITY),
        ),
        (
            "an asymmetric pair",
            json!({ "maxIndexingCapacityInOCU": 7, "maxSearchCapacityInOCU": 23 }),
            (7, 23),
        ),
        (
            "only the indexing limit named",
            json!({ "maxIndexingCapacityInOCU": 100 }),
            (100, 23),
        ),
        (
            "only the search limit named",
            json!({ "maxSearchCapacityInOCU": 1_000 }),
            (100, 1_000),
        ),
        (
            "the largest value the modelled integer carries",
            json!({
                "maxIndexingCapacityInOCU": i64::from(i32::MAX),
                "maxSearchCapacityInOCU": i64::from(i32::MAX) - 1,
            }),
            (i64::from(i32::MAX), i64::from(i32::MAX) - 1),
        ),
        (
            "back to the defaults",
            json!({
                "maxIndexingCapacityInOCU": DEFAULT_MAX_INDEXING,
                "maxSearchCapacityInOCU": DEFAULT_MAX_SEARCH,
            }),
            (DEFAULT_MAX_INDEXING, DEFAULT_MAX_SEARCH),
        ),
    ];

    for (case, limits, expected) in cases {
        let reported = update_limits(&router, case, json!({ "capacityLimits": limits })).await;
        assert_eq!(
            reported,
            (Some(expected.0), Some(expected.1)),
            "case UpdateAccountSettings({case}): expected the update to report {expected:?}"
        );
        assert_eq!(
            get_limits(&router, case).await,
            (Some(expected.0), Some(expected.1)),
            "case GetAccountSettings(after {case}): expected the stored limits {expected:?}"
        );
    }

    // A request that names no `capacityLimits` at all changes neither limit: both members
    // of `CapacityLimits` are optional, and so is the member itself.
    let before = get_limits(&router, "before the empty update").await;
    let reported = update_limits(&router, "no capacityLimits named", json!({})).await;
    assert_eq!(
        reported, before,
        "case UpdateAccountSettings(no capacityLimits named): expected the stored limits \
         {before:?} reported back"
    );
    assert_eq!(
        get_limits(&router, "after the empty update").await,
        before,
        "case GetAccountSettings(after an update naming no limits): expected {before:?}"
    );

    // An update signed for another Region writes the one account-level pair, which every
    // Region then reports.
    let elsewhere = (MIN_CAPACITY, MIN_CAPACITY + 8);
    let (status, code, answer) = call_in(
        &router,
        REGIONS[2],
        "UpdateAccountSettings",
        json!({ "capacityLimits": {
            "maxIndexingCapacityInOCU": elsewhere.0,
            "maxSearchCapacityInOCU": elsewhere.1,
        }}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateAccountSettings(in {}): expected 200, got {status} {code} with {answer}",
        REGIONS[2]
    );
    for region in REGIONS {
        let case = format!("GetAccountSettings(in {region}, written in {})", REGIONS[2]);
        assert_eq!(
            get_limits_in(&router, region, &case).await,
            (Some(elsewhere.0), Some(elsewhere.1)),
            "case {case}: expected the account-level limits {elsewhere:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateAccountSettings: each member's published bound, and one past it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_account_settings_refuses_a_capacity_below_the_published_minimum() {
    let router = emulator();

    // Each member on its own, at the published bound (accepted) and below it (refused):
    // one step under the bound, zero, a negative value, and the smallest the modelled
    // `integer` carries.
    for member in ["maxIndexingCapacityInOCU", "maxSearchCapacityInOCU"] {
        for value in [MIN_CAPACITY - 1, 0, -1, i64::from(i32::MIN)] {
            let case = format!("UpdateAccountSettings({member} = {value})");
            let (status, code, answer) = call(
                &router,
                "UpdateAccountSettings",
                json!({ "capacityLimits": { member: value } }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case}: expected 400 ValidationException, got {status} with {answer}"
            );
        }

        // The bound itself is accepted and stored, so the refusal is about being BELOW it.
        let case = format!("UpdateAccountSettings({member} = {MIN_CAPACITY})");
        let reported = update_limits(
            &router,
            &case,
            json!({ "capacityLimits": { member: MIN_CAPACITY } }),
        )
        .await;
        let stored = if member == "maxIndexingCapacityInOCU" {
            reported.0
        } else {
            reported.1
        };
        assert_eq!(
            stored,
            Some(MIN_CAPACITY),
            "case {case}: expected the published minimum to be stored, got {reported:?}"
        );
    }

    // A refused update left the account exactly where the accepted ones put it.
    assert_eq!(
        get_limits(&router, "after the refusals").await,
        (Some(MIN_CAPACITY), Some(MIN_CAPACITY)),
        "case GetAccountSettings(after refused updates): a refusal must store nothing"
    );
}
