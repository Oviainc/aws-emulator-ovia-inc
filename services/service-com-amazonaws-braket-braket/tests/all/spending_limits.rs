// SPDX-License-Identifier: CC0-1.0

//! Braket spending limits, end to end over HTTP.
//!
//! The rules these suites pin:
//!
//! * `CreateSpendingLimit` mints a spending-limit ARN for a device whose ARN names a
//!   quantum processor and PERSISTS a record, which `SearchSpendingLimits` reads back: the
//!   amount, the period and the tags come back exactly as they were written, and a
//!   request that sends no `timePeriod` gets the default window.
//! * `clientToken` is an `@idempotencyToken`: the same token with the same request
//!   returns the first ARN and creates nothing; with a different request it is refused.
//! * The device decides whether a limit may exist at all: a device the inventory publishes
//!   as RETIRED is `DeviceRetiredException` (410), a SIMULATOR is `ValidationException`
//!   (400) — "Simulators do not support spending limits" — and every other quantum
//!   processor is accepted, OFFLINE included and whether or not the inventory publishes it.
//! * `queuedSpend` and `totalSpend` are COMPUTED from the device's quantum tasks over
//!   the limit's own period, at the per-shot price the device's capability document
//!   publishes.
//! * The search is filtered (on a limit's own ARN or its device's, `EQUAL` only) and
//!   paginated.
//!
//! Each suite asserts its rule over a TABLE spanning the class rather than one recorded
//! call: the rows differ along the amount's form, the period's length and presence, the
//! tag count, the device (every published row, by type and status), the shot count and
//! the page size. Everything goes through the production router, reusing the harness in
//! `quantum_task_loop.rs`, so what is pinned is what a Braket client sees.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_braket_braket::state::DEFAULT_SPENDING_LIMIT_WINDOW;

use crate::quantum_task_loop::{create_body, encode, member, Braket, Reply};

/// The region the harness's SigV4 credential scope names, so a minted ARN carries it.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The status Amazon Braket answers a created spending limit with.
pub(crate) const CREATED: u16 = 200;

/// `POST /spending-limit`.
pub(crate) async fn create_limit(braket: &Braket, body: Value) -> Reply {
    braket.call("POST", "/spending-limit", Some(body)).await
}

/// `POST /spending-limits`.
async fn search_limits(braket: &Braket, body: Value) -> Reply {
    braket.call("POST", "/spending-limits", Some(body)).await
}

/// The `spendingLimits` list of a successful search.
pub(crate) async fn limits_on(braket: &Braket, device_arn: &str) -> Vec<Value> {
    let reply = search_limits(
        braket,
        json!({
            "filters": [{ "name": "deviceArn", "operator": "EQUAL", "values": [device_arn] }],
            "maxResults": 100,
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "SearchSpendingLimits on {device_arn}: expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .body
        .get("spendingLimits")
        .and_then(Value::as_array)
        .cloned()
        .expect("spendingLimits is a required output member")
}

/// The summary carrying `arn`, from a search of its device.
pub(crate) async fn limit_by_arn(braket: &Braket, device_arn: &str, arn: &str) -> Value {
    limits_on(braket, device_arn)
        .await
        .into_iter()
        .find(|limit| member(limit, "spendingLimitArn") == arn)
        .unwrap_or_else(|| panic!("SearchSpendingLimits does not report {arn}"))
}

/// A `CreateSpendingLimit` body with the required members filled in.
pub(crate) fn limit_body(client_token: &str, device_arn: &str, amount: &str) -> Value {
    json!({
        "clientToken": client_token,
        "deviceArn": device_arn,
        "spendingLimit": amount,
    })
}

/// Seconds since the epoch, as `TimePeriod`'s `epoch-seconds` members carry.
pub(crate) fn now_epoch() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs(),
    )
    .expect("an epoch second fits in i64")
}

/// A `TimePeriod` JSON object.
pub(crate) fn period(start: i64, end: i64) -> Value {
    json!({ "startAt": start, "endAt": end })
}

/// The epoch-seconds member of a reported `TimePeriod`.
pub(crate) fn period_second(limit: &Value, name: &str) -> f64 {
    limit
        .get("timePeriod")
        .and_then(|p| p.get(name))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("the summary has no timePeriod.{name}: {limit}"))
}

/// What one charged unit costs on `device_arn` and the unit it is charged per, read
/// from the device's own published capability document (`service.deviceCost`).
async fn published_price(braket: &Braket, device_arn: &str) -> (f64, String) {
    let reply = braket.get_device(device_arn).await;
    assert_eq!(
        reply.status, 200,
        "GetDevice {device_arn}: expected 200, got {} {}",
        reply.status, reply.body
    );
    let capabilities: Value = serde_json::from_str(&reply.str_member("deviceCapabilities"))
        .expect("deviceCapabilities is a JSON document");
    let cost = capabilities
        .get("service")
        .and_then(|s| s.get("deviceCost"))
        .cloned()
        .unwrap_or_else(|| panic!("{device_arn} publishes no service.deviceCost"));
    let price = cost
        .get("price")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{device_arn} publishes no deviceCost.price"));
    (price, member(&cost, "unit").to_string())
}

/// The ARN of a device the inventory publishes with `device_type` and `status`.
pub(crate) async fn a_device(braket: &Braket, device_type: &str, status: &str) -> String {
    let inventory = braket.inventory().await;
    let device = inventory
        .iter()
        .find(|d| member(d, "deviceType") == device_type && member(d, "deviceStatus") == status)
        .unwrap_or_else(|| panic!("the inventory holds no {status} {device_type}"));
    member(device, "deviceArn").to_string()
}

/// The ARNs of every device the inventory publishes with `device_type` and `status`.
async fn devices(braket: &Braket, device_type: &str, status: &str) -> Vec<String> {
    braket
        .inventory()
        .await
        .iter()
        .filter(|d| member(d, "deviceType") == device_type && member(d, "deviceStatus") == status)
        .map(|d| member(d, "deviceArn").to_string())
        .collect()
}

/// Assert `arn` is a spending-limit ARN of the form the model's `SpendingLimitArn`
/// pattern spells (`^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$`),
/// carrying the request's own region and the emulator account.
fn assert_spending_limit_arn(arn: &str, case: &str) {
    let prefix = format!("arn:aws:braket:{REGION}:{ACCOUNT}:spending-limit/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an ARN starting {prefix}, got {arn:?}"
    );
    assert!(
        arn.len() > prefix.len(),
        "case {case:?}: the ARN carries no resource id: {arn:?}"
    );
    assert!(
        arn.len() <= 256,
        "case {case:?}: SpendingLimitArn is @length(max: 256), got {} characters",
        arn.len()
    );
}

/// A created limit is persisted and read back member for member: the amount and the
/// period exactly as written, the tags, and a fresh limit with nothing spent yet.
#[tokio::test]
async fn a_created_spending_limit_round_trips_through_the_search() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();

    // (case, amount, period, tags) — the rows differ along the amount's form (whole,
    // one decimal, two decimals, zero, large), the period (absent, one second, a month,
    // a decade) and the tag count (none, one, several).
    let rows: Vec<(&str, &str, Option<Value>, Option<Value>)> = vec![
        ("a whole amount, no period, no tags", "100", None, None),
        (
            "two decimal places and a month-long period",
            "100.50",
            Some(period(start, start + 30 * 24 * 60 * 60)),
            None,
        ),
        (
            "one decimal place and a one-second period",
            "0.5",
            Some(period(start, start + 1)),
            None,
        ),
        (
            "a zero limit and a decade-long period",
            "0",
            Some(period(start, start + 10 * 365 * 24 * 60 * 60)),
            None,
        ),
        (
            "a large amount and one tag",
            "1000000",
            Some(period(start - 60, start + 60)),
            Some(json!({ "team": "quantum" })),
        ),
        (
            "several tags",
            "250.25",
            Some(period(start, start + 3600)),
            Some(json!({ "team": "quantum", "env": "test", "owner": "ada" })),
        ),
        (
            "a period that starts in the past and ends far in the future",
            "12.34",
            Some(period(0, start + 86_400)),
            None,
        ),
    ];

    for (index, (case, amount, time_period, tags)) in rows.iter().enumerate() {
        let mut body = limit_body(&format!("tok-round-trip-{index}"), &device, amount);
        if let Some(time_period) = time_period {
            body["timePeriod"] = time_period.clone();
        }
        if let Some(tags) = tags {
            body["tags"] = tags.clone();
        }
        let reply = create_limit(&braket, body).await;
        assert_eq!(
            reply.status, CREATED,
            "case {case:?}: expected {CREATED}, got {} {}",
            reply.status, reply.body
        );
        let arn = reply.str_member("spendingLimitArn");
        assert_spending_limit_arn(&arn, case);

        let limit = limit_by_arn(&braket, &device, &arn).await;
        assert_eq!(
            member(&limit, "spendingLimit"),
            *amount,
            "case {case:?}: the amount must come back as written, got {limit}"
        );
        assert_eq!(
            member(&limit, "deviceArn"),
            device,
            "case {case:?}: the limit must name the device it was created for, got {limit}"
        );
        match time_period {
            Some(sent) => {
                let sent_start = sent["startAt"].as_f64().expect("a numeric startAt");
                let sent_end = sent["endAt"].as_f64().expect("a numeric endAt");
                assert!(
                    (period_second(&limit, "startAt") - sent_start).abs() < f64::EPSILON,
                    "case {case:?}: startAt must round-trip exactly, sent {sent_start}, \
                     got {}",
                    period_second(&limit, "startAt")
                );
                assert!(
                    (period_second(&limit, "endAt") - sent_end).abs() < f64::EPSILON,
                    "case {case:?}: endAt must round-trip exactly, sent {sent_end}, got {}",
                    period_second(&limit, "endAt")
                );
            }
            None => {
                // No period sent: the limit covers the default window from its creation.
                let reported = period_second(&limit, "endAt") - period_second(&limit, "startAt");
                #[allow(clippy::cast_precision_loss)]
                let expected = DEFAULT_SPENDING_LIMIT_WINDOW.as_secs() as f64;
                assert!(
                    (reported - expected).abs() < 1.0,
                    "case {case:?}: a limit created without a timePeriod must cover the \
                     default window of {expected} s, got {reported} s"
                );
            }
        }
        match tags {
            Some(sent) => {
                assert_eq!(
                    limit.get("tags"),
                    Some(sent),
                    "case {case:?}: the tags must come back as written, got {limit}"
                );
            }
            None => assert!(
                limit.get("tags").is_none_or(Value::is_null),
                "case {case:?}: a limit created with no tags must report none, got {limit}"
            ),
        }
        assert_eq!(
            limit.get("createdAt"),
            limit.get("updatedAt"),
            "case {case:?}: a limit nothing has changed was last updated when it was \
             created, got {limit}"
        );
        assert_eq!(
            member(&limit, "queuedSpend"),
            "0.00",
            "case {case:?}: no task is queued on the device, got {limit}"
        );
        assert_eq!(
            member(&limit, "totalSpend"),
            "0.00",
            "case {case:?}: nothing has been spent on the device, got {limit}"
        );
    }

    assert_eq!(
        limits_on(&braket, &device).await.len(),
        rows.len(),
        "every created limit is persisted, and no more than those"
    );
}

/// Whether a limit may be created at all is decided by the device: every published row
/// of the inventory, by type and status, plus the ARNs that name a device the inventory
/// does not publish and the ARNs that name no device at all.
#[tokio::test]
async fn the_device_decides_whether_a_spending_limit_may_be_created() {
    let braket = Braket::new();

    // (case, deviceArn, expected status, expected error) — one row per published
    // device, so the table spans both device types and all three statuses.
    let mut rows: Vec<(String, String, u16, Option<&str>)> = Vec::new();
    for device in braket.inventory().await {
        let arn = member(&device, "deviceArn").to_string();
        let device_type = member(&device, "deviceType").to_string();
        let status = member(&device, "deviceStatus").to_string();
        let expected = if status == "RETIRED" {
            // A retired device is gone, whatever it was.
            (410, Some("DeviceRetiredException"))
        } else if device_type == "SIMULATOR" {
            // "Simulators do not support spending limits."
            (400, Some("ValidationException"))
        } else {
            // Every other device accepts one — OFFLINE included, because a device being
            // temporarily offline does not stop a limit being set on it.
            (CREATED, None)
        };
        rows.push((
            format!("{device_type} {status} {arn}"),
            arn,
            expected.0,
            expected.1,
        ));
    }
    // A quantum processor this emulator's inventory does not publish is still a quantum
    // processor: a cost control is about the device the ARN names, so the admission turns
    // on the KIND in the ARN rather than on the inventory.
    rows.push((
        "a QPU ARN the inventory does not publish".to_string(),
        "arn:aws:braket:::device/qpu/acme/Nonesuch".to_string(),
        CREATED,
        None,
    ));
    rows.push((
        "a QPU ARN of another region and partition".to_string(),
        "arn:aws-cn:braket:cn-north-1::device/qpu/acme/Other".to_string(),
        CREATED,
        None,
    ));
    // A simulator ARN is refused on the same reading, published or not.
    rows.push((
        "a simulator ARN the inventory does not publish".to_string(),
        "arn:aws:braket:::device/quantum-simulator/acme/Nonesuch".to_string(),
        400,
        Some("ValidationException"),
    ));
    // `deviceArn` targets an `@arnReference` shape with no `@pattern`, so these reach
    // the handler: neither names a device, and this operation models no
    // `ResourceNotFoundException`, so both are invalid input.
    rows.push((
        "not an ARN at all".to_string(),
        "not-a-device-arn".to_string(),
        400,
        Some("ValidationException"),
    ));
    rows.push((
        "a device ARN whose kind Amazon Braket publishes nothing under".to_string(),
        "arn:aws:braket:::device/quantum-annealer/acme/Nonesuch".to_string(),
        400,
        Some("ValidationException"),
    ));
    // A value that stops short of the published form names no device either, whatever its
    // category segment reads as.
    rows.push((
        "a device ARN carrying a category but no provider or name".to_string(),
        "arn:aws:braket:::device/qpu".to_string(),
        400,
        Some("ValidationException"),
    ));
    rows.push((
        "a device ARN carrying no device name".to_string(),
        "arn:aws:braket:us-west-1::device/qpu/acme".to_string(),
        400,
        Some("ValidationException"),
    ));
    rows.push((
        "a device ARN whose device name is empty".to_string(),
        "arn:aws:braket:us-west-1::device/qpu/acme/".to_string(),
        400,
        Some("ValidationException"),
    ));

    for (index, (case, device_arn, status, error)) in rows.iter().enumerate() {
        let reply = create_limit(
            &braket,
            limit_body(&format!("tok-device-{index}"), device_arn, "100.00"),
        )
        .await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            *error,
            "case {case:?}: expected {error:?}, got {:?} in {}",
            reply.error,
            reply.body
        );
        if *status == CREATED {
            let arn = reply.str_member("spendingLimitArn");
            assert_spending_limit_arn(&arn, case);
            let limit = limit_by_arn(&braket, device_arn, &arn).await;
            assert_eq!(
                member(&limit, "deviceArn"),
                device_arn,
                "case {case:?}: the stored limit must name its device, got {limit}"
            );
        } else {
            assert!(
                limits_on(&braket, device_arn).await.is_empty(),
                "case {case:?}: a refused create must persist nothing"
            );
        }
    }
}

/// A period whose end does not follow its start covers no time, so it is refused.
#[tokio::test]
async fn a_time_period_that_covers_no_time_is_refused() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();

    // (case, period) — the rows differ in how far the end falls before the start.
    let rows: Vec<(&str, Value)> = vec![
        ("the end equals the start", period(start, start)),
        ("the end is one second before the start", period(start, start - 1)),
        ("the end is a year before the start", period(start, start - 31_536_000)),
        ("the end is the epoch", period(start, 0)),
    ];

    for (index, (case, time_period)) in rows.iter().enumerate() {
        let mut body = limit_body(&format!("tok-period-{index}"), &device, "100.00");
        body["timePeriod"] = time_period.clone();
        let reply = create_limit(&braket, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );
    }

    assert!(
        limits_on(&braket, &device).await.is_empty(),
        "no refused create persisted a limit"
    );
}

/// `clientToken` is an `@idempotencyToken`: the same token with the same request is the
/// same limit, and with a different request it is a reuse and refused.
#[tokio::test]
async fn the_client_token_makes_a_create_idempotent() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let other_device = devices(&braket, "QPU", "ONLINE")
        .await
        .into_iter()
        .find(|arn| *arn != device)
        .expect("the inventory holds two ONLINE QPUs");
    let start = now_epoch();

    let token = "tok-idempotent";
    let mut first = limit_body(token, &device, "100.50");
    first["timePeriod"] = period(start, start + 3600);
    first["tags"] = json!({ "team": "quantum" });

    let created = create_limit(&braket, first.clone()).await;
    assert_eq!(
        created.status, CREATED,
        "the first create: expected {CREATED}, got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("spendingLimitArn");

    // Replaying the same request, several times over, is the same limit.
    for attempt in 1..=3 {
        let replay = create_limit(&braket, first.clone()).await;
        assert_eq!(
            replay.status, CREATED,
            "replay {attempt}: expected {CREATED}, got {} {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.str_member("spendingLimitArn"),
            arn,
            "replay {attempt}: a replayed token must return the first ARN"
        );
    }
    assert_eq!(
        limits_on(&braket, &device).await.len(),
        1,
        "three replays of one token created one limit"
    );

    // (case, the member that differs) — one row per member of the stored record.
    let mut rows: Vec<(&str, Value)> = Vec::new();
    let mut body = first.clone();
    body["spendingLimit"] = json!("200.00");
    rows.push(("a different amount", body));
    let mut body = first.clone();
    body["timePeriod"] = period(start, start + 7200);
    rows.push(("a different period", body));
    let mut body = first.clone();
    body["tags"] = json!({ "team": "chemistry" });
    rows.push(("different tags", body));
    let mut body = first.clone();
    body.as_object_mut().expect("an object").remove("tags");
    rows.push(("no tags at all", body));
    let mut body = first.clone();
    body["deviceArn"] = json!(other_device);
    rows.push(("a different device", body));

    for (case, body) in rows {
        let reply = create_limit(&braket, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token reused for a different request is refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );
    }
    assert_eq!(
        limits_on(&braket, &device).await.len(),
        1,
        "no refused reuse created a second limit"
    );
    assert!(
        limits_on(&braket, &other_device).await.is_empty(),
        "a refused reuse created nothing on the other device either"
    );
}

/// `queuedSpend` is computed from the device's queued quantum tasks at the price the
/// device publishes, and only over the limit's own period; `totalSpend` is what has
/// actually been charged, and this emulator executes nothing, so nothing is charged.
#[tokio::test]
async fn the_reported_spend_is_computed_from_the_devices_quantum_tasks() {
    // (case, shots, whether the limit's period covers now)
    let rows: &[(&str, i64, bool)] = &[
        ("one shot", 1, true),
        ("ten shots", 10, true),
        ("a hundred shots", 100, true),
        ("a thousand shots", 1_000, true),
        ("ten shots, a period that has not started", 10, false),
    ];

    for (index, (case, shots, covers_now)) in rows.iter().enumerate() {
        // A fresh emulator per row, so each row's tasks are the only ones on its device.
        let braket = Braket::new();
        let device = a_device(&braket, "QPU", "ONLINE").await;
        let (price, unit) = published_price(&braket, &device).await;
        assert_eq!(
            unit, "shot",
            "case {case:?}: a QPU is charged per shot, got {unit:?}"
        );
        let start = now_epoch();
        let mut body = limit_body(&format!("tok-spend-{index}"), &device, "1000000");
        body["timePeriod"] = if *covers_now {
            period(start - 3600, start + 3600)
        } else {
            period(start + 86_400, start + 172_800)
        };
        let created = create_limit(&braket, body).await;
        assert_eq!(
            created.status, CREATED,
            "case {case:?}: expected {CREATED}, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("spendingLimitArn");

        let before = limit_by_arn(&braket, &device, &arn).await;
        assert_eq!(
            member(&before, "queuedSpend"),
            "0.00",
            "case {case:?}: nothing is queued yet, got {before}"
        );

        let task = braket
            .create_task(create_body(&format!("task-spend-{index}"), &device, *shots))
            .await;
        assert_eq!(
            task.status, 201,
            "case {case:?}: CreateQuantumTask expected 201, got {} {}",
            task.status, task.body
        );

        #[allow(clippy::cast_precision_loss)]
        let committed = price * (*shots as f64);
        let expected_queued = if *covers_now {
            format!("{committed:.2}")
        } else {
            // The task was created outside the period, so it is not this period's spend.
            "0.00".to_string()
        };
        let after = limit_by_arn(&braket, &device, &arn).await;
        assert_eq!(
            member(&after, "queuedSpend"),
            expected_queued,
            "case {case:?}: {shots} shots at {price} USD each must be reported as \
             {expected_queued}, got {after}"
        );
        assert_eq!(
            member(&after, "totalSpend"),
            "0.00",
            "case {case:?}: no task has completed, so nothing has been charged, got {after}"
        );
    }
}

/// The search filters on `deviceArn` and refuses anything else, and it pages through
/// the matches with an opaque token.
#[tokio::test]
async fn the_search_filters_and_pages_over_the_stored_limits() {
    let braket = Braket::new();
    let online = devices(&braket, "QPU", "ONLINE").await;
    let (first_device, second_device) = (online[0].clone(), online[1].clone());

    // Five limits on one device, one on another: enough to page, and enough for a
    // filter to exclude something.
    let mut on_first: BTreeSet<String> = BTreeSet::new();
    for index in 0..5 {
        let reply = create_limit(
            &braket,
            limit_body(&format!("tok-page-{index}"), &first_device, "100.00"),
        )
        .await;
        assert_eq!(
            reply.status, CREATED,
            "limit {index}: expected {CREATED}, got {} {}",
            reply.status, reply.body
        );
        on_first.insert(reply.str_member("spendingLimitArn"));
    }
    let reply = create_limit(
        &braket,
        limit_body("tok-page-other", &second_device, "250.00"),
    )
    .await;
    assert_eq!(
        reply.status, CREATED,
        "the other device's limit: expected {CREATED}, got {} {}",
        reply.status, reply.body
    );
    let on_second = reply.str_member("spendingLimitArn");

    // A filter keeps only its own device's limits; two values keep both devices'.
    let filtered: BTreeSet<String> = limits_on(&braket, &first_device)
        .await
        .iter()
        .map(|limit| member(limit, "spendingLimitArn").to_string())
        .collect();
    assert_eq!(
        filtered, on_first,
        "a deviceArn filter reports exactly that device's limits"
    );
    let both = search_limits(
        &braket,
        json!({
            "filters": [{
                "name": "deviceArn",
                "operator": "EQUAL",
                "values": [first_device, second_device],
            }],
            "maxResults": 100,
        }),
    )
    .await;
    assert_eq!(
        both.body
            .get("spendingLimits")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(6),
        "a filter with two values matches either device: {}",
        both.body
    );

    // No filter at all reports every stored limit.
    let unfiltered = search_limits(&braket, json!({ "maxResults": 100 })).await;
    assert_eq!(
        unfiltered
            .body
            .get("spendingLimits")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(6),
        "an unfiltered search reports every limit: {}",
        unfiltered.body
    );

    // (case, maxResults) — the page size the traversal is driven at.
    for (case, page_size) in [
        ("one at a time", 1_usize),
        ("two at a time", 2),
        ("three at a time", 3),
        ("exactly the match count", 5),
        ("more than the match count", 100),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "filters": [{
                    "name": "deviceArn",
                    "operator": "EQUAL",
                    "values": [first_device],
                }],
                "maxResults": page_size,
            });
            if let Some(token) = token.as_deref() {
                body["nextToken"] = json!(token);
            }
            let reply = search_limits(&braket, body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} {}",
                reply.status, reply.body
            );
            let page = reply
                .body
                .get("spendingLimits")
                .and_then(Value::as_array)
                .cloned()
                .expect("spendingLimits is a required output member");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} limits, got {}",
                page.len()
            );
            seen.extend(page.iter().map(|l| member(l, "spendingLimitArn").to_string()));
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: the traversal must end, but it has made {pages} pages"
            );
            match reply.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let collected: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            collected.len(),
            seen.len(),
            "case {case:?}: a traversal must not report a limit twice, got {seen:?}"
        );
        assert_eq!(
            collected, on_first,
            "case {case:?}: a traversal must report every match exactly once"
        );
        assert!(
            !collected.contains(&on_second),
            "case {case:?}: the other device's limit must not appear"
        );
    }

    // A filter on a limit's OWN ARN narrows to that one limit: the other identifier of a
    // spending-limit summary, matched the same way.
    let one = search_limits(
        &braket,
        json!({
            "filters": [{
                "name": "spendingLimitArn",
                "operator": "EQUAL",
                "values": [on_second],
            }],
            "maxResults": 100,
        }),
    )
    .await;
    assert_eq!(
        one.status, 200,
        "a spendingLimitArn filter is served, got {} {}",
        one.status, one.body
    );
    let reported: Vec<String> = one
        .body
        .get("spendingLimits")
        .and_then(Value::as_array)
        .expect("spendingLimits is a required output member")
        .iter()
        .map(|limit| member(limit, "spendingLimitArn").to_string())
        .collect();
    assert_eq!(
        reported,
        vec![on_second.clone()],
        "a spendingLimitArn filter reports exactly that limit: {}",
        one.body
    );

    // (case, filter name) — a member this search does not narrow on.
    for (case, name) in [
        ("the amount", "spendingLimit"),
        ("the creation instant", "createdAt"),
        ("a member of no shape", "nonesuch"),
        ("a supported name with a trailing space", "deviceArn "),
        ("a supported name in another case", "SpendingLimitArn"),
    ] {
        let reply = search_limits(
            &braket,
            json!({
                "filters": [{ "name": name, "operator": "EQUAL", "values": ["anything"] }],
            }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: filtering on {name:?} is refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );
    }

    // A token is bound to the filters it was issued for, and an invented one is refused.
    let page = search_limits(
        &braket,
        json!({
            "filters": [{
                "name": "deviceArn",
                "operator": "EQUAL",
                "values": [first_device],
            }],
            "maxResults": 1,
        }),
    )
    .await;
    let issued = page
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a page of 1 out of 5 carries a nextToken")
        .to_string();
    for (case, body) in [
        (
            "a token replayed against a different filter set",
            json!({
                "filters": [{
                    "name": "deviceArn",
                    "operator": "EQUAL",
                    "values": [second_device],
                }],
                "nextToken": issued,
            }),
        ),
        (
            "a token this service never issued",
            json!({ "nextToken": "not-a-token" }),
        ),
    ] {
        let reply = search_limits(&braket, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );
    }
}

/// A read never changes what a later caller sees: searching twice reports the same
/// limits, and a search does not create, delete or alter one.
#[tokio::test]
async fn searching_twice_reports_the_same_limits() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();
    let mut body = limit_body("tok-stable", &device, "42.42");
    body["timePeriod"] = period(start, start + 600);
    let created = create_limit(&braket, body).await;
    assert_eq!(
        created.status, CREATED,
        "the create: expected {CREATED}, got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("spendingLimitArn");

    let first = limit_by_arn(&braket, &device, &arn).await;
    let second = limit_by_arn(&braket, &device, &arn).await;
    assert_eq!(
        first, second,
        "a limit read twice with nothing in between is the same limit"
    );
    assert_eq!(
        limits_on(&braket, &device).await.len(),
        1,
        "the reads created nothing"
    );
    // And the ARN the create minted is the one the reader keys on: percent-encoded into
    // a URI label it names the same resource for the operations that take it there.
    assert_eq!(
        encode(&arn).matches("%2F").count(),
        1,
        "a spending-limit ARN carries exactly one / (the resource id separator): {arn}"
    );
}
