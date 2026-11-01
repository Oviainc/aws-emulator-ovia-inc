// SPDX-License-Identifier: CC0-1.0

//! The tracker + device-position loop, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call is
//! a separate HTTP request against one shared engine: a value that survives from one
//! call to the next survived because it was PERSISTED, not because a handler echoed
//! its own input back.
//!
//! The tables span the classes the rules generalise over (the `PositionFiltering`
//! enum, the WGS 84 coordinate domain, `MaxResults` page sizes, sample-time
//! orderings); the request in the issue report is one row of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_location_locationservice::state;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{
    call, call_in_region, emulator, emulator_with_engine, iso_millis, millis_of, wire_time, Answer,
    ACCOUNT, REGION,
};

async fn create_tracker(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/tracking/v0/trackers", Some(body)).await
}

async fn describe_tracker(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/tracking/v0/trackers/{name}"), None).await
}

async fn delete_tracker(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/tracking/v0/trackers/{name}"),
        None,
    )
    .await
}

async fn list_trackers(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/tracking/v0/list-trackers", Some(body)).await
}

async fn batch_update(router: &MultiServiceRouter, name: &str, updates: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tracking/v0/trackers/{name}/positions"),
        Some(json!({ "Updates": updates })),
    )
    .await
}

async fn get_device_position(router: &MultiServiceRouter, name: &str, device: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/tracking/v0/trackers/{name}/devices/{device}/positions/latest"),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// CreateTracker / DescribeTracker
// ---------------------------------------------------------------------------

/// One row of the create-then-describe table.
struct TrackerCase {
    name: &'static str,
    request: fn() -> Value,
    /// What DescribeTracker must report, computed from what was stored rather than
    /// from the request: an absent `Description` reads back as the empty string and
    /// an absent `PositionFiltering` reads back as the documented default.
    description: &'static str,
    position_filtering: &'static str,
    kms_key_id: Option<&'static str>,
    event_bridge_enabled: bool,
    geospatial_queries: bool,
    tags: &'static [(&'static str, &'static str)],
}

/// The create-then-describe table. Rows differ along the members the rule
/// generalises over: presence of each optional member, every value of the
/// `PositionFiltering` enum, tag-map size, and the punctuation a `ResourceName`
/// admits (`^[-._\w]+$`).
///
/// `PositionFiltering` values are the model's enum members
/// (`smithy_query shape=PositionFiltering`): TimeBased, DistanceBased, AccuracyBased.
fn tracker_cases() -> Vec<TrackerCase> {
    vec![
        TrackerCase {
            // Every optional member absent: the defaults must come from the
            // service, not from the request.
            name: "minimal",
            request: || json!({ "TrackerName": "minimal" }),
            description: "",
            position_filtering: "TimeBased",
            kms_key_id: None,
            event_bridge_enabled: false,
            geospatial_queries: false,
            tags: &[],
        },
        TrackerCase {
            name: "demo-tracker",
            request: || {
                json!({
                    "TrackerName": "demo-tracker",
                    "Description": "demo",
                    "PositionFiltering": "TimeBased",
                })
            },
            description: "demo",
            position_filtering: "TimeBased",
            kms_key_id: None,
            event_bridge_enabled: false,
            geospatial_queries: false,
            tags: &[],
        },
        TrackerCase {
            name: "distance.based_1",
            request: || {
                json!({
                    "TrackerName": "distance.based_1",
                    "Description": "fleet of delivery vans",
                    "PositionFiltering": "DistanceBased",
                    "EventBridgeEnabled": true,
                    "Tags": { "team": "logistics", "env": "staging", "cost-center": "42" },
                })
            },
            description: "fleet of delivery vans",
            position_filtering: "DistanceBased",
            kms_key_id: None,
            event_bridge_enabled: true,
            geospatial_queries: false,
            tags: &[("cost-center", "42"), ("env", "staging"), ("team", "logistics")],
        },
        TrackerCase {
            // The row the model's own enum coverage needs, plus a customer managed
            // key and the geospatial-queries opt-in.
            name: "AccuracyBased-tracker",
            request: || {
                json!({
                    "TrackerName": "AccuracyBased-tracker",
                    "PositionFiltering": "AccuracyBased",
                    "KmsKeyId": "alias/aws/geo",
                    "KmsKeyEnableGeospatialQueries": true,
                    "EventBridgeEnabled": false,
                    "Tags": { "owner": "maps" },
                })
            },
            description: "",
            position_filtering: "AccuracyBased",
            kms_key_id: Some("alias/aws/geo"),
            event_bridge_enabled: false,
            geospatial_queries: true,
            tags: &[("owner", "maps")],
        },
    ]
}

#[tokio::test]
async fn create_then_describe_reports_what_was_stored() {
    let router = emulator();

    for case in tracker_cases() {
        let created = create_tracker(&router, (case.request)()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateTracker must succeed, got {} {:?}",
            case.name, created.status, created.body
        );
        let created = created.json();
        let expected_arn = format!("arn:aws:geo:{REGION}:{ACCOUNT}:tracker/{}", case.name);
        assert_eq!(
            created.get("TrackerName").and_then(Value::as_str),
            Some(case.name),
            "case {:?}: CreateTracker must answer the created name, got {created}",
            case.name
        );
        assert_eq!(
            created.get("TrackerArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: expected TrackerArn {expected_arn}, got {created}",
            case.name
        );
        let create_time = millis_of(&created, "CreateTime");

        let described = describe_tracker(&router, case.name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeTracker must succeed, got {} {:?}",
            case.name, described.status, described.body
        );
        let described = described.json();

        assert_eq!(
            described.get("TrackerArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: DescribeTracker must report the stored ARN, got {described}",
            case.name
        );
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            Some(case.description),
            "case {:?}: expected Description {:?}, got {described}",
            case.name,
            case.description
        );
        assert_eq!(
            described.get("PositionFiltering").and_then(Value::as_str),
            Some(case.position_filtering),
            "case {:?}: expected PositionFiltering {:?}, got {described}",
            case.name,
            case.position_filtering
        );
        assert_eq!(
            described.get("KmsKeyId").and_then(Value::as_str),
            case.kms_key_id,
            "case {:?}: expected KmsKeyId {:?}, got {described}",
            case.name,
            case.kms_key_id
        );
        assert_eq!(
            described.get("EventBridgeEnabled").and_then(Value::as_bool),
            Some(case.event_bridge_enabled),
            "case {:?}: expected EventBridgeEnabled {}, got {described}",
            case.name,
            case.event_bridge_enabled
        );
        assert_eq!(
            described
                .get("KmsKeyEnableGeospatialQueries")
                .and_then(Value::as_bool),
            Some(case.geospatial_queries),
            "case {:?}: expected KmsKeyEnableGeospatialQueries {}, got {described}",
            case.name,
            case.geospatial_queries
        );
        // `DescribeTrackerResponse$PricingPlan` doc: "Always returns
        // RequestBasedUsage"; `$PricingPlanDataSource`: "Always returns an empty
        // string".
        assert_eq!(
            described.get("PricingPlan").and_then(Value::as_str),
            Some("RequestBasedUsage"),
            "case {:?}: PricingPlan must always be RequestBasedUsage, got {described}",
            case.name
        );
        assert_eq!(
            described.get("PricingPlanDataSource").and_then(Value::as_str),
            Some(""),
            "case {:?}: PricingPlanDataSource must always be empty, got {described}",
            case.name
        );

        let mut reported_tags: Vec<(String, String)> = described
            .get("Tags")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        reported_tags.sort();
        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {:?}: expected Tags {expected_tags:?}, got {reported_tags:?}",
            case.name
        );

        assert_eq!(
            millis_of(&described, "CreateTime"),
            create_time,
            "case {:?}: DescribeTracker must report the CreateTime CreateTracker stored, got {described}",
            case.name
        );
        assert_eq!(
            millis_of(&described, "UpdateTime"),
            create_time,
            "case {:?}: a never-updated tracker's UpdateTime must equal its CreateTime, got {described}",
            case.name
        );
    }
}

#[tokio::test]
async fn tracker_arn_carries_the_regions_the_requests_are_addressed_to() {
    // One ARN per request region, so the region segment is read from the request
    // rather than baked in.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = emulator();
        let body = bytes::Bytes::from(
            serde_json::to_vec(&json!({ "TrackerName": "regional" })).expect("body serializes"),
        );
        let created = call_in_region(&router, "POST", "/tracking/v0/trackers", body, region).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateTracker must succeed, got {} {:?}",
            created.status, created.body
        );
        let expected = format!("arn:aws:geo:{region}:{ACCOUNT}:tracker/regional");
        assert_eq!(
            created.json().get("TrackerArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region:?}: expected TrackerArn {expected}, got {:?}",
            created.body
        );
    }
}

#[tokio::test]
async fn creating_a_name_that_already_exists_conflicts() {
    let router = emulator();
    for case in tracker_cases() {
        assert_eq!(
            create_tracker(&router, (case.request)()).await.status,
            200,
            "case {:?}: the first CreateTracker must succeed",
            case.name
        );
        let again = create_tracker(&router, (case.request)()).await;
        assert_eq!(
            again.status, 409,
            "case {:?}: re-creating an existing tracker must be 409, got {} {:?}",
            case.name, again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {:?}: expected ConflictException, got {:?}",
            case.name,
            again.body
        );
    }
}

#[tokio::test]
async fn reading_an_unknown_tracker_is_not_found() {
    let router = emulator();
    // Nothing stored at all, and a stored tracker whose name differs: neither may
    // answer for "absent".
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "present" }))
            .await
            .status,
        200
    );

    for name in ["absent", "Present", "present2"] {
        let described = describe_tracker(&router, name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: DescribeTracker on an unknown tracker must be 404, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?}",
            described.body
        );

        let deleted = delete_tracker(&router, name).await;
        assert_eq!(
            deleted.status, 404,
            "case {name:?}: DeleteTracker on an unknown tracker must be 404, got {} {:?}",
            deleted.status, deleted.body
        );

        let position = get_device_position(&router, name, "device-1").await;
        assert_eq!(
            position.status, 404,
            "case {name:?}: GetDevicePosition under an unknown tracker must be 404, got {} {:?}",
            position.status, position.body
        );

        let updated = batch_update(
            &router,
            name,
            json!([update("device-1", "2024-01-01T00:00:00Z", [-122.4, 37.8])]),
        )
        .await;
        assert_eq!(
            updated.status, 404,
            "case {name:?}: BatchUpdateDevicePosition under an unknown tracker must be 404, got {} {:?}",
            updated.status, updated.body
        );
    }
}

// ---------------------------------------------------------------------------
// ListTrackers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_trackers_pages_over_every_stored_tracker() {
    let router = emulator();
    let names: Vec<String> = (1..=7).map(|i| format!("tracker-{i:02}")).collect();
    for name in &names {
        assert_eq!(
            create_tracker(&router, json!({ "TrackerName": name }))
                .await
                .status,
            200,
            "creating {name} must succeed"
        );
    }

    // Page sizes spanning 1, several divisors and non-divisors of the collection
    // size, the exact size, the modelled maximum, and no MaxResults at all.
    for max_results in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = serde_json::Map::new();
            if let Some(limit) = max_results {
                body.insert("MaxResults".to_string(), json!(limit));
            }
            if let Some(cursor) = &token {
                body.insert("NextToken".to_string(), json!(cursor));
            }
            let answer = list_trackers(&router, Value::Object(body)).await;
            assert_eq!(
                answer.status, 200,
                "case {max_results:?}: ListTrackers must succeed, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer.json();
            let entries = page
                .get("Entries")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {max_results:?}: Entries must be a list in {page}"))
                .clone();
            if let Some(limit) = max_results {
                assert!(
                    entries.len() as i32 <= limit,
                    "case {max_results:?}: a page must not exceed MaxResults, got {} entries",
                    entries.len()
                );
            }
            for entry in &entries {
                let name = entry
                    .get("TrackerName")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {max_results:?}: entry has no TrackerName: {entry}"))
                    .to_string();
                // Every entry is computed from the stored tracker.
                assert_eq!(
                    entry.get("Description").and_then(Value::as_str),
                    Some(""),
                    "case {max_results:?}: entry {name} must carry the stored Description, got {entry}"
                );
                assert_eq!(
                    entry.get("PricingPlan").and_then(Value::as_str),
                    Some("RequestBasedUsage"),
                    "case {max_results:?}: entry {name} must always report RequestBasedUsage, got {entry}"
                );
                assert_eq!(
                    millis_of(entry, "CreateTime"),
                    millis_of(entry, "UpdateTime"),
                    "case {max_results:?}: entry {name} was never updated, so its timestamps must match"
                );
                seen.push(name);
            }
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {max_results:?}: pagination did not terminate, saw {seen:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, names,
            "case {max_results:?}: paging must yield every stored tracker exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_cursor_this_service_did_not_mint_is_refused() {
    let router = emulator();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "tracker-01" }))
            .await
            .status,
        200
    );

    // Each row is a token shape a client could send that this service never mints:
    // no tag, the wrong collection's tag, a tag with no page key, and a bare
    // resource name.
    for token in [
        "not-a-token",
        "trackers",
        "trackers:",
        "geofence-collections:tracker-01",
        "tracker-01",
    ] {
        let answer = list_trackers(&router, json!({ "NextToken": token })).await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: a malformed NextToken must be refused with 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// BatchUpdateDevicePosition / GetDevicePosition
// ---------------------------------------------------------------------------

/// Build one `DevicePositionUpdate`.
fn update(device: &str, sample_time: &str, position: [f64; 2]) -> Value {
    json!({
        "DeviceId": device,
        "SampleTime": wire_time(sample_time),
        "Position": [position[0], position[1]],
    })
}

#[tokio::test]
async fn the_latest_position_is_the_highest_sample_time_whatever_the_arrival_order() {
    // Rows differ along the ORDER samples arrive in and how they are batched: the
    // rule is "highest SampleTime wins", not "last one written wins".
    let samples: [(&str, [f64; 2]); 4] = [
        ("2024-01-01T00:00:00Z", [-122.4, 37.8]),
        ("2024-01-02T06:30:00Z", [-122.5, 37.9]),
        ("2024-01-03T12:00:00.250Z", [0.0, 0.0]),
        ("2024-06-01T23:59:59.999Z", [151.2093, -33.8688]),
    ];
    let latest = samples[3];

    let orders: [(&str, Vec<usize>); 4] = [
        ("ascending", vec![0, 1, 2, 3]),
        ("descending", vec![3, 2, 1, 0]),
        ("latest first", vec![3, 0, 2, 1]),
        ("latest in the middle", vec![1, 3, 0, 2]),
    ];

    for (case, order) in orders {
        for batched in [false, true] {
            let router = emulator();
            assert_eq!(
                create_tracker(&router, json!({ "TrackerName": "fleet" }))
                    .await
                    .status,
                200
            );

            if batched {
                // One request carrying the whole order: `Updates` is len 1..10.
                let updates: Vec<Value> = order
                    .iter()
                    .map(|i| update("device-1", samples[*i].0, samples[*i].1))
                    .collect();
                let answer = batch_update(&router, "fleet", json!(updates)).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} batched: BatchUpdateDevicePosition must succeed, got {} {:?}",
                    answer.status, answer.body
                );
            } else {
                for i in &order {
                    let answer = batch_update(
                        &router,
                        "fleet",
                        json!([update("device-1", samples[*i].0, samples[*i].1)]),
                    )
                    .await;
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: BatchUpdateDevicePosition must succeed, got {} {:?}",
                        answer.status, answer.body
                    );
                }
            }

            let got = get_device_position(&router, "fleet", "device-1").await;
            assert_eq!(
                got.status, 200,
                "case {case:?} batched={batched}: GetDevicePosition must succeed, got {} {:?}",
                got.status, got.body
            );
            let got = got.json();
            assert_eq!(
                got.get("DeviceId").and_then(Value::as_str),
                Some("device-1"),
                "case {case:?} batched={batched}: expected DeviceId device-1, got {got}"
            );
            assert_eq!(
                millis_of(&got, "SampleTime"),
                iso_millis(latest.0),
                "case {case:?} batched={batched}: expected the highest SampleTime {}, got {got}",
                latest.0
            );
            assert_eq!(
                got.get("Position").and_then(Value::as_array),
                Some(&vec![json!(latest.1[0]), json!(latest.1[1])]),
                "case {case:?} batched={batched}: expected Position {:?}, got {got}",
                latest.1
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_update_is_stored_with_a_server_assigned_received_time() {
    let router = emulator();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "fleet" }))
            .await
            .status,
        200
    );

    // Rows differ along the optional members of a `DevicePositionUpdate`:
    // `Accuracy` present/absent and `PositionProperties` of size 0, 1 and 4 (the
    // map's modelled maximum).
    struct UpdateCase {
        case: &'static str,
        device: &'static str,
        sample_time: &'static str,
        position: [f64; 2],
        accuracy: Option<f64>,
        properties: &'static [(&'static str, &'static str)],
    }

    let cases = [
        UpdateCase {
            case: "bare",
            device: "bare",
            sample_time: "2024-01-01T00:00:00Z",
            position: [-122.4, 37.8],
            accuracy: None,
            properties: &[],
        },
        UpdateCase {
            case: "accuracy only",
            device: "accuracy-only",
            sample_time: "2024-01-01T00:00:01.500Z",
            position: [8.5417, 47.3769],
            accuracy: Some(12.5),
            properties: &[],
        },
        UpdateCase {
            case: "one property",
            device: "one-property",
            sample_time: "2024-01-01T00:00:02Z",
            position: [13.405, 52.52],
            accuracy: None,
            properties: &[("field-worker", "ana")],
        },
        UpdateCase {
            case: "four properties",
            device: "four-properties",
            sample_time: "2024-01-01T00:00:03Z",
            position: [139.6917, 35.6895],
            accuracy: Some(0.0),
            properties: &[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")],
        },
    ];

    for row in cases {
        let UpdateCase {
            case,
            device,
            sample_time,
            position,
            accuracy,
            properties,
        } = row;

        let mut payload = update(device, sample_time, position);
        let object = payload
            .as_object_mut()
            .expect("an update payload is a JSON object");
        if let Some(horizontal) = accuracy {
            object.insert("Accuracy".to_string(), json!({ "Horizontal": horizontal }));
        }
        if !properties.is_empty() {
            let map: serde_json::Map<String, Value> = properties
                .iter()
                .map(|(k, v)| ((*k).to_string(), json!(v)))
                .collect();
            object.insert("PositionProperties".to_string(), Value::Object(map));
        }

        let before = now_millis();
        let answer = batch_update(&router, "fleet", json!([payload])).await;
        let after = now_millis();
        assert_eq!(
            answer.status, 200,
            "case {case:?}: BatchUpdateDevicePosition must succeed, got {} {:?}",
            answer.status, answer.body
        );
        let errors = answer.json();
        assert_eq!(
            errors.get("Errors").and_then(Value::as_array),
            Some(&vec![]),
            "case {case:?}: an all-valid batch must report no Errors, got {errors}"
        );

        let got = get_device_position(&router, "fleet", device).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: GetDevicePosition must succeed, got {} {:?}",
            got.status, got.body
        );
        let got = got.json();

        assert_eq!(
            millis_of(&got, "SampleTime"),
            iso_millis(sample_time),
            "case {case:?}: SampleTime must be the one sent, got {got}"
        );

        // ReceivedTime is the server's own clock reading, so it falls inside the
        // window the call was made in and is never the SampleTime.
        let received = millis_of(&got, "ReceivedTime");
        assert!(
            received >= before && received <= after,
            "case {case:?}: ReceivedTime {received} must lie in [{before}, {after}], got {got}"
        );

        match accuracy {
            Some(horizontal) => assert_eq!(
                got.get("Accuracy")
                    .and_then(|a| a.get("Horizontal"))
                    .and_then(Value::as_f64),
                Some(horizontal),
                "case {case:?}: expected Accuracy.Horizontal {horizontal}, got {got}"
            ),
            None => assert!(
                got.get("Accuracy").is_none_or(Value::is_null),
                "case {case:?}: an update with no Accuracy must read back without one, got {got}"
            ),
        }

        let mut reported: Vec<(String, String)> = got
            .get("PositionProperties")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        let expected: Vec<(String, String)> = properties
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected PositionProperties {expected:?}, got {reported:?}"
        );
    }
}

/// The host clock in epoch milliseconds, for bracketing a server-assigned timestamp.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the host clock is after the epoch")
        .as_millis() as i64
}

#[tokio::test]
async fn positions_outside_the_wgs84_domain_are_reported_per_entry() {
    // WGS 84 bounds: longitude in [-180, 180], latitude in [-90, 90]. Each
    // published bound is tested, and one step past it.
    let cases: [(&str, [f64; 2], bool); 11] = [
        ("origin", [0.0, 0.0], true),
        ("max longitude", [180.0, 0.0], true),
        ("min longitude", [-180.0, 0.0], true),
        ("max latitude", [0.0, 90.0], true),
        ("min latitude", [0.0, -90.0], true),
        ("both maxima", [180.0, 90.0], true),
        ("both minima", [-180.0, -90.0], true),
        ("past max longitude", [180.000001, 0.0], false),
        ("past min longitude", [-180.000001, 0.0], false),
        ("past max latitude", [0.0, 90.000001], false),
        ("past min latitude", [0.0, -90.000001], false),
    ];

    for (case, position, accepted) in cases {
        let router = emulator();
        assert_eq!(
            create_tracker(&router, json!({ "TrackerName": "fleet" }))
                .await
                .status,
            200
        );

        let sample_time = "2024-01-01T00:00:00Z";
        let answer = batch_update(
            &router,
            "fleet",
            json!([update("device-1", sample_time, position)]),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a per-entry failure is reported in the body, not as a status, got {} {:?}",
            answer.status, answer.body
        );
        let body = answer.json();
        let errors = body
            .get("Errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: Errors must be a list in {body}"));

        let stored = get_device_position(&router, "fleet", "device-1").await;
        if accepted {
            assert!(
                errors.is_empty(),
                "case {case:?}: {position:?} is inside the WGS 84 domain, so Errors must be empty, got {errors:?}"
            );
            assert_eq!(
                stored.status, 200,
                "case {case:?}: an accepted position must be readable, got {} {:?}",
                stored.status, stored.body
            );
            assert_eq!(
                stored.json().get("Position").and_then(Value::as_array),
                Some(&vec![json!(position[0]), json!(position[1])]),
                "case {case:?}: expected the stored Position {position:?}, got {:?}",
                stored.body
            );
        } else {
            assert_eq!(
                errors.len(),
                1,
                "case {case:?}: {position:?} is outside the WGS 84 domain, so exactly one error is expected, got {errors:?}"
            );
            let error = &errors[0];
            assert_eq!(
                error.get("DeviceId").and_then(Value::as_str),
                Some("device-1"),
                "case {case:?}: the error must name the device, got {error}"
            );
            assert_eq!(
                millis_of(error, "SampleTime"),
                iso_millis(sample_time),
                "case {case:?}: the error must carry the entry's SampleTime, got {error}"
            );
            // `BatchItemErrorCode` member for "Input fails to satisfy the
            // constraints specified by the service".
            assert_eq!(
                error
                    .get("Error")
                    .and_then(|e| e.get("Code"))
                    .and_then(Value::as_str),
                Some("ValidationError"),
                "case {case:?}: expected Error.Code ValidationError, got {error}"
            );
            assert_eq!(
                stored.status, 404,
                "case {case:?}: a refused position must not be stored, got {} {:?}",
                stored.status, stored.body
            );
        }
    }
}

#[tokio::test]
async fn a_mixed_batch_stores_the_valid_entries_and_reports_only_the_failures() {
    let router = emulator();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "fleet" }))
            .await
            .status,
        200
    );

    let answer = batch_update(
        &router,
        "fleet",
        json!([
            update("good-1", "2024-01-01T00:00:00Z", [-122.4, 37.8]),
            update("bad-1", "2024-01-01T00:00:01Z", [-200.0, 37.8]),
            update("good-2", "2024-01-01T00:00:02Z", [151.2093, -33.8688]),
            update("bad-2", "2024-01-01T00:00:03Z", [0.0, 91.0]),
        ]),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a mixed batch must still answer 200, got {} {:?}",
        answer.status, answer.body
    );
    let body = answer.json();
    let mut failed: Vec<String> = body
        .get("Errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("Errors must be a list in {body}"))
        .iter()
        .map(|e| {
            e.get("DeviceId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    failed.sort();
    assert_eq!(
        failed,
        vec!["bad-1".to_string(), "bad-2".to_string()],
        "only the out-of-domain entries may be reported, got {failed:?}"
    );

    for device in ["good-1", "good-2"] {
        let got = get_device_position(&router, "fleet", device).await;
        assert_eq!(
            got.status, 200,
            "case {device:?}: a valid entry of a mixed batch must be stored, got {} {:?}",
            got.status, got.body
        );
    }
    for device in ["bad-1", "bad-2"] {
        let got = get_device_position(&router, "fleet", device).await;
        assert_eq!(
            got.status, 404,
            "case {device:?}: a refused entry must store nothing, got {} {:?}",
            got.status, got.body
        );
    }
}

#[tokio::test]
async fn a_device_with_no_stored_position_is_not_found() {
    let router = emulator();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "fleet" }))
            .await
            .status,
        200
    );
    assert_eq!(
        batch_update(
            &router,
            "fleet",
            json!([update("device-1", "2024-01-01T00:00:00Z", [-122.4, 37.8])]),
        )
        .await
        .status,
        200
    );

    // A tracker that holds one device answers 404 for every other device id,
    // including one that differs only in case.
    for device in ["unknown-device", "Device-1", "device-10"] {
        let got = get_device_position(&router, "fleet", device).await;
        assert_eq!(
            got.status, 404,
            "case {device:?}: a device with no stored position must be 404, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.error_code(),
            "ResourceNotFoundException",
            "case {device:?}: expected ResourceNotFoundException, got {:?}",
            got.body
        );
    }
}

#[tokio::test]
async fn a_batch_larger_than_the_modelled_maximum_is_refused() {
    let router = emulator();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "fleet" }))
            .await
            .status,
        200
    );

    // `DevicePositionUpdateList` is `len 1..10`, so a batch is accepted at 10 and
    // refused at one past it; the cap is what keeps the per-request work from
    // being caller-chosen.
    let batch = |count: usize| -> Value {
        json!((0..count)
            .map(|i| update(
                &format!("device-{i}"),
                "2024-01-01T00:00:00Z",
                [-122.4, 37.8]
            ))
            .collect::<Vec<_>>())
    };

    for count in [1_usize, 10] {
        let answer = batch_update(&router, "fleet", batch(count)).await;
        assert_eq!(
            answer.status, 200,
            "case {count}: a batch within the modelled maximum must be accepted, got {} {:?}",
            answer.status, answer.body
        );
    }
    for count in [11_usize, 25] {
        let answer = batch_update(&router, "fleet", batch(count)).await;
        assert_eq!(
            answer.status, 400,
            "case {count}: a batch past the modelled maximum must be refused, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {count}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_devices_stored_history_is_bounded_and_keeps_the_newest_samples() {
    let (router, engine) = emulator_with_engine();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "fleet" }))
            .await
            .status,
        200
    );

    // Write more samples than the store keeps, in batches of the modelled
    // maximum, with strictly increasing sample times one minute apart (inside the
    // documented 30-day retention window, so retention is not what trims them).
    let total = state::MAX_POSITIONS_PER_DEVICE + 3 * state::MAX_BATCH_UPDATES;
    let base = iso_millis("2024-01-01T00:00:00Z");
    for chunk in 0..total.div_ceil(state::MAX_BATCH_UPDATES) {
        let updates: Vec<Value> = (0..state::MAX_BATCH_UPDATES)
            .map(|i| {
                let index = chunk * state::MAX_BATCH_UPDATES + i;
                json!({
                    "DeviceId": "device-1",
                    "SampleTime": (base + index as i64 * 60_000) as f64 / 1000.0,
                    "Position": [-122.4 + index as f64 * 0.0001, 37.8],
                })
            })
            .collect();
        let answer = batch_update(&router, "fleet", json!(updates)).await;
        assert_eq!(
            answer.status, 200,
            "case chunk {chunk}: BatchUpdateDevicePosition must succeed, got {} {:?}",
            answer.status, answer.body
        );
    }

    let stored = state::service_state(&*engine).expect("the service state is reachable");
    let guard = stored.lock();
    let tracker = guard
        .trackers
        .get("fleet")
        .expect("the tracker was created through the API");
    let history = tracker
        .devices
        .get("device-1")
        .expect("the device has a stored history");
    assert_eq!(
        history.len(),
        state::MAX_POSITIONS_PER_DEVICE,
        "the stored history must be capped at {}, got {}",
        state::MAX_POSITIONS_PER_DEVICE,
        history.len()
    );
    let written = total.div_ceil(state::MAX_BATCH_UPDATES) * state::MAX_BATCH_UPDATES;
    let newest = base + (written as i64 - 1) * 60_000;
    assert_eq!(
        history.last().map(|p| p.sample_time),
        Some(newest),
        "the cap must evict the OLDEST samples, so the newest one is still held"
    );
    assert!(
        history.windows(2).all(|w| w[0].sample_time <= w[1].sample_time),
        "the history must stay sorted ascending by sample time"
    );
}

#[tokio::test]
async fn a_second_sample_for_the_same_instant_replaces_the_first() {
    let (router, engine) = emulator_with_engine();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "fleet" }))
            .await
            .status,
        200
    );

    // A sample is keyed by (DeviceId, SampleTime): re-reporting one instant
    // corrects it rather than adding a second sample for the same instant. Rows
    // differ in how the pair arrives: two requests, and both in one batch.
    for (case, batched) in [("two requests", false), ("one batch", true)] {
        let first = update("device-1", "2024-03-01T00:00:00Z", [-122.4, 37.8]);
        let corrected = update("device-1", "2024-03-01T00:00:00Z", [8.5417, 47.3769]);
        let other = update("device-1", "2024-03-01T00:00:01Z", [13.405, 52.52]);

        if batched {
            assert_eq!(
                batch_update(&router, "fleet", json!([first, other, corrected]))
                    .await
                    .status,
                200,
                "case {case:?}: BatchUpdateDevicePosition must succeed"
            );
        } else {
            for payload in [first, other, corrected] {
                assert_eq!(
                    batch_update(&router, "fleet", json!([payload])).await.status,
                    200,
                    "case {case:?}: BatchUpdateDevicePosition must succeed"
                );
            }
        }

        let history = {
            let stored = state::service_state(&*engine).expect("the service state is reachable");
            let guard = stored.lock();
            guard
                .trackers
                .get("fleet")
                .expect("the tracker was created through the API")
                .devices
                .get("device-1")
                .expect("the device has a stored history")
                .clone()
        };
        assert_eq!(
            history.len(),
            2,
            "case {case:?}: the two distinct instants must be the only samples held, got {:?}",
            history.iter().map(|p| p.sample_time).collect::<Vec<_>>()
        );
        assert_eq!(
            history[0].position,
            [8.5417, 47.3769],
            "case {case:?}: the later report for that instant must have replaced the first, got {:?}",
            history[0].position
        );

        assert_eq!(delete_tracker(&router, "fleet").await.status, 200);
        assert_eq!(
            create_tracker(&router, json!({ "TrackerName": "fleet" }))
                .await
                .status,
            200
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteTracker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_tracker_removes_it_and_its_device_histories() {
    let router = emulator();
    for name in ["keep", "drop"] {
        assert_eq!(
            create_tracker(&router, json!({ "TrackerName": name }))
                .await
                .status,
            200
        );
        assert_eq!(
            batch_update(
                &router,
                name,
                json!([update("device-1", "2024-01-01T00:00:00Z", [-122.4, 37.8])]),
            )
            .await
            .status,
            200
        );
    }

    let deleted = delete_tracker(&router, "drop").await;
    assert_eq!(
        deleted.status, 200,
        "DeleteTracker must succeed, got {} {:?}",
        deleted.status, deleted.body
    );

    assert_eq!(
        describe_tracker(&router, "drop").await.status,
        404,
        "a deleted tracker must not describe"
    );
    assert_eq!(
        get_device_position(&router, "drop", "device-1").await.status,
        404,
        "a deleted tracker's device history must be gone"
    );
    assert_eq!(
        delete_tracker(&router, "drop").await.status,
        404,
        "deleting a tracker twice must be 404 the second time"
    );

    let listed = list_trackers(&router, json!({})).await;
    assert_eq!(listed.status, 200, "ListTrackers must succeed");
    let listed = listed.json();
    let names: Vec<&str> = listed
        .get("Entries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("Entries must be a list in {listed}"))
        .iter()
        .filter_map(|e| e.get("TrackerName").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec!["keep"],
        "a deleted tracker must disappear from ListTrackers, got {names:?}"
    );

    // The surviving tracker kept its own history: delete removed only what it owned.
    assert_eq!(
        get_device_position(&router, "keep", "device-1").await.status,
        200,
        "deleting one tracker must not touch another's device history"
    );
}

// ---------------------------------------------------------------------------
// The whole loop, as a client walks it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_tracking_loop_works_end_to_end_across_requests() {
    let router = emulator();

    let created = create_tracker(
        &router,
        json!({
            "TrackerName": "demo-tracker",
            "PositionFiltering": "TimeBased",
            "Description": "demo",
        }),
    )
    .await;
    assert_eq!(created.status, 200, "CreateTracker: {:?}", created.body);

    let described = describe_tracker(&router, "demo-tracker").await;
    assert_eq!(described.status, 200, "DescribeTracker: {:?}", described.body);
    assert_eq!(
        described.json().get("Description").and_then(Value::as_str),
        Some("demo"),
        "DescribeTracker must report the stored description, got {:?}",
        described.body
    );

    let listed = list_trackers(&router, json!({})).await;
    assert_eq!(listed.status, 200, "ListTrackers: {:?}", listed.body);
    assert_eq!(
        listed
            .json()
            .get("Entries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "ListTrackers must list the one created tracker, got {:?}",
        listed.body
    );

    let updated = batch_update(
        &router,
        "demo-tracker",
        json!([update("device-1", "2024-01-01T00:00:00Z", [-122.4, 37.8])]),
    )
    .await;
    assert_eq!(updated.status, 200, "BatchUpdate: {:?}", updated.body);
    assert_eq!(
        updated.json().get("Errors").and_then(Value::as_array),
        Some(&vec![]),
        "an all-valid batch must answer an empty Errors list, got {:?}",
        updated.body
    );

    let got = get_device_position(&router, "demo-tracker", "device-1").await;
    assert_eq!(got.status, 200, "GetDevicePosition: {:?}", got.body);
    let got = got.json();
    assert_eq!(
        got.get("Position").and_then(Value::as_array),
        Some(&vec![json!(-122.4), json!(37.8)]),
        "GetDevicePosition must report the position that was written, got {got}"
    );

    assert_eq!(
        get_device_position(&router, "demo-tracker", "unknown-device")
            .await
            .status,
        404,
        "an unknown device must be 404"
    );

    assert_eq!(
        delete_tracker(&router, "demo-tracker").await.status,
        200,
        "DeleteTracker must succeed"
    );
    assert_eq!(
        describe_tracker(&router, "demo-tracker").await.status,
        404,
        "a deleted tracker must not describe"
    );
    let listed = list_trackers(&router, json!({})).await;
    assert_eq!(
        listed
            .json()
            .get("Entries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a deleted tracker must leave ListTrackers empty, got {:?}",
        listed.body
    );
}

/// RULE: a latest-position read that RESOLVED its tracker but holds no sample for the
/// device states that there are no records for the device id it was given; the
/// missing-named-resource sentence is reserved for a tracker the service cannot resolve
/// by name. Both are `ResourceNotFoundException`, HTTP 404.
///
/// Rows span what the rule generalises over: which of the two lookups failed, and the
/// shapes a device id with no samples can take next to one that has them (a never
/// reported id, one differing only in case, one that is a prefix extension, one carrying
/// the punctuation an `Id` admits).
#[tokio::test]
async fn a_position_read_states_which_lookup_failed() {
    let router = emulator();
    assert_eq!(
        create_tracker(&router, json!({ "TrackerName": "fleet" }))
            .await
            .status,
        200
    );
    assert_eq!(
        batch_update(
            &router,
            "fleet",
            json!([update("device-1", "2024-01-01T00:00:00Z", [-122.4, 37.8])]),
        )
        .await
        .status,
        200
    );

    // The sentence each lookup failure states, written out here rather than read from
    // the crate so a reworded constant fails this test instead of moving with it.
    const NO_RECORDS: &str = "records not found for given deviceId";
    let unknown_tracker = |name: &str| {
        format!("Resource of type \"Tracker\" with name \"{name}\" cannot be found.")
    };

    // The tracker exists; the device has no sample under it.
    for device in ["unknown-device", "Device-1", "device-10", "device_1.a-b"] {
        let got = get_device_position(&router, "fleet", device).await;
        assert_eq!(
            got.status, 404,
            "case {device:?}: expected 404 for a device with no stored position, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.error_code(),
            "ResourceNotFoundException",
            "case {device:?}: expected ResourceNotFoundException, got {:?}",
            got.body
        );
        assert_eq!(
            got.message(),
            NO_RECORDS,
            "case {device:?}: expected the no-records sentence, got {:?}",
            got.message()
        );
    }

    // The tracker itself cannot be resolved, whatever the device id.
    for tracker in ["no-such-fleet", "Fleet"] {
        let got = get_device_position(&router, tracker, "device-1").await;
        assert_eq!(
            got.status, 404,
            "case {tracker:?}: expected 404 for an unknown tracker, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.message(),
            unknown_tracker(tracker),
            "case {tracker:?}: expected the missing-tracker sentence, got {:?}",
            got.message()
        );
    }
}
