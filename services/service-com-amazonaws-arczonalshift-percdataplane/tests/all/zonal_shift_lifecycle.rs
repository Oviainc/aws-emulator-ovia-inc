// SPDX-License-Identifier: CC0-1.0

//! The zonal-shift lifecycle, driven end to end over the wire.
//!
//! Every case here builds the production `MultiServiceRouter`, registers this service
//! on one shared engine, and dispatches real `restJson1` requests — the same path a
//! socket-served SDK request takes — so what is pinned is the HTTP behaviour, not a
//! handler called directly.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a zonal shift a caller starts is PERSISTED, so every later read reports it;
//! * `appliedWeights` is 0 for the Availability Zone the shift in effect moves traffic
//!   away from and 1 for every other zone the resource is deployed in, and is derived
//!   from the shifts in progress — so a canceled or expired shift moves no weight;
//! * `expiresIn` is a whole number of minutes or hours, converted to an expiry time,
//!   and is refused past the documented three-day maximum;
//! * a shift whose expiry time the clock has passed reads back as `EXPIRED`;
//! * a shift that is no longer `ACTIVE` can be neither updated nor canceled.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_arczonalshift_percdataplane::ops::types::{
    APPLIED_STATUS_VALUES, CONFLICT_EXCEPTION_REASON_VALUES, SHIFT_TYPE_VALUES,
    VALIDATION_EXCEPTION_REASON_VALUES, ZONAL_AUTOSHIFT_STATUS_VALUES, ZONAL_SHIFT_STATUS_VALUES,
};
use service_com_amazonaws_arczonalshift_percdataplane::state::{
    parse_expires_in, ShiftType, ZonalAutoshiftStatus, ZonalShiftStatus,
    MAX_COMPLETED_SHIFTS_RETAINED,
};
use service_com_amazonaws_arczonalshift_percdataplane::{
    ComAmazonawsArczonalshiftPercdataplaneState as ZonalShiftStore, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// One emulator: the production router with this service mounted on a shared engine.
///
/// The engine is handed back as well as the router, because the clock-driven case has
/// to move a stored shift's expiry time into the past, which no modeled operation can
/// do.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator core's business, not this service's; off here so a
    // case asserts the service's behaviour rather than its signing.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A buffered `restJson1` request whose SigV4 credential scope names this service's
/// signing name, so the router routes it here.
fn request(method: &str, uri: &str, body: &str) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20240101/us-east-1/arc-zonal-shift/aws4_request, \
             SignedHeaders=host, Signature=deadbeef",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a valid test request")
}

/// Dispatch a call and read back its status and JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {uri}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// Percent-encode the reserved characters an ARN `resourceIdentifier` or a base64
/// `nextToken` carries, so each fits the single URI label or query value it is placed
/// in: `:` and `/` are path/query delimiters, and `+` and `=` are the base64 alphabet's
/// two sub-delimiters (RFC 3986 section 2.2 reserved characters). Written out rather
/// than pulled from an escaping crate so this test crate adds no dependency, and
/// matching the convention of the sibling suites that bind an ARN to a URI label.
fn encoded(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('+', "%2B")
        .replace('=', "%3D")
}

/// The seeded managed resources, as `(resourceIdentifier, availabilityZones)` in the
/// order the store holds them. Read from the engine the router serves, so a case names
/// no ARN of its own.
fn seeded(engine: &engine::InMemoryEngine) -> Vec<(String, Vec<String>)> {
    engine
        .state::<ZonalShiftStore>()
        .lock()
        .managed_resources
        .values()
        .map(|resource| {
            (
                resource.arn.clone(),
                resource.availability_zones.clone(),
            )
        })
        .collect()
}

/// `__type` as the restJson1 error envelope carries it, with any `shapeId#` prefix
/// dropped.
fn error_code(body: &serde_json::Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn start_body(resource: &str, away_from: &str, expires_in: &str, comment: &str) -> String {
    serde_json::json!({
        "resourceIdentifier": resource,
        "awayFrom": away_from,
        "expiresIn": expires_in,
        "comment": comment,
    })
    .to_string()
}

#[tokio::test]
async fn list_managed_resources_reports_the_store_and_pages_through_it() {
    let (router, engine) = emulator();
    let expected: Vec<String> = seeded(&engine)
        .into_iter()
        .map(|(arn, _)| arn)
        .collect();
    assert!(
        expected.len() >= 3,
        "the store must seed enough managed resources to page over, got {}",
        expected.len()
    );

    // One row per page size: the page size changes how many calls the walk takes, never
    // which resources it sees or their order.
    for page_size in [None, Some(1), Some(2), Some(3), Some(100)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut calls = 0;
        loop {
            calls += 1;
            assert!(
                calls <= expected.len() + 1,
                "case {case}: pagination did not terminate after {calls} calls"
            );
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(ref next) = token {
                query.push(format!("nextToken={}", encoded(next)));
            }
            let uri = if query.is_empty() {
                "/managedresources".to_string()
            } else {
                format!("/managedresources?{}", query.join("&"))
            };
            let (status, body) = call(&router, "GET", &uri, "").await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");
            let items = body["items"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: items must be a list, got {body}"));
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size as usize,
                    "case {case}: page of {} exceeds maxResults {size}",
                    items.len()
                );
            }
            for item in items {
                seen.push(item["arn"].as_str().unwrap_or_default().to_string());
                let zones: Vec<String> = item["availabilityZones"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: availabilityZones is required"))
                    .iter()
                    .map(|az| az.as_str().unwrap_or_default().to_string())
                    .collect();
                assert!(
                    !zones.is_empty(),
                    "case {case}: a managed resource is deployed in at least one zone"
                );
                let weights = item["appliedWeights"]
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case}: appliedWeights must be a map"));
                assert_eq!(
                    weights.len(),
                    zones.len(),
                    "case {case}: appliedWeights is keyed by the resource's zones, got {weights:?}"
                );
                for zone in &zones {
                    assert_eq!(
                        weights.get(zone).and_then(serde_json::Value::as_f64),
                        Some(1.0),
                        "case {case}: with no shift in progress every zone carries weight 1, \
                         got {weights:?}"
                    );
                }
                assert_eq!(
                    item["zonalShifts"].as_array().map(Vec::len),
                    Some(0),
                    "case {case}: no shift has been started, got {}",
                    item["zonalShifts"]
                );
                assert_eq!(
                    item["practiceRunStatus"], "DISABLED",
                    "case {case}: no practice run configuration exists, got {}",
                    item["practiceRunStatus"]
                );
            }
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: the walk must return every seeded resource exactly once, in order"
        );
    }
}

#[tokio::test]
async fn a_started_shift_is_persisted_and_every_later_read_reports_it() {
    // The rows differ in which resource is shifted, how many zones it has, which zone
    // traffic moves away from, and the unit the length of time is given in.
    let rows: [(usize, usize, &str, &str); 3] = [
        (0, 3, "4320m", "the longest shift, in minutes"),
        (1, 0, "1h", "bad deploy in the first zone"),
        (2, 1, "30m", "shifting the two-zone resource"),
    ];

    for (resource_index, zone_index, expires_in, comment) in rows {
        let (router, engine) = emulator();
        let resources = seeded(&engine);
        let (resource, zones) = resources[resource_index].clone();
        let away_from = zones[zone_index].clone();
        let case = format!("{} away from {away_from} for {expires_in}", resources[resource_index].0);

        let (status, started) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(&resource, &away_from, expires_in, comment),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case}: StartZonalShift answers 201, got {status} {started}"
        );
        let shift_id = started["zonalShiftId"].as_str().unwrap_or_default().to_string();
        assert!(
            (6..=36).contains(&shift_id.len())
                && shift_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {case}: zonalShiftId must match ^[A-Za-z0-9-]+$ with length 6..36, \
             got {shift_id:?}"
        );
        assert_eq!(
            started["status"], "ACTIVE",
            "case {case}: a shift that has just started is ACTIVE, got {}",
            started["status"]
        );
        assert_eq!(
            started["comment"], comment,
            "case {case}: the stored comment is returned, got {}",
            started["comment"]
        );
        let start_time = started["startTime"].as_f64().expect("startTime");
        let expiry_time = started["expiryTime"].as_f64().expect("expiryTime");
        let expected_length = expected_seconds(expires_in);
        assert!(
            (expiry_time - start_time - expected_length).abs() < 1.0,
            "case {case}: expiryTime is startTime plus {expected_length}s, got {}",
            expiry_time - start_time
        );

        // The resource read reflects the shift: it is the one applied, and the zone it
        // moves away from carries weight 0 while the others carry 1.
        let (status, resource_view) = call(
            &router,
            "GET",
            &format!("/managedresources/{}", encoded(&resource)),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: GetManagedResource answers 200, got {status} {resource_view}"
        );
        assert_eq!(
            weights(&resource_view),
            expected_weights(&zones, Some(&away_from)),
            "case {case}: appliedWeights is 0 for {away_from} and 1 elsewhere"
        );
        let in_resource = resource_view["zonalShifts"].as_array().expect("zonalShifts");
        assert_eq!(
            in_resource.len(),
            1,
            "case {case}: exactly the one shift in progress is listed, got {in_resource:?}"
        );
        assert_eq!(
            in_resource[0]["zonalShiftId"], shift_id,
            "case {case}: the shift just started is the one listed"
        );
        assert_eq!(
            in_resource[0]["appliedStatus"], "APPLIED",
            "case {case}: the only customer-initiated shift is the one in effect, got {}",
            in_resource[0]["appliedStatus"]
        );
        assert_eq!(
            in_resource[0]["shiftType"], "ZONAL_SHIFT",
            "case {case}: a customer-initiated shift is a ZONAL_SHIFT, got {}",
            in_resource[0]["shiftType"]
        );

        // The list reads the same store, and filters on it.
        let listed = list_shifts(&router, &format!("?status=ACTIVE&resourceIdentifier={}", encoded(&resource))).await;
        assert_eq!(
            listed.iter().map(|s| s["zonalShiftId"].as_str().unwrap_or_default()).collect::<Vec<_>>(),
            vec![shift_id.as_str()],
            "case {case}: ListZonalShifts(status=ACTIVE) returns the started shift"
        );
        assert!(
            list_shifts(&router, "?status=CANCELED").await.is_empty(),
            "case {case}: nothing has been canceled yet"
        );

        // An update overwrites the stored comment and recomputes the expiry time, and
        // the next read sees both. The new expiry is measured from an instant taken
        // just before the call, because the handler computes it from the clock then.
        let updated_comment = "extending the shift";
        let before_update = unix_seconds_now();
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/zonalshifts/{shift_id}"),
            &serde_json::json!({ "comment": updated_comment, "expiresIn": "2h" }).to_string(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateZonalShift answers 200, got {status} {updated}"
        );
        assert_eq!(
            updated["comment"], updated_comment,
            "case {case}: a new comment overwrites the old one, got {}",
            updated["comment"]
        );
        let new_expiry = updated["expiryTime"].as_f64().expect("expiryTime");
        assert!(
            (new_expiry - before_update - 2.0 * 3600.0).abs() < 5.0,
            "case {case}: expiresIn=2h expires two hours from the update, got {}s after it",
            new_expiry - before_update
        );
        assert_eq!(
            updated["startTime"].as_f64(),
            Some(start_time),
            "case {case}: an update does not move the time the shift started"
        );
        let listed = list_shifts(&router, &format!("?resourceIdentifier={}", encoded(&resource))).await;
        assert_eq!(
            listed[0]["comment"], updated_comment,
            "case {case}: the update was stored, not just answered, got {}",
            listed[0]["comment"]
        );
        assert_eq!(
            listed[0]["expiryTime"].as_f64(),
            Some(new_expiry),
            "case {case}: the recomputed expiry time was stored"
        );

        // Cancelling releases the zone: the shift stops being in progress and every
        // weight goes back to 1.
        let (status, canceled) =
            call(&router, "DELETE", &format!("/zonalshifts/{shift_id}"), "").await;
        assert_eq!(
            status, 200,
            "case {case}: CancelZonalShift answers 200, got {status} {canceled}"
        );
        assert_eq!(
            canceled["status"], "CANCELED",
            "case {case}: a canceled shift reads CANCELED, got {}",
            canceled["status"]
        );
        let (_, resource_view) = call(
            &router,
            "GET",
            &format!("/managedresources/{}", encoded(&resource)),
            "",
        )
        .await;
        assert_eq!(
            weights(&resource_view),
            expected_weights(&zones, None),
            "case {case}: a canceled shift moves no weight"
        );
        assert_eq!(
            resource_view["zonalShifts"].as_array().map(Vec::len),
            Some(0),
            "case {case}: a canceled shift is no longer in progress for the resource"
        );
        assert_eq!(
            list_shifts(&router, "?status=CANCELED").await.len(),
            1,
            "case {case}: a completed shift is still listed, with its CANCELED status"
        );
        assert!(
            list_shifts(&router, "?status=ACTIVE").await.is_empty(),
            "case {case}: nothing is active once the shift is canceled"
        );
    }
}

#[test]
fn expires_in_is_read_as_a_whole_number_of_minutes_or_hours_and_never_panics() {
    // The grammar the model documents, over a set that spans the ways a value can be
    // outside it — including values whose last character is multi-byte, which must be
    // answered "not of that form" rather than indexed into on a byte offset.
    let accepted: [(&str, u64); 5] = [
        ("1m", 60),
        ("90m", 5_400),
        ("1h", 3_600),
        ("72h", 259_200),
        ("4320m", 259_200),
    ];
    for (value, seconds) in accepted {
        assert_eq!(
            parse_expires_in(value),
            Some(Duration::from_secs(seconds)),
            "case {value:?}: expected {seconds}s"
        );
    }
    for value in [
        "", "m", "h", "0m", "0h", "1", "1d", "1M", "1H", "1 h", "h1", "72h1", "73h", "4321m",
        "9999h", "1µ", "12µ", "1mµ", "١٢m", "1h\u{200b}", "µm",
    ] {
        assert_eq!(
            parse_expires_in(value),
            None,
            "case {value:?}: must be refused, not read as a length of time"
        );
    }
}

#[tokio::test]
async fn expires_in_is_converted_to_an_expiry_time_and_bounded_at_three_days() {
    // Accepted: both units, and each end of the documented range — "up to three days
    // (72 hours)" — expressed in hours and in minutes.
    let accepted: [(&str, f64); 6] = [
        ("1m", 60.0),
        ("30m", 1800.0),
        ("1h", 3600.0),
        ("20h", 72_000.0),
        ("72h", 259_200.0),
        ("4320m", 259_200.0),
    ];
    for (expires_in, expected_length) in accepted {
        let (router, engine) = emulator();
        let (resource, zones) = seeded(&engine)[0].clone();
        let (status, body) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(&resource, &zones[0], expires_in, "within the maximum"),
        )
        .await;
        assert_eq!(
            status, 201,
            "case expiresIn={expires_in}: expected 201, got {status} {body}"
        );
        let length = body["expiryTime"].as_f64().expect("expiryTime")
            - body["startTime"].as_f64().expect("startTime");
        assert!(
            (length - expected_length).abs() < 1.0,
            "case expiresIn={expires_in}: expected {expected_length}s, got {length}s"
        );
    }

    // Refused: one step past each end of the range, in each unit.
    for expires_in in ["73h", "4321m", "9999h"] {
        let (router, engine) = emulator();
        let (resource, zones) = seeded(&engine)[0].clone();
        let (status, body) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(&resource, &zones[0], expires_in, "past the maximum"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case expiresIn={expires_in}: past three days is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case expiresIn={expires_in}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["reason"], "InvalidExpiresIn",
            "case expiresIn={expires_in}: expected reason InvalidExpiresIn, got {body}"
        );
    }
}

#[tokio::test]
async fn unknown_identifiers_are_not_found_and_a_shift_that_is_over_is_a_conflict() {
    let (router, engine) = emulator();
    let resources = seeded(&engine);
    let (resource, zones) = resources[0].clone();

    // An identifier no resource carries is not found, whatever shape it has.
    for unknown in [
        "arn:aws:elasticloadbalancing:us-east-1:111111111111:loadbalancer/app/absent/0",
        "arn:aws:autoscaling:us-west-2:000000000000:autoScalingGroupName/absent",
    ] {
        let (status, body) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(unknown, "use1-az1", "1h", "unknown resource"),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown}: an unregistered resource is not found, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {unknown}: expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!("/managedresources/{}", encoded(unknown)),
            "",
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown}: GetManagedResource is not found, got {status} {body}"
        );
    }

    // A zone the resource is not deployed in cannot be shifted away from.
    for away_from in ["use1-az3", "usw2-az1"] {
        let (status, body) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(&resource, away_from, "1h", "zone the resource is not in"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case awayFrom={away_from}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["reason"], "UnsupportedAz",
            "case awayFrom={away_from}: expected reason UnsupportedAz, got {body}"
        );
    }

    // An id no shift carries is not found, for both operations keyed by one.
    for (method, name) in [("PATCH", "UpdateZonalShift"), ("DELETE", "CancelZonalShift")] {
        let (status, body) = call(&router, method, "/zonalshifts/zs-0000009999", "{}").await;
        assert_eq!(
            status, 404,
            "case {name}: an unknown zonalShiftId is not found, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name}: expected ResourceNotFoundException, got {body}"
        );
    }

    let (status, first) = call(
        &router,
        "POST",
        "/zonalshifts",
        &start_body(&resource, &zones[0], "1h", "the first shift"),
    )
    .await;
    assert_eq!(status, 201, "the first shift starts: {first}");
    let shift_id = first["zonalShiftId"].as_str().unwrap_or_default().to_string();

    // A second customer-initiated shift for the same resource conflicts with the first,
    // whichever zone it names.
    for away_from in [zones[0].clone(), zones[1].clone()] {
        let (status, body) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(&resource, &away_from, "1h", "the second shift"),
        )
        .await;
        assert_eq!(
            status, 409,
            "case second shift away from {away_from}: expected 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case second shift away from {away_from}: expected ConflictException, got {body}"
        );
        assert_eq!(
            body["reason"], "ZonalShiftAlreadyExists",
            "case second shift away from {away_from}: expected reason \
             ZonalShiftAlreadyExists, got {body}"
        );
        assert_eq!(
            body["zonalShiftId"], shift_id,
            "case second shift away from {away_from}: the conflict names the shift that \
             already exists, got {body}"
        );
    }

    // Another resource is unaffected: the conflict is per resource, not global.
    let (other_resource, other_zones) = resources[1].clone();
    let (status, body) = call(
        &router,
        "POST",
        "/zonalshifts",
        &start_body(&other_resource, &other_zones[0], "1h", "a different resource"),
    )
    .await;
    assert_eq!(
        status, 201,
        "a shift for a different resource is not a conflict, got {status} {body}"
    );

    // Once a shift is over, neither operation that changes it is allowed.
    let (status, _) = call(&router, "DELETE", &format!("/zonalshifts/{shift_id}"), "").await;
    assert_eq!(status, 200, "the first shift cancels");
    for (method, body_text, name) in [
        ("DELETE", "", "cancel twice"),
        ("PATCH", r#"{"comment":"too late"}"#, "update after cancel"),
    ] {
        let (status, body) =
            call(&router, method, &format!("/zonalshifts/{shift_id}"), body_text).await;
        assert_eq!(
            status, 409,
            "case {name}: a shift that is not ACTIVE is a conflict, got {status} {body}"
        );
        assert_eq!(
            body["reason"], "ZonalShiftStatusNotActive",
            "case {name}: expected reason ZonalShiftStatusNotActive, got {body}"
        );
    }

    // A token this service never issued names no page.
    for token in ["not-a-token", "zzzz"] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/zonalshifts?nextToken={}", encoded(token)),
            "",
        )
        .await;
        assert_eq!(
            status, 400,
            "case nextToken={token}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["reason"], "InvalidToken",
            "case nextToken={token}: expected reason InvalidToken, got {body}"
        );
    }

    // A `status` filter outside the model's `ZonalShiftStatus` enum is refused, not
    // served as "nothing matched": the model carries a `ValidationExceptionReason` of
    // `InvalidStatus` for exactly this.
    for status_filter in ["FOO", "active", "ACTIVE,CANCELED"] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/zonalshifts?status={}", encoded(status_filter)),
            "",
        )
        .await;
        assert_eq!(
            status, 400,
            "case status={status_filter}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case status={status_filter}: the modelled enum constraint refuses it as a \
             ValidationException, got {body}"
        );
    }

    // A `maxResults` outside the modelled range (1..100), and a `resourceIdentifier`
    // shorter than the modelled minimum length, are refused rather than coerced.
    for (query, name) in [
        ("maxResults=0", "maxResults below the range"),
        ("maxResults=101", "maxResults above the range"),
        ("maxResults=-5", "negative maxResults"),
        ("resourceIdentifier=short", "resourceIdentifier under the minimum length"),
    ] {
        let (status, body) = call(&router, "GET", &format!("/zonalshifts?{query}"), "").await;
        assert_eq!(
            status, 400,
            "case {name}: expected 400, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_shift_whose_expiry_the_clock_has_passed_reads_back_as_expired() {
    let (router, engine) = emulator();
    let (resource, zones) = seeded(&engine)[0].clone();
    let (status, started) = call(
        &router,
        "POST",
        "/zonalshifts",
        &start_body(&resource, &zones[0], "1h", "a shift that will expire"),
    )
    .await;
    assert_eq!(status, 201, "the shift starts: {started}");
    let shift_id = started["zonalShiftId"].as_str().unwrap_or_default().to_string();

    // Move the clock past the shift's expiry, the one thing no modeled operation can
    // do: reach into the store and backdate the expiry time.
    {
        let store = engine.state::<ZonalShiftStore>();
        let mut store = store.lock();
        let shift = store
            .zonal_shifts
            .get_mut(&shift_id)
            .expect("the started shift is in the store");
        shift.expiry_time = SystemTime::now() - Duration::from_secs(1);
    }

    let listed = list_shifts(&router, &format!("?resourceIdentifier={}", encoded(&resource))).await;
    assert_eq!(
        listed[0]["status"], "EXPIRED",
        "an expired shift reads back as EXPIRED, got {}",
        listed[0]["status"]
    );
    assert!(
        list_shifts(&router, "?status=ACTIVE").await.is_empty(),
        "an expired shift is no longer ACTIVE"
    );

    let (_, resource_view) = call(
        &router,
        "GET",
        &format!("/managedresources/{}", encoded(&resource)),
        "",
    )
    .await;
    assert_eq!(
        weights(&resource_view),
        expected_weights(&zones, None),
        "an expired shift moves no weight"
    );
    assert_eq!(
        resource_view["zonalShifts"].as_array().map(Vec::len),
        Some(0),
        "an expired shift is no longer in progress for the resource"
    );

    // The resource is free again, so a new shift for it is not a conflict.
    let (status, body) = call(
        &router,
        "POST",
        "/zonalshifts",
        &start_body(&resource, &zones[1], "15m", "after the first expired"),
    )
    .await;
    assert_eq!(
        status, 201,
        "a resource whose shift expired takes a new shift, got {status} {body}"
    );
    assert_eq!(
        weights(
            &call(
                &router,
                "GET",
                &format!("/managedresources/{}", encoded(&resource)),
                ""
            )
            .await
            .1
        ),
        expected_weights(&zones, Some(&zones[1])),
        "the new shift moves the weight of the zone it names"
    );
}

#[tokio::test]
async fn the_store_keeps_a_bounded_number_of_completed_shifts() {
    // Starting and cancelling repeatedly is unbounded work a caller can drive, so the
    // store keeps at most MAX_COMPLETED_SHIFTS_RETAINED completed shifts and drops the
    // oldest first. The shift still in progress is never dropped.
    let (router, engine) = emulator();
    let resources = seeded(&engine);
    let mut ids: Vec<String> = Vec::new();
    // One past the cap, so the oldest must have been dropped.
    for round in 0..=MAX_COMPLETED_SHIFTS_RETAINED {
        let (resource, zones) = &resources[round % resources.len()];
        let (status, started) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(resource, &zones[0], "5m", "one of many"),
        )
        .await;
        assert_eq!(status, 201, "round {round}: the shift starts, got {started}");
        let id = started["zonalShiftId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, canceled) = call(&router, "DELETE", &format!("/zonalshifts/{id}"), "").await;
        assert_eq!(
            status, 200,
            "round {round}: the shift cancels, got {canceled}"
        );
        ids.push(id);
    }

    let held = engine.state::<ZonalShiftStore>().lock().zonal_shifts.len();
    assert!(
        held <= MAX_COMPLETED_SHIFTS_RETAINED,
        "the store keeps at most {MAX_COMPLETED_SHIFTS_RETAINED} completed shifts, holds {held}"
    );
    let oldest = ids.first().expect("at least one shift was started");
    let newest = ids.last().expect("at least one shift was started");
    let (status, body) = call(&router, "DELETE", &format!("/zonalshifts/{oldest}"), "").await;
    assert_eq!(
        status, 404,
        "the oldest completed shift was dropped first, got {status} {body}"
    );
    let (status, body) = call(&router, "DELETE", &format!("/zonalshifts/{newest}"), "").await;
    assert_eq!(
        status, 409,
        "the newest completed shift is still held, so cancelling it again conflicts: \
         got {status} {body}"
    );
}

#[tokio::test]
async fn every_wire_value_this_service_writes_is_one_the_model_declares() {
    // The catalogs come from the Smithy model (the generated `*_VALUES` constants in
    // `ops::types`), so a status or reason this service invents fails here.
    for status in [
        ZonalShiftStatus::Active,
        ZonalShiftStatus::Expired,
        ZonalShiftStatus::Canceled,
    ] {
        assert!(
            ZONAL_SHIFT_STATUS_VALUES.contains(&status.as_wire()),
            "case {status:?}: {} is not a ZonalShiftStatus the model declares",
            status.as_wire()
        );
    }
    for shift_type in [
        ShiftType::ZonalShift,
        ShiftType::FisExperiment,
        ShiftType::ZonalAutoshift,
        ShiftType::PracticeRun,
    ] {
        assert!(
            SHIFT_TYPE_VALUES.contains(&shift_type.as_wire()),
            "case {shift_type:?}: {} is not a ShiftType the model declares",
            shift_type.as_wire()
        );
    }
    for status in [ZonalAutoshiftStatus::Disabled, ZonalAutoshiftStatus::Enabled] {
        assert!(
            ZONAL_AUTOSHIFT_STATUS_VALUES.contains(&status.as_wire()),
            "case {status:?}: {} is not a ZonalAutoshiftStatus the model declares",
            status.as_wire()
        );
    }
    for applied_status in ["APPLIED", "NOT_APPLIED"] {
        assert!(
            APPLIED_STATUS_VALUES.contains(&applied_status),
            "case {applied_status}: not an AppliedStatus the model declares"
        );
    }
    for reason in ["ZonalShiftAlreadyExists", "ZonalShiftStatusNotActive"] {
        assert!(
            CONFLICT_EXCEPTION_REASON_VALUES.contains(&reason),
            "case {reason}: not a ConflictExceptionReason the model declares"
        );
    }
    for reason in ["InvalidExpiresIn", "UnsupportedAz", "InvalidToken"] {
        assert!(
            VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
            "case {reason}: not a ValidationExceptionReason the model declares"
        );
    }
}

#[tokio::test]
async fn the_operations_this_slice_leaves_unimplemented_still_answer_501() {
    let (router, engine) = emulator();
    let (resource, zones) = seeded(&engine)[0].clone();
    // A practice run is a separate slice: it answers "not written yet", never an empty
    // success.
    let (status, body) = call(
        &router,
        "POST",
        "/practiceruns",
        &serde_json::json!({
            "resourceIdentifier": resource,
            "awayFrom": zones[0],
            "comment": "not implemented yet",
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 501,
        "StartPracticeRun is still a stub, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotImplemented",
        "a stub answers NotImplemented, got {body}"
    );
}

/// `ListZonalShifts` items for a query string.
async fn list_shifts(router: &MultiServiceRouter, query: &str) -> Vec<serde_json::Value> {
    let (status, body) = call(router, "GET", &format!("/zonalshifts{query}"), "").await;
    assert_eq!(status, 200, "ListZonalShifts{query}: got {status} {body}");
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `appliedWeights` of a resource view, as a sorted map.
fn weights(resource_view: &serde_json::Value) -> BTreeMap<String, f64> {
    resource_view["appliedWeights"]
        .as_object()
        .unwrap_or_else(|| panic!("appliedWeights is required, got {resource_view}"))
        .iter()
        .map(|(zone, weight)| (zone.clone(), weight.as_f64().unwrap_or(f64::NAN)))
        .collect()
}

/// The weights a resource deployed in `zones` must report while traffic is moved away
/// from `away_from`: 0 there, 1 everywhere else.
fn expected_weights(zones: &[String], away_from: Option<&String>) -> BTreeMap<String, f64> {
    zones
        .iter()
        .map(|zone| {
            let weight = if Some(zone) == away_from { 0.0 } else { 1.0 };
            (zone.clone(), weight)
        })
        .collect()
}

/// The number of seconds since the Unix epoch, right now — the instant the service's
/// own clock reads when it converts an `expiresIn` into an expiry time.
fn unix_seconds_now() -> f64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// The number of seconds an `expiresIn` names: a whole number followed by `m`
/// (minutes) or `h` (hours), as the model documents.
fn expected_seconds(expires_in: &str) -> f64 {
    if let Some(minutes) = expires_in.strip_suffix('m') {
        minutes.parse::<f64>().expect("a whole number") * 60.0
    } else if let Some(hours) = expires_in.strip_suffix('h') {
        hours.parse::<f64>().expect("a whole number") * 3600.0
    } else {
        panic!("expiresIn {expires_in} is not a form the model allows")
    }
}
