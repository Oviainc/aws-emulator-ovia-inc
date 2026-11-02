// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how `BatchGetAssetPropertyValueHistory` resolves the
//! `(assetId, propertyId)` pair each of its entries carries, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: an entry addressed by `(assetId, propertyId)`
//! resolves against EVERY property the named asset carries — the ones
//! materialised from its asset model's own top-level definitions AND the ones
//! that model's composite models contribute, both addressed by the same pair —
//! so such an entry is answered as a success entry carrying the stored samples
//! of that property, windowed, quality-filtered and ordered exactly as the
//! entry's own members ask. Only a pair naming no property of the asset is an
//! error entry, with the `ResourceNotFoundException` error code; the same
//! resolution decides the `completionStatus` of an entry a previous page
//! already processed, so a composite model's property is reported SUCCESS
//! there too.
//!
//! Each table's rows differ along WHERE the property is declared (on the asset
//! model itself, on a composite model declared with it, on one added
//! afterwards, on one nested inside another, and on the component model a
//! component-model-based composite model reuses) and along the property's data
//! type, with the asset's own property as the control row: every row must be
//! served the same way. The inner table varies what the entry itself asks for
//! — which window bounds it carries and where they land, which `Quality` it
//! filters on and which `TimeOrdering` it wants.
//!
//! The wire harness (`router`, `call`, …) lives in the `asset_lifecycle`
//! suite, the fixture spanning every place an asset carries a property in
//! `composite_model_property_streams`, the batch-entry lookups in
//! `batch_get_latest_values`, and the clock anchoring and this operation's
//! success-entry reading in `batch_property_value_history`; this suite reuses
//! all four rather than keeping a copy.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, router};
use crate::batch_get_latest_values::{count, find_entry};
use crate::batch_property_value_history::{anchored, relative, reported_seconds};
use crate::composite_model_property_streams::{boiler, numeric, variant, Row};

/// Where `BatchGetAssetPropertyValueHistory` is bound: `POST
/// /properties/batch/history`, from `smithy_query(query="operation",
/// operation="BatchGetAssetPropertyValueHistory")`.
const BATCH_HISTORY: &str = "/properties/batch/history";

/// Where `BatchPutAssetPropertyValue` is bound, used here to seed the samples
/// the batch read must report.
const BATCH_PUT: &str = "/properties";

/// A `CustomID`-shaped id no create ever minted, for the rows that must be
/// refused.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The `BatchGetAssetPropertyValueHistoryErrorCode` an entry naming no property
/// of the asset reports.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The seconds, after the shared table epoch, of the samples every row is
/// seeded with.
const SAMPLE_SECONDS: [i64; 4] = [100, 200, 300, 400];

/// The `Quality` each of those samples is written with, the last without one at
/// all — which IoT SiteWise counts as GOOD. Source:
/// `smithy_query(query="shape", shape="Quality")` — GOOD | BAD | UNCERTAIN.
const SAMPLE_QUALITIES: [Option<&str>; 4] = [Some("GOOD"), Some("BAD"), Some("UNCERTAIN"), None];

/// The reading row `at` stores at table second `seconds`.
///
/// Distinct per row AND per second, and integral so an INTEGER property keeps
/// it too: a handler that answered one row's series for another, or one
/// sample's value for its neighbour's, is caught rather than agreeing with
/// itself.
fn reading(at: usize, seconds: i64) -> f64 {
    (1_000 * (at as i64 + 1) + seconds) as f64
}

/// Every place an asset carries a property, as the shared fixture names its
/// rows: the dimension this suite's outer table spans.
///
/// Checked by NAME rather than by row count, so a fixture that dropped the
/// nested or the component-model row — the two places this suite exists to
/// cover — fails here instead of quietly narrowing the table.
const REQUIRED_CASES: [&str; 6] = [
    "the asset's own property",
    "a property of a composite model declared with the asset model",
    "a property of a composite model added to the model afterwards",
    "an INTEGER property of a composite model",
    "a property of a composite model nested inside another",
    "a property a component model contributes through the composite model that reuses it",
];

/// Assert the fixture still spans every place an asset carries a property, and
/// both `PropertyDataType`s its rows vary along.
fn assert_fixture_spans_every_place(rows: &[Row]) {
    let cases: Vec<&str> = rows.iter().map(|row| row.case).collect();
    for required in REQUIRED_CASES {
        assert!(
            cases.contains(&required),
            "case {required:?}: the fixture must span every place an asset carries a property, got {cases:?}"
        );
    }
    for data_type in ["DOUBLE", "INTEGER"] {
        assert!(
            rows.iter().any(|row| row.data_type == data_type),
            "case {data_type:?}: the table must span both data types, got {:?}",
            rows.iter().map(|row| row.data_type).collect::<Vec<_>>()
        );
    }
}

/// Seed row `at`'s property with one sample per [`SAMPLE_SECONDS`].
async fn seed(router: &MultiServiceRouter, asset_id: &str, at: usize, row: &Row) {
    let values: Vec<Value> = SAMPLE_SECONDS
        .iter()
        .zip(SAMPLE_QUALITIES)
        .map(|(seconds, quality)| {
            let mut sample = json!({
                "value": variant(row.data_type, reading(at, *seconds)),
                "timestamp": {"timeInSeconds": anchored(*seconds)},
            });
            if let Some(quality) = quality {
                sample["quality"] = json!(quality);
            }
            sample
        })
        .collect();
    let (status, body) = call(
        router,
        "POST",
        BATCH_PUT,
        Some(json!({"entries": [{
            "entryId": "seed",
            "assetId": asset_id,
            "propertyId": row.property_id,
            "propertyValues": values,
        }]})),
    )
    .await;
    let case = row.case;
    assert_eq!(status, 200, "case {case:?}: the write got {status} {body}");
    assert_eq!(
        count(&body["errorEntries"]),
        0,
        "case {case:?}: every sample of a property of this asset must be accepted, got {body}"
    );
}

/// Assert every sample a success entry reports still carries the value row `at`
/// stored at that sample's second.
///
/// The sibling suite's own sample check reads a value fixed by the second
/// alone; this one is keyed by ROW as well, which is what catches one row's
/// series being answered for another.
fn assert_samples_intact(case: &str, at: usize, entry: &Value) {
    for sample in entry["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {entry}"))
    {
        let seconds = relative(
            sample["timestamp"]["timeInSeconds"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: a sample with no timeInSeconds in {sample}")),
        );
        let expected = reading(at, seconds);
        let actual = numeric(&sample["value"]);
        assert!(
            actual.is_some_and(|actual| (actual - expected).abs() < 1e-9),
            "case {case:?}: the sample at {seconds} must keep its own value {expected}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_entry_naming_any_property_the_asset_carries_is_answered_from_the_store() {
    let router = router();
    let (asset_id, rows) = boiler(&router, "BatchHistComposite").await;
    assert_fixture_spans_every_place(&rows);

    for (at, row) in rows.iter().enumerate() {
        seed(&router, &asset_id, at, row).await;
    }

    for (at, row) in rows.iter().enumerate() {
        // What the ENTRY asks of the property it names: the window bounds it
        // carries and where they land, the quality it filters on and the
        // ordering it wants. Every variant rides in ONE request, so a handler
        // that applied one entry's members to another would fail here.
        let all: Vec<i64> = SAMPLE_SECONDS.to_vec();
        let variants: Vec<(&str, Value, Vec<i64>)> = vec![
            (
                "no bounds, no filter",
                json!({"entryId": "plain", "assetId": asset_id, "propertyId": row.property_id}),
                all.clone(),
            ),
            (
                "startDate is exclusive, landing on a sample",
                json!({"entryId": "startonsample", "assetId": asset_id,
                       "propertyId": row.property_id, "startDate": anchored(100)}),
                vec![200, 300, 400],
            ),
            (
                "endDate is inclusive, landing on a sample",
                json!({"entryId": "endonsample", "assetId": asset_id,
                       "propertyId": row.property_id, "endDate": anchored(300)}),
                vec![100, 200, 300],
            ),
            (
                "both bounds, DESCENDING",
                json!({"entryId": "bothbounds", "assetId": asset_id,
                       "propertyId": row.property_id, "startDate": anchored(100),
                       "endDate": anchored(300), "timeOrdering": "DESCENDING"}),
                vec![300, 200],
            ),
            (
                "qualities=BAD",
                json!({"entryId": "badonly", "assetId": asset_id,
                       "propertyId": row.property_id, "qualities": ["BAD"]}),
                vec![200],
            ),
            (
                "qualities=GOOD, which an unqualified write counts as",
                json!({"entryId": "goodonly", "assetId": asset_id,
                       "propertyId": row.property_id, "qualities": ["GOOD"]}),
                vec![100, 400],
            ),
            (
                "a window holding no sample",
                json!({"entryId": "emptywindow", "assetId": asset_id,
                       "propertyId": row.property_id, "startDate": anchored(200),
                       "endDate": anchored(200)}),
                vec![],
            ),
        ];

        // The two pairs that name nothing: a propertyId this asset carries no
        // property under, and an assetId no asset stands under.
        let mut entries: Vec<Value> = variants.iter().map(|(_, entry, _)| entry.clone()).collect();
        entries.push(json!({
            "entryId": "absent-property",
            "assetId": asset_id,
            "propertyId": ABSENT_ID,
        }));
        entries.push(json!({
            "entryId": "absent-asset",
            "assetId": ABSENT_ID,
            "propertyId": row.property_id,
        }));

        let request = json!({"entries": entries});
        let (status, body) = call(&router, "POST", BATCH_HISTORY, Some(request.clone())).await;
        let case = row.case;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAssetPropertyValueHistory got {status} {body}"
        );
        assert_eq!(
            count(&body["skippedEntries"]),
            0,
            "case {case:?}: no previous page processed anything, so skippedEntries must be empty: {body}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: one page answered every entry, so there must be no nextToken: {body}"
        );
        assert_eq!(
            count(&body["successEntries"]) + count(&body["errorEntries"]),
            entries.len(),
            "case {case:?}: every entry must be answered exactly once, got {body}"
        );

        for (what, entry, expected) in &variants {
            let id = entry["entryId"].as_str().expect("an entryId");
            let case = format!("{case} / {what}");
            assert!(
                find_entry(&body["errorEntries"], id).is_none(),
                "case {case:?}: this pair names a property of the asset, so it must not be an error entry, got {body}"
            );
            let success = find_entry(&body["successEntries"], id).unwrap_or_else(|| {
                panic!("case {case:?}: expected a success entry {id}, got {body}")
            });
            assert_eq!(
                &reported_seconds(&case, success),
                expected,
                "case {case:?}: expected the samples at {expected:?}, got {success}"
            );
            assert_samples_intact(&case, at, success);
        }

        for (what, id) in [
            (
                "a propertyId the asset carries no property under",
                "absent-property",
            ),
            ("an assetId no asset stands under", "absent-asset"),
        ] {
            let case = format!("{case} / {what}");
            assert!(
                find_entry(&body["successEntries"], id).is_none(),
                "case {case:?}: {id} names nothing, so it must not be a success entry, got {body}"
            );
            let failed = find_entry(&body["errorEntries"], id).unwrap_or_else(|| {
                panic!("case {case:?}: expected an error entry {id}, got {body}")
            });
            assert_eq!(
                failed["errorCode"].as_str(),
                Some(NOT_FOUND),
                "case {case:?}: expected the error code {NOT_FOUND}, got {failed}"
            );
            assert!(
                failed["errorMessage"]
                    .as_str()
                    .is_some_and(|message| message.contains(ABSENT_ID)),
                "case {case:?}: the message must name the id that resolved to nothing, got {failed}"
            );
        }

        // Reporting changes nothing a later caller sees: the same request
        // answers the same way.
        let (again_status, again) = call(&router, "POST", BATCH_HISTORY, Some(request)).await;
        assert_eq!(
            again_status, status,
            "case {case:?}: expected {status} again, got {again_status} {again}"
        );
        assert_eq!(
            again, body,
            "case {case:?}: a read must not change what a later read sees, got {again} after {body}"
        );
    }
}

/// The data-point budget the paging test gives each page.
///
/// Smaller than the samples one row carries times the number of rows, so the
/// walk takes several pages and a page boundary lands INSIDE an entry.
const PAGE_POINTS: i64 = 5;

/// How many pages the walk may take before the test gives up, so a handler that
/// never finishes a walk fails instead of hanging.
const PAGE_CEILING: usize = 50;

#[tokio::test]
async fn an_entry_a_previous_page_processed_is_skipped_with_the_status_it_resolved_to() {
    let router = router();
    let (asset_id, rows) = boiler(&router, "BatchHistPaged").await;
    assert_fixture_spans_every_place(&rows);
    for (at, row) in rows.iter().enumerate() {
        seed(&router, &asset_id, at, row).await;
    }

    // The pair that names nothing goes FIRST, so every later page carries a
    // skipped entry of each completion status.
    let mut entries: Vec<Value> = vec![json!({
        "entryId": "absent",
        "assetId": asset_id,
        "propertyId": ABSENT_ID,
    })];
    entries.extend(rows.iter().enumerate().map(|(at, _)| {
        json!({
            "entryId": format!("row{at}"),
            "assetId": asset_id,
            "propertyId": rows[at].property_id,
        })
    }));

    // What each entry resolves to, so the completion status of a skipped entry
    // can be checked against the pair it names rather than against its place.
    let mut expected_status: BTreeMap<String, &str> = BTreeMap::new();
    expected_status.insert("absent".to_string(), "ERROR");
    for at in 0..rows.len() {
        expected_status.insert(format!("row{at}"), "SUCCESS");
    }

    let mut collected: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    // The entries an earlier page COMPLETED: the ones whose every stored sample
    // has been reported, and the one that resolved to an error. Derived from
    // what has been answered rather than from a page index, so the expectation
    // does not restate the handler's own bookkeeping.
    let mut finished: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let mut request = json!({"entries": entries, "maxResults": PAGE_POINTS});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(&router, "POST", BATCH_HISTORY, Some(request)).await;
        let case = format!("page {pages}");
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAssetPropertyValueHistory got {status} {body}"
        );

        // Every entry an earlier page finished is reported as skipped on this
        // one, with the completion status the pair it names resolves to —
        // SUCCESS for a property of the asset, wherever it is declared, and
        // ERROR only for the pair that names none.
        assert_eq!(
            count(&body["skippedEntries"]),
            finished.len(),
            "case {case:?}: expected {} skipped entries, got {body}",
            finished.len()
        );
        for id in &finished {
            let skipped = find_entry(&body["skippedEntries"], id).unwrap_or_else(|| {
                panic!("case {case:?}: {id} was already processed, so it must be skipped: {body}")
            });
            let expected = expected_status[id];
            assert_eq!(
                skipped["completionStatus"].as_str(),
                Some(expected),
                "case {case:?}: {id} resolves to {expected}, got {skipped}"
            );
            if expected == "ERROR" {
                assert_eq!(
                    skipped["errorInfo"]["errorCode"].as_str(),
                    Some(NOT_FOUND),
                    "case {case:?}: expected the error code {NOT_FOUND} for {id}, got {skipped}"
                );
                assert!(
                    skipped["errorInfo"]
                        .get("errorTimestamp")
                        .is_some_and(|stamp| !stamp.is_null()),
                    "case {case:?}: an ERROR skipped entry carries the instant it failed, got {skipped}"
                );
            } else {
                assert!(
                    skipped.get("errorInfo").is_none_or(Value::is_null),
                    "case {case:?}: a SUCCESS skipped entry carries no errorInfo, got {skipped}"
                );
            }
        }

        for entry in body["errorEntries"].as_array().cloned().unwrap_or_default() {
            let id = entry["entryId"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                expected_status.get(id.as_str()).copied(),
                Some("ERROR"),
                "case {case:?}: {id} names a property of the asset, so it must not be an error entry, got {body}"
            );
            assert_eq!(
                entry["errorCode"].as_str(),
                Some(NOT_FOUND),
                "case {case:?}: expected the error code {NOT_FOUND}, got {entry}"
            );
            errors.push(id);
        }

        let mut page_points = 0usize;
        for entry in body["successEntries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let id = entry["entryId"].as_str().unwrap_or_default().to_string();
            let at: usize = id
                .strip_prefix("row")
                .and_then(|at| at.parse().ok())
                .unwrap_or_else(|| panic!("case {case:?}: unexpected success entry {id} in {body}"));
            let seconds = reported_seconds(&case, &entry);
            page_points += seconds.len();
            assert_samples_intact(&case, at, &entry);
            collected.entry(id).or_default().extend(seconds);
        }
        assert!(
            page_points <= PAGE_POINTS as usize,
            "case {case:?}: a page carries at most {PAGE_POINTS} data points, got {page_points}"
        );

        // Whatever is complete by now is what the NEXT page must report as
        // skipped: an entry the page boundary cut is not complete, so it is
        // answered again rather than skipped.
        finished = errors.clone();
        finished.extend(
            collected
                .iter()
                .filter(|(_, seconds)| seconds.len() == SAMPLE_SECONDS.len())
                .map(|(id, _)| id.clone()),
        );

        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        pages += 1;
        assert!(
            pages < PAGE_CEILING,
            "case {case:?}: the walk must end, but it is still paging after {pages} pages"
        );
        if token.is_none() {
            break;
        }
    }

    assert!(
        pages > 1,
        "the budget of {PAGE_POINTS} data points per page must cut this result set into pages, got {pages}"
    );
    assert_eq!(
        errors,
        vec!["absent".to_string()],
        "the pair that names no property must be an error entry exactly once, got {errors:?}"
    );
    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        let id = format!("row{at}");
        assert_eq!(
            collected.get(&id).map(Vec::as_slice),
            Some(SAMPLE_SECONDS.as_slice()),
            "case {case:?}: the walk must report every stored sample of this property exactly once, got {:?}",
            collected.get(&id)
        );
    }
}
