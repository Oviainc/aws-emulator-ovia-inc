// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `BatchGetAssetPropertyValue`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: one batch-get request answers EVERY entry it carries
//! from the one shared engine store — each entry that names a stored property is a
//! success entry carrying that property's latest stored value (and no value at all
//! when its definition declares none and nothing has been written to it), each
//! entry that names no stored property
//! is an error entry carrying the `BatchGetAssetPropertyValueErrorCode` for why,
//! and the operation reports without changing anything a later caller sees.
//!
//! The harness (`router`, `call`, `create_model`, ...) lives in the
//! `asset_lifecycle` suite; this file reuses it rather than keeping a second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router, router_with_engine,
};

/// Where `BatchGetAssetPropertyValue` is bound: `POST /properties/batch/latest`,
/// from `smithy_query(query="operation", operation="BatchGetAssetPropertyValue")`.
const BATCH_LATEST: &str = "/properties/batch/latest";

/// Where `BatchPutAssetPropertyValue` is bound, used here to put the values the
/// batch get must report.
const BATCH_PUT: &str = "/properties";

/// The fixed epoch the timestamps in this suite's tables are written against.
///
/// `BatchPutAssetPropertyValue` ingests only TQVs inside
/// `[now - 7 days, now + 10 minutes]`, so a seed at a fixed calendar second is
/// refused. The tables keep their readable timestamps and the shared
/// `super::support` anchoring maps this epoch onto a point inside the window, with
/// [`relative`] mapping a reported timestamp back for the assertions.
const ROW_EPOCH: i64 = 1_700_000_000;

/// A table second as the absolute `timeInSeconds` the wire carries.
fn anchored(seconds: i64) -> i64 {
    crate::support::anchored(ROW_EPOCH, seconds)
}

/// The inverse of [`anchored`].
fn relative(seconds: i64) -> i64 {
    crate::support::relative(ROW_EPOCH, seconds)
}

/// The entry of `list` whose `entryId` is `entry_id`, if any.
pub(crate) fn find_entry<'a>(list: &'a Value, entry_id: &str) -> Option<&'a Value> {
    list.as_array()?
        .iter()
        .find(|entry| entry["entryId"] == entry_id)
}

/// How many elements a response list carries (0 when it is absent).
pub(crate) fn count(list: &Value) -> usize {
    list.as_array().map_or(0, Vec::len)
}

/// A model whose properties cover one measurement per `PropertyDataType` a
/// `Variant` member exists for, plus an attribute carrying a model default value
/// and one carrying none — the second is the property nothing ever gives a value.
/// Source: `smithy_query(query="shape", shape="PropertyDataType,Variant")`.
fn mixed_type_model() -> Value {
    json!([
        {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}},
        {"name": "Cycles", "dataType": "INTEGER", "unit": "Count", "type": {"measurement": {}}},
        {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
        {"name": "Serial", "dataType": "STRING", "type": {"attribute": {"defaultValue": "unset"}}},
        {"name": "Site", "dataType": "STRING", "type": {"attribute": {}}},
    ])
}

/// Put one value on `(asset_id, property_id)` through `BatchPutAssetPropertyValue`.
async fn put_value(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    value: Value,
    seconds: i64,
    quality: &str,
) {
    let (status, body) = call(
        router,
        "POST",
        BATCH_PUT,
        Some(json!({"entries": [{
            "entryId": "seed",
            "assetId": asset_id,
            "propertyId": property,
            "propertyValues": [{
                "value": value,
                "timestamp": {"timeInSeconds": anchored(seconds), "offsetInNanos": 125},
                "quality": quality,
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "seeding {property}: got {status} {body}");
    assert_eq!(
        count(&body["errorEntries"]),
        0,
        "seeding {property}: the write was refused: {body}"
    );
}

#[tokio::test]
async fn batch_get_reports_the_latest_stored_value_of_every_entry_it_carries() {
    let router = router();
    let model_id = create_model(&router, "Boiler", mixed_type_model()).await;
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");

    // Rows differ along the dimensions the rule generalises over: the property's
    // data type and so its `Variant` member, the value, the `Quality` the write
    // carried (GOOD | BAD | UNCERTAIN, from `smithy_query(query="shape",
    // shape="Quality")`) and the timestamp. Each row is written TWICE, older then
    // newer, so a reported value that is not the newest sample fails the row.
    let rows: Vec<(&str, String, &str, Value, Value, i64, &str)> = vec![
        (
            "DOUBLE measurement",
            property_id(&asset["assetProperties"], "Temperature"),
            "doubleValue",
            json!(11.25),
            json!(42.5),
            1_700_000_100,
            "GOOD",
        ),
        (
            "INTEGER measurement",
            property_id(&asset["assetProperties"], "Cycles"),
            "integerValue",
            json!(1),
            json!(7),
            1_700_000_200,
            "BAD",
        ),
        (
            "STRING measurement",
            property_id(&asset["assetProperties"], "Status"),
            "stringValue",
            json!("STARTING"),
            json!("NOMINAL"),
            1_700_000_300,
            "UNCERTAIN",
        ),
        (
            "BOOLEAN measurement",
            property_id(&asset["assetProperties"], "Running"),
            "booleanValue",
            json!(false),
            json!(true),
            1_700_000_400,
            "GOOD",
        ),
    ];

    for (case, property, member, older, newer, seconds, quality) in &rows {
        put_value(
            &router,
            &asset_id,
            property,
            json!({ *member: older }),
            seconds - 60,
            "GOOD",
        )
        .await;
        put_value(
            &router,
            &asset_id,
            property,
            json!({ *member: newer }),
            *seconds,
            quality,
        )
        .await;
        assert!(
            !property.is_empty(),
            "case {case:?}: the model property has no id"
        );
    }

    // One request carrying every row, plus the attribute whose current value is
    // the default its model declares and the attribute that declares none.
    let defaulted = property_id(&asset["assetProperties"], "Serial");
    let unwritten = property_id(&asset["assetProperties"], "Site");
    let mut entries: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(at, (_, property, ..))| {
            json!({"entryId": format!("row{at}"), "assetId": asset_id, "propertyId": property})
        })
        .collect();
    entries.push(json!({
        "entryId": "defaulted",
        "assetId": asset_id,
        "propertyId": defaulted,
    }));
    entries.push(json!({
        "entryId": "unwritten",
        "assetId": asset_id,
        "propertyId": unwritten,
    }));

    let (status, body) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(status, 200, "BatchGetAssetPropertyValue: got {status} {body}");
    assert_eq!(
        count(&body["errorEntries"]),
        0,
        "every entry names a stored property, so errorEntries must be empty: {body}"
    );
    assert_eq!(
        count(&body["skippedEntries"]),
        0,
        "no previous page processed anything, so skippedEntries must be empty: {body}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "one page answered every entry, so there must be no nextToken: {body}"
    );
    assert_eq!(
        count(&body["successEntries"]),
        rows.len() + 2,
        "expected one success entry per requested entry, got {body}"
    );

    for (at, (case, _, member, _, newer, seconds, quality)) in rows.iter().enumerate() {
        let entry_id = format!("row{at}");
        let entry = find_entry(&body["successEntries"], &entry_id)
            .unwrap_or_else(|| panic!("case {case:?}: no success entry {entry_id} in {body}"));
        let reported = &entry["assetPropertyValue"];
        assert_eq!(
            &reported["value"][member], newer,
            "case {case:?}: expected the newest {member} {newer}, got {entry}"
        );
        assert_eq!(
            reported["timestamp"]["timeInSeconds"].as_i64().map(relative),
            Some(*seconds),
            "case {case:?}: expected the newest sample's timestamp {seconds}, got {entry}"
        );
        assert_eq!(
            reported["timestamp"]["offsetInNanos"].as_i64(),
            Some(125),
            "case {case:?}: the stored nanosecond offset is not reported in {entry}"
        );
        assert_eq!(
            reported["quality"].as_str(),
            Some(*quality),
            "case {case:?}: expected quality {quality}, got {entry}"
        );
        assert_eq!(
            reported["value"].as_object().map(|value| value.len()),
            Some(1),
            "case {case:?}: a Variant carries exactly one member, got {entry}"
        );
    }

    // An attribute's model default value IS its current value, so the entry for a
    // property nothing was written to carries it; a property whose definition
    // declares no value at all carries none. (The rule across data types is pinned
    // by the `attribute_default_values` suite.)
    let default = find_entry(&body["successEntries"], "defaulted")
        .unwrap_or_else(|| panic!("no success entry for the defaulted property in {body}"));
    assert_eq!(
        default["assetPropertyValue"]["value"]["stringValue"].as_str(),
        Some("unset"),
        "an attribute reports the default value its model declares, got {default}"
    );
    assert_eq!(
        default["assetPropertyValue"]["quality"].as_str(),
        Some("GOOD"),
        "the materialised default carries the quality of an ordinary reading, got {default}"
    );
    let empty = find_entry(&body["successEntries"], "unwritten")
        .unwrap_or_else(|| panic!("no success entry for the unwritten property in {body}"));
    assert!(
        empty.get("assetPropertyValue").is_none(),
        "a property whose definition declares no value has no current value, got {empty}"
    );

    // Reporting changes nothing a later caller sees: the same request answers the
    // same way.
    let (again_status, again) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(again_status, 200, "second batch get: got {again_status} {again}");
    assert_eq!(
        again, body,
        "a batch get must not change what the next caller reads: {again}"
    );
}

#[tokio::test]
async fn an_entry_that_names_no_stored_property_is_an_error_entry() {
    let router = router();
    let absent = "11111111-2222-3333-4444-555555555555";
    let model_id = create_model(&router, "Press", mixed_type_model()).await;
    let asset_id = create_asset(&router, "Press1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let cycles = property_id(&asset["assetProperties"], "Cycles");
    put_value(
        &router,
        &asset_id,
        &cycles,
        json!({"integerValue": 3}),
        1_700_000_500,
        "GOOD",
    )
    .await;

    // Rows span the ways an entry can fail to name one stored property. The error
    // codes are the `BatchGetAssetPropertyValueErrorCode` values the model lists:
    // ResourceNotFoundException | InvalidRequestException | AccessDeniedException
    // (the last needs an authorization decision this emulator does not make).
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "resolves",
            json!({"entryId": "ok", "assetId": asset_id, "propertyId": cycles}),
            None,
        ),
        (
            "unknown asset",
            json!({"entryId": "noasset", "assetId": absent, "propertyId": cycles}),
            Some("ResourceNotFoundException"),
        ),
        (
            "unknown property",
            json!({"entryId": "noproperty", "assetId": asset_id, "propertyId": absent}),
            Some("ResourceNotFoundException"),
        ),
        (
            "unknown alias",
            json!({"entryId": "noalias", "propertyAlias": "/no/such/stream"}),
            Some("ResourceNotFoundException"),
        ),
        (
            "no identifier at all",
            json!({"entryId": "noid"}),
            Some("InvalidRequestException"),
        ),
        (
            "both identifying forms at once",
            json!({"entryId": "bothforms", "assetId": asset_id, "propertyId": cycles,
                   "propertyAlias": "/company/press/1/cycles"}),
            Some("InvalidRequestException"),
        ),
        (
            "property id without an asset id",
            json!({"entryId": "lonelyproperty", "propertyId": cycles}),
            Some("InvalidRequestException"),
        ),
    ];

    let entries: Vec<Value> = cases.iter().map(|(_, entry, _)| entry.clone()).collect();
    let (status, body) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(
        status, 200,
        "a per-entry failure is reported in the body, not as a request failure: got {status} {body}"
    );

    for (case, entry, expected) in &cases {
        let entry_id = entry["entryId"].as_str().expect("an entryId");
        match expected {
            None => {
                let success = find_entry(&body["successEntries"], entry_id).unwrap_or_else(|| {
                    panic!("case {case:?}: expected a success entry {entry_id}, got {body}")
                });
                assert_eq!(
                    success["assetPropertyValue"]["value"]["integerValue"].as_i64(),
                    Some(3),
                    "case {case:?}: expected the stored value, got {success}"
                );
                assert!(
                    find_entry(&body["errorEntries"], entry_id).is_none(),
                    "case {case:?}: {entry_id} must not also be an error entry: {body}"
                );
            }
            Some(code) => {
                let failed = find_entry(&body["errorEntries"], entry_id).unwrap_or_else(|| {
                    panic!("case {case:?}: expected an error entry {entry_id}, got {body}")
                });
                assert_eq!(
                    failed["errorCode"].as_str(),
                    Some(*code),
                    "case {case:?}: expected errorCode {code}, got {failed}"
                );
                assert!(
                    failed["errorMessage"]
                        .as_str()
                        .is_some_and(|message| !message.is_empty()),
                    "case {case:?}: an error entry must carry an errorMessage, got {failed}"
                );
                assert!(
                    find_entry(&body["successEntries"], entry_id).is_none(),
                    "case {case:?}: {entry_id} must not also be a success entry: {body}"
                );
            }
        }
    }

    assert_eq!(
        count(&body["successEntries"]) + count(&body["errorEntries"]),
        cases.len(),
        "every entry is answered exactly once: {body}"
    );
}

#[tokio::test]
async fn the_alias_form_names_the_same_property_as_the_id_form() {
    let (router, engine) = router_with_engine();
    let model_id = create_model(&router, "Turbine", mixed_type_model()).await;
    let asset_id = create_asset(&router, "Turbine1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    put_value(
        &router,
        &asset_id,
        &temperature,
        json!({"doubleValue": 63.75}),
        1_700_000_600,
        "GOOD",
    )
    .await;

    // `UpdateAssetProperty` — the operation that gives a property its alias — is
    // still a scaffold stub, so the alias index is seeded through the shared store
    // the handlers read, exactly as that operation will write it.
    let alias = "/company/windfarm/3/turbine/7/temperature";
    {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        state.lock().property_alias_index.insert(
            alias.to_string(),
            (asset_id.clone(), temperature.clone()),
        );
    }

    let (status, body) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({"entries": [
            {"entryId": "byid", "assetId": asset_id, "propertyId": temperature},
            {"entryId": "byalias", "propertyAlias": alias},
        ]})),
    )
    .await;
    assert_eq!(status, 200, "batch get by alias: got {status} {body}");
    let by_id = find_entry(&body["successEntries"], "byid")
        .unwrap_or_else(|| panic!("no success entry byid in {body}"));
    let by_alias = find_entry(&body["successEntries"], "byalias")
        .unwrap_or_else(|| panic!("no success entry byalias in {body}"));
    assert_eq!(
        by_alias["assetPropertyValue"], by_id["assetPropertyValue"],
        "case \"alias\": the alias must report the same value as the id form, got {by_alias}"
    );
    assert_eq!(
        by_id["assetPropertyValue"]["value"]["doubleValue"].as_f64(),
        Some(63.75),
        "case \"id\": expected the stored value 63.75, got {by_id}"
    );
}

#[tokio::test]
async fn a_request_is_bounded_at_the_documented_entry_count() {
    let router = router();
    let model_id = create_model(&router, "Kiln", mixed_type_model()).await;
    let asset_id = create_asset(&router, "Kiln1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let heat = property_id(&asset["assetProperties"], "Temperature");

    // Rows: the published bound and one past it — "You can specify up to 128
    // entries per request", from `smithy_query(query="doc",
    // shape="BatchGetAssetPropertyValueRequest", member="entries")`.
    let counts = [(1_usize, true), (127, true), (128, true), (129, false)];
    for (entry_count, accepted) in counts {
        let entries: Vec<Value> = (0..entry_count)
            .map(|at| {
                json!({"entryId": format!("e{at}"), "assetId": asset_id, "propertyId": heat})
            })
            .collect();
        let (status, body) = call(
            &router,
            "POST",
            BATCH_LATEST,
            Some(json!({ "entries": entries })),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {entry_count} entries: expected 200, got {status} {body}"
            );
            assert_eq!(
                count(&body["successEntries"]),
                entry_count,
                "case {entry_count} entries: every entry must be answered, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {entry_count} entries: expected 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("InvalidRequestException"),
                "case {entry_count} entries: expected InvalidRequestException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_result_set_past_the_page_budget_is_paged_with_a_next_token() {
    let router = router();
    let model_id = create_model(
        &router,
        "Recorder",
        json!([
            {"name": "A", "dataType": "STRING", "type": {"measurement": {}}},
            {"name": "B", "dataType": "STRING", "type": {"measurement": {}}},
            {"name": "C", "dataType": "STRING", "type": {"measurement": {}}},
            {"name": "D", "dataType": "STRING", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(&router, "Recorder1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;

    // Each value is 400 KiB, so the four of them cannot share one page of the
    // handler's byte budget: the walk has to split them and hand back a token.
    let names = ["A", "B", "C", "D"];
    let mut entries = Vec::new();
    for (at, name) in names.iter().enumerate() {
        let property = property_id(&asset["assetProperties"], name);
        put_value(
            &router,
            &asset_id,
            &property,
            json!({"stringValue": name.repeat(400 * 1024)}),
            1_700_000_700 + at as i64,
            "GOOD",
        )
        .await;
        entries.push(
            json!({"entryId": format!("big{at}"), "assetId": asset_id, "propertyId": property}),
        );
    }

    let mut answered: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let request = match &token {
            None => json!({ "entries": entries }),
            Some(next) => json!({ "entries": entries, "nextToken": next }),
        };
        let (status, body) = call(&router, "POST", BATCH_LATEST, Some(request)).await;
        assert_eq!(status, 200, "page {pages}: got {status} {body}");

        // Everything already answered is reported as skipped on this page, in order
        // and with the completion status it was answered with.
        assert_eq!(
            count(&body["skippedEntries"]),
            answered.len(),
            "page {pages}: expected {} skipped entries, got {body}",
            answered.len()
        );
        for entry_id in &answered {
            let skipped = find_entry(&body["skippedEntries"], entry_id).unwrap_or_else(|| {
                panic!("page {pages}: {entry_id} was already answered, so it must be skipped: {body}")
            });
            assert_eq!(
                skipped["completionStatus"].as_str(),
                Some("SUCCESS"),
                "page {pages}: {entry_id} resolved, so its completionStatus is SUCCESS, got {skipped}"
            );
            assert!(
                skipped.get("errorInfo").is_none(),
                "page {pages}: a SUCCESS skipped entry carries no errorInfo, got {skipped}"
            );
        }

        let page: Vec<String> = body["successEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("page {pages}: no successEntries in {body}"))
            .iter()
            .map(|entry| entry["entryId"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(
            !page.is_empty(),
            "page {pages}: a page must answer at least one entry, got {body}"
        );
        for entry in body["successEntries"].as_array().into_iter().flatten() {
            assert_eq!(
                entry["assetPropertyValue"]["value"]["stringValue"]
                    .as_str()
                    .map(str::len),
                Some(400 * 1024),
                "page {pages}: the stored value must be reported whole, got entry {}",
                entry["entryId"]
            );
        }
        answered.extend(page);
        pages += 1;
        assert!(pages <= names.len(), "the walk must terminate: {answered:?}");

        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }

    assert!(
        pages > 1,
        "case \"oversize result set\": expected more than one page, got {pages}"
    );
    let expected: Vec<String> = (0..names.len()).map(|at| format!("big{at}")).collect();
    assert_eq!(
        answered, expected,
        "every entry must be answered exactly once, in order: got {answered:?}"
    );

    // A page token belongs to the request that produced it: replayed against a
    // different entry list it names no page, instead of re-paging that other list.
    let (status, first_page) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(status, 200, "first page again: got {status} {first_page}");
    let token = first_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first page of an oversize result set carries a nextToken"))
        .to_string();
    let mut reordered = entries.clone();
    reordered.reverse();
    let (status, refused) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": reordered, "nextToken": token })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"token replayed against another entry list\": expected 400, got {status} {refused}"
    );
    assert!(
        error_code(&refused).ends_with("InvalidRequestException"),
        "case \"token replayed against another entry list\": expected InvalidRequestException, got {refused}"
    );
}

#[tokio::test]
async fn a_next_token_that_names_no_page_of_this_request_is_refused() {
    let router = router();
    let model_id = create_model(&router, "Pump", mixed_type_model()).await;
    let asset_id = create_asset(&router, "Pump1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let cycles = property_id(&asset["assetProperties"], "Cycles");
    let entries = json!([
        {"entryId": "one", "assetId": asset_id, "propertyId": cycles},
        {"entryId": "two", "assetId": asset_id, "propertyId": cycles},
    ]);

    // Rows: a token that is not one this service issues at all, one that decodes to
    // a position at or past the end of this request's two entries, and one that
    // decodes to no number. Every row is inside `NextToken`'s modelled
    // `^[A-Za-z0-9+/=]+$` pattern, so the refusal comes from the handler. (A
    // well-formed token replayed against a DIFFERENT entry list is covered by the
    // paging suite above.)
    let bad_tokens = ["ff", "3938", "32", "6162", "LTE="];
    for token in bad_tokens {
        let (status, body) = call(
            &router,
            "POST",
            BATCH_LATEST,
            Some(json!({ "entries": entries, "nextToken": token })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidRequestException"),
            "case {token:?}: expected InvalidRequestException, got {body}"
        );
    }
}
