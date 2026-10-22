// SPDX-License-Identifier: CC0-1.0

//! What `BatchPutAssetPropertyValue` refuses and what it silently drops, driven
//! over HTTP through the same `MultiServiceRouter` the emulator boots with.
//!
//! Two rules, both from the operation's own model doc:
//!
//! 1. "IoT SiteWise accepts only TQVs that have a timestamp of no more than 7 days
//!    in the past and no more than 10 minutes in the future. IoT SiteWise rejects
//!    timestamps outside of the inclusive range of [-7 days, +10 minutes] and
//!    returns a TimestampOutOfRangeException error." It is reported PER ENTRY, in
//!    `errorEntries`, and the refused sample is not stored.
//! 2. "For each asset property, IoT SiteWise overwrites TQVs with duplicate
//!    timestamps unless the newer TQV has a different quality." A differing-quality
//!    re-put is accepted (`errorEntries == []`) and has no effect.
//!
//! Every table below spans the class the rule generalises over rather than the one
//! recorded case: the window on each published bound, one past it and well past it;
//! the quality rule over every ordered pair of the modelled `Quality` values plus
//! the omitted form.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, create_model, property_id, router};
use super::support::now_seconds;

/// Seconds in seven days — the published past bound of the ingest window.
const SEVEN_DAYS: i64 = 7 * 24 * 60 * 60;

/// Seconds in ten minutes — the published future bound of the ingest window.
const TEN_MINUTES: i64 = 10 * 60;

/// Create an asset model with `properties`, an asset from it, and return
/// `(assetId, assetProperties)`.
async fn asset_with(router: &MultiServiceRouter, properties: Value) -> (String, Value) {
    let model_id = create_model(router, "Sensor", properties).await;
    let asset_id = create_asset(router, "Sensor1", &model_id).await;
    let (status, described) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    (asset_id, described["assetProperties"].clone())
}

/// One single-value `BatchPutAssetPropertyValue` request.
async fn put(
    router: &MultiServiceRouter,
    entry_id: &str,
    asset_id: &str,
    property: &str,
    value: Value,
    seconds: i64,
    quality: Option<&str>,
) -> (u16, Value) {
    let mut sample = json!({
        "value": value,
        "timestamp": {"timeInSeconds": seconds},
    });
    if let Some(quality) = quality {
        sample["quality"] = json!(quality);
    }
    call(
        router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": entry_id,
            "assetId": asset_id,
            "propertyId": property,
            "propertyValues": [sample],
        }]})),
    )
    .await
}

/// The latest `propertyValue` of a property, or `Value::Null`.
async fn latest(router: &MultiServiceRouter, asset_id: &str, property: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={property}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetAssetPropertyValue: got {status} {body}");
    body.get("propertyValue").cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_timestamp_outside_the_ingest_window_is_refused_per_entry_and_not_stored() {
    let router = router();
    let (asset_id, properties) = asset_with(
        &router,
        json!([
            {"name": "Inside", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Outside", "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let inside = property_id(&properties, "Inside");
    let outside = property_id(&properties, "Outside");

    let now = now_seconds();
    // Rows span the window by OFFSET FROM NOW, in ascending order, and cover each
    // published bound and one past it. The bounds come from the operation's model
    // doc: "the inclusive range of [-7 days, +10 minutes]".
    //
    // Each bound is probed EXACTLY on the accepting side and a margin past it on the
    // refusing side, because the server evaluates the window against ITS clock,
    // which is at or after the instant this test read `now`: a later server clock
    // makes a past timestamp staler (so `-7 days` exactly could fall out) and a
    // future timestamp fresher (so `+10 minutes + 1s` could fall in). Probing the
    // accepted rows on the bound and the refused rows a margin beyond it is
    // therefore the same coverage without the race.
    let rows: [(&str, i64, bool); 8] = [
        ("a year in the past", -365 * 24 * 60 * 60, false),
        ("well past the seven-day bound", -SEVEN_DAYS - 30, false),
        ("just inside the seven-day bound", -SEVEN_DAYS + 30, true),
        ("an hour in the past", -3_600, true),
        ("the current second", 0, true),
        ("nine minutes ahead", 540, true),
        ("exactly the ten-minute bound", TEN_MINUTES, true),
        ("well past the ten-minute bound", TEN_MINUTES + 30, false),
    ];

    let mut newest_accepted: Option<(i64, f64)> = None;
    for (at, (case, offset, accepted)) in rows.into_iter().enumerate() {
        let seconds = now + offset;
        let reading = offset as f64 + 0.5;
        let property = if accepted { &inside } else { &outside };
        // `PutAssetPropertyValueEntry$entryId` is modelled with the pattern
        // `^[a-zA-Z0-9_-]+$`, so the row's prose name cannot be the entryId.
        let entry_id = format!("e{at}");
        let (status, body) = put(
            &router,
            &entry_id,
            &asset_id,
            property,
            json!({"doubleValue": reading}),
            seconds,
            Some("GOOD"),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        let errors = body["errorEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no errorEntries in {body}"));

        if accepted {
            assert!(
                errors.is_empty(),
                "case {case:?}: an in-window timestamp must ingest cleanly, got {body}"
            );
            newest_accepted = Some((seconds, reading));
        } else {
            assert_eq!(
                errors.len(),
                1,
                "case {case:?}: expected exactly one errorEntry, got {body}"
            );
            assert_eq!(
                errors[0]["entryId"].as_str().unwrap_or(""),
                entry_id,
                "case {case:?}: the errorEntry must carry the submitted entryId, got {body}"
            );
            assert_eq!(
                errors[0]["errors"][0]["errorCode"].as_str().unwrap_or(""),
                "TimestampOutOfRangeException",
                "case {case:?}: unexpected errorCode in {body}"
            );
            assert!(
                errors[0]["errors"][0]["errorMessage"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: an errorEntry carries a message, got {body}"
            );
            assert_eq!(
                errors[0]["errors"][0]["timestamps"][0]["timeInSeconds"].as_i64(),
                Some(seconds),
                "case {case:?}: the errorEntry must name the refused timestamp, got {body}"
            );
        }

        // Whatever the row, the accepted series holds exactly what the accepted rows
        // wrote: the newest of them (rows ascend), and nothing from a refused one.
        let stored = latest(&router, &asset_id, &inside).await;
        match newest_accepted {
            None => assert!(
                stored.is_null(),
                "case {case:?}: nothing accepted yet, so there must be no value, got {stored}"
            ),
            Some((expected_seconds, expected_value)) => {
                assert_eq!(
                    stored["timestamp"]["timeInSeconds"].as_i64(),
                    Some(expected_seconds),
                    "case {case:?}: expected the latest value at {expected_seconds}, got {stored}"
                );
                assert_eq!(
                    stored["value"]["doubleValue"].as_f64(),
                    Some(expected_value),
                    "case {case:?}: expected doubleValue {expected_value}, got {stored}"
                );
            }
        }
    }

    // Every refused row targeted this property, and not one of them landed.
    let refused = latest(&router, &asset_id, &outside).await;
    assert!(
        refused.is_null(),
        "an out-of-range sample must never be stored, got {refused}"
    );
}

#[tokio::test]
async fn a_re_put_at_an_occupied_timestamp_overwrites_only_when_the_quality_matches() {
    let router = router();
    let (asset_id, properties) = asset_with(
        &router,
        json!([{"name": "Speed", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let speed = property_id(&properties, "Speed");

    // The `Quality` enum's values are GOOD, BAD and UNCERTAIN
    // (smithy_query(query="shape", shape="com.amazonaws.iotsitewise#Quality")), and
    // `AssetPropertyValue$quality` is optional — so `None` is a fourth submitted
    // form, naming the same quality as an explicit GOOD. Rows are every pair that
    // matters: each value against itself, each ordered mismatch, and the omitted
    // form against GOOD, against itself and against a mismatch.
    let rows: [(&str, Option<&str>, Option<&str>, bool); 12] = [
        ("GOOD over GOOD", Some("GOOD"), Some("GOOD"), true),
        ("BAD over BAD", Some("BAD"), Some("BAD"), true),
        ("UNCERTAIN over UNCERTAIN", Some("UNCERTAIN"), Some("UNCERTAIN"), true),
        ("omitted over omitted", None, None, true),
        ("omitted over GOOD", Some("GOOD"), None, true),
        ("GOOD over omitted", None, Some("GOOD"), true),
        ("BAD over GOOD", Some("GOOD"), Some("BAD"), false),
        ("GOOD over BAD", Some("BAD"), Some("GOOD"), false),
        ("UNCERTAIN over GOOD", Some("GOOD"), Some("UNCERTAIN"), false),
        ("BAD over UNCERTAIN", Some("UNCERTAIN"), Some("BAD"), false),
        ("BAD over omitted", None, Some("BAD"), false),
        ("omitted over UNCERTAIN", Some("UNCERTAIN"), None, false),
    ];

    // Ascending timestamps inside the ingest window, so each row's sample is the
    // newest in the series and `GetAssetPropertyValue` reports exactly it.
    let base = now_seconds() - 600;
    for (at, (case, stored_quality, new_quality, overwrites)) in rows.into_iter().enumerate() {
        let seconds = base + at as i64;

        let (status, first) = put(
            &router,
            &format!("first{at}"),
            &asset_id,
            &speed,
            json!({"doubleValue": 20.0}),
            seconds,
            stored_quality,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: first put got {status} {first}");
        assert_eq!(
            first["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: the first put must ingest cleanly, got {first}"
        );
        let after_first = latest(&router, &asset_id, &speed).await;
        assert_eq!(
            after_first["value"]["doubleValue"].as_f64(),
            Some(20.0),
            "case {case:?}: the first put must be the latest value, got {after_first}"
        );

        let (status, second) = put(
            &router,
            &format!("second{at}"),
            &asset_id,
            &speed,
            json!({"doubleValue": 30.0}),
            seconds,
            new_quality,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: second put got {status} {second}");
        // A dropped put is ACCEPTED: no error entry, no exception.
        assert_eq!(
            second["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a re-put is always accepted, got {second}"
        );

        let (expected_value, expected_quality) = if overwrites {
            (30.0, new_quality)
        } else {
            (20.0, stored_quality)
        };
        // Read twice: a dropped put must not change what a later caller sees, and
        // the answer must be stable across independent reads.
        for read in 0..2 {
            let stored = latest(&router, &asset_id, &speed).await;
            assert_eq!(
                stored["value"]["doubleValue"].as_f64(),
                Some(expected_value),
                "case {case:?}: read {read} expected doubleValue {expected_value}, got {stored}"
            );
            assert_eq!(
                stored["quality"].as_str(),
                expected_quality,
                "case {case:?}: read {read} expected quality {expected_quality:?}, got {stored}"
            );
            assert_eq!(
                stored["timestamp"]["timeInSeconds"].as_i64(),
                Some(seconds),
                "case {case:?}: read {read} expected timeInSeconds {seconds}, got {stored}"
            );
        }
    }
}

#[tokio::test]
async fn partial_entry_processing_decides_whether_one_bad_tqv_sinks_the_whole_entry() {
    // "If set to true, we ingest all TQVs not resulting in an error. If set to
    // false, an invalid TQV fails ingestion of the entire entry that contains it."
    // Rows: the flag absent (the all-or-nothing default), explicitly false, and
    // true.
    let rows: [(&str, Option<bool>, bool); 3] = [
        ("flag absent", None, false),
        ("flag false", Some(false), false),
        ("flag true", Some(true), true),
    ];

    for (case, flag, ingests_the_good_one) in rows {
        let router = router();
        let (asset_id, properties) = asset_with(
            &router,
            json!([{"name": "Level", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
        )
        .await;
        let level = property_id(&properties, "Level");

        let now = now_seconds();
        let good_second = now - 120;
        let bad_second = now + TEN_MINUTES + 1_200;
        let mut body = json!({"entries": [{
            "entryId": "mixed",
            "assetId": asset_id,
            "propertyId": level,
            "propertyValues": [
                {"value": {"doubleValue": 1.5}, "timestamp": {"timeInSeconds": good_second}},
                {"value": {"doubleValue": 2.5}, "timestamp": {"timeInSeconds": bad_second}},
            ],
        }]});
        if let Some(flag) = flag {
            body["enablePartialEntryProcessing"] = json!(flag);
        }

        let (status, response) = call(&router, "POST", "/properties", Some(body)).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {response}");
        let errors = response["errorEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no errorEntries in {response}"));
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: the bad TQV must be reported, got {response}"
        );
        assert_eq!(
            errors[0]["errors"][0]["errorCode"].as_str().unwrap_or(""),
            "TimestampOutOfRangeException",
            "case {case:?}: unexpected errorCode in {response}"
        );
        // Only the offending TQV's timestamp is named, not the entry's good one.
        let timestamps = errors[0]["errors"][0]["timestamps"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no timestamps in {response}"));
        assert_eq!(
            timestamps.len(),
            1,
            "case {case:?}: only the refused TQV is named, got {response}"
        );
        assert_eq!(
            timestamps[0]["timeInSeconds"].as_i64(),
            Some(bad_second),
            "case {case:?}: expected the refused timestamp {bad_second}, got {response}"
        );

        let stored = latest(&router, &asset_id, &level).await;
        if ingests_the_good_one {
            assert_eq!(
                stored["timestamp"]["timeInSeconds"].as_i64(),
                Some(good_second),
                "case {case:?}: the in-window TQV must have landed, got {stored}"
            );
            assert_eq!(
                stored["value"]["doubleValue"].as_f64(),
                Some(1.5),
                "case {case:?}: expected doubleValue 1.5, got {stored}"
            );
        } else {
            assert!(
                stored.is_null(),
                "case {case:?}: one bad TQV fails the whole entry, got {stored}"
            );
        }
    }
}
