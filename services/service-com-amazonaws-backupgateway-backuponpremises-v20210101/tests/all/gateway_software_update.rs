// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateGatewaySoftwareNow`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: `UpdateGatewaySoftwareNow` TRIGGERS the appliance software update
//! of the gateway its `GatewayArn` resolves to — it answers 200 at once with that record's
//! own ARN, persists the update on the record, and the version a read of the gateway
//! reports becomes the newest one when the install has fallen due, so no caller polls for
//! it and no read moves it along. Triggering it again is the same update, nothing else on
//! the record moves, and no other gateway's appliance is touched. A well-formed
//! `GatewayArn` the store holds no gateway under is refused with the modelled
//! `ResourceNotFoundException` (HTTP 400, `ErrorCode` `GatewayNotFound`) and a request
//! outside the modelled constraints
//! with `ValidationException` (HTTP 400), with nothing written either way.
//!
//! The tables below span that class: gateways differing in what else the record carries
//! (tags, a maintenance window, a bandwidth rate-limit schedule, an association with a
//! hypervisor, all of them at once), each point of the install's span, trigger counts of
//! one, two and three, and ARNs differing in partition, resource type, id and length,
//! including each published bound of `GatewayArn` and one past it.
//!
//! Where a case needs the install to have fallen due, the stored instant it falls due at
//! is moved BACK instead of sleeping ([`rewind_install`]): the version a read reports is
//! derived from that one instant and the clock, so an install that was due earlier and one
//! observed later are the same observation. One case waits for real
//! ([`a_real_wait_alone_carries_the_install_to_the_newest_version`]) to prove that no
//! further call is what carries it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::ops::types::BandwidthRateLimitInterval;
use backup_gateway::state::{
    service_state, GATEWAY_SOFTWARE_UPDATE_INSTALLS_AFTER, GATEWAY_SOFTWARE_VERSION,
    GATEWAY_UPDATED_SOFTWARE_VERSION,
};

use crate::bandwidth_rate_limit_schedule::{arn_in, arn_of_length, interval};
use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, create_body, gateway_count, issued_activation_key,
    never_created_arn, Reply,
};

/// How far past the instant an install falls due a case looks, when it waits or rewinds
/// for real: enough that the install is unambiguously due.
const PAST_THE_INSTALL: Duration =
    GATEWAY_SOFTWARE_UPDATE_INSTALLS_AFTER.saturating_add(Duration::from_millis(100));

async fn trigger(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(
        router,
        "UpdateGatewaySoftwareNow",
        &json!({ "GatewayArn": arn }),
    )
    .await
}

/// The `Gateway` object `GetGateway` reports for `arn`, with the status it answered.
async fn gateway_object(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let reply = call(router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    let gateway = reply.body.get("Gateway").cloned().unwrap_or(Value::Null);
    (reply.status, gateway)
}

/// The `SoftwareVersion` member `GetGateway` reports for `arn`.
async fn reported_version(router: &MultiServiceRouter, arn: &str) -> String {
    let (status, gateway) = gateway_object(router, arn).await;
    assert_eq!(
        status, 200,
        "reading {arn}: GetGateway must answer 200, got {status} {gateway:?}"
    );
    gateway
        .get("SoftwareVersion")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no SoftwareVersion in {gateway:?}"))
        .to_string()
}

/// The version the STORED record under `arn` carries, read out of the store rather than
/// off the wire: the version the appliance is running, before any install still to come.
fn stored_version(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    store
        .gateway(arn)
        .unwrap_or_else(|| panic!("no gateway stored under {arn}"))
        .software_version
        .clone()
}

/// The install in flight on the record under `arn`, as the store holds it: the version it
/// installs and the instant it falls due. `None` when no update is installing.
fn pending_install(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<(String, SystemTime)> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    store
        .gateway(arn)
        .unwrap_or_else(|| panic!("no gateway stored under {arn}"))
        .software_update
        .as_ref()
        .map(|update| (update.to_version.clone(), update.installed_at))
}

/// Move the stored instant the install on `arn` falls due at BACK by `by`, so a read
/// observes the point of the install's span that lies `by` after the trigger.
///
/// The version a read reports is derived from that one instant and the clock, so this is
/// the same observation as waiting `by`.
fn rewind_install(engine: &Arc<engine::InMemoryEngine>, arn: &str, by: Duration) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    let record = store
        .gateway_mut(arn)
        .unwrap_or_else(|| panic!("no gateway stored under {arn}"));
    let update = record
        .software_update
        .as_mut()
        .unwrap_or_else(|| panic!("no software update installing on {arn}"));
    update.installed_at -= by;
}

/// What else the gateway of one row carries before its update is triggered: an update must
/// behave the same whatever the rest of the record holds.
#[derive(Debug, Clone, Copy)]
struct Fixture {
    case: &'static str,
    tags: usize,
    window: bool,
    schedule: bool,
    associated: bool,
}

impl Fixture {
    const fn bare(case: &'static str) -> Self {
        Self {
            case,
            tags: 0,
            window: false,
            schedule: false,
            associated: false,
        }
    }
}

/// The rows of the fixture table: a gateway carrying nothing beyond its activation, one
/// carrying each other member this API can set, and one carrying all of them at once.
fn fixture_rows() -> Vec<Fixture> {
    vec![
        Fixture::bare("a gateway carrying nothing but its activation"),
        Fixture {
            tags: 3,
            ..Fixture::bare("a gateway carrying tags")
        },
        Fixture {
            window: true,
            ..Fixture::bare("a gateway carrying a maintenance window")
        },
        Fixture {
            schedule: true,
            ..Fixture::bare("a gateway carrying a bandwidth rate-limit schedule")
        },
        Fixture {
            associated: true,
            ..Fixture::bare("a gateway associated with a hypervisor")
        },
        Fixture {
            tags: 1,
            window: true,
            schedule: true,
            associated: true,
            case: "a gateway carrying every member this API can set",
        },
    ]
}

/// Create the gateway one row describes and hand back its ARN, having driven every setup
/// call over the wire so the record under test is one the API really wrote.
async fn set_up(router: &MultiServiceRouter, fixture: &Fixture, index: usize) -> String {
    let tags: BTreeMap<String, String> = (0..fixture.tags)
        .map(|tag| (format!("key-{tag}"), format!("value-{tag}")))
        .collect();
    let created = call(
        router,
        "CreateGateway",
        &create_body(
            &format!("gw-update-{index}"),
            &issued_activation_key(router).await,
            &tags,
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: CreateGateway must answer 200, got {} {:?}",
        fixture.case, created.status, created.body
    );
    let arn = created.gateway_arn().to_string();

    if fixture.window {
        let put = call(
            router,
            "PutMaintenanceStartTime",
            &json!({
                "GatewayArn": arn,
                "HourOfDay": 3,
                "MinuteOfHour": 30,
                "DayOfWeek": 2,
            }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: PutMaintenanceStartTime must answer 200, got {} {:?}",
            fixture.case, put.status, put.body
        );
    }

    if fixture.schedule {
        let intervals: Vec<BandwidthRateLimitInterval> =
            vec![interval(1, 0, 5, 30, &[1, 2, 3], Some(102_400))];
        let sent = Value::Array(
            intervals
                .iter()
                .map(|slot| {
                    json!({
                        "StartHourOfDay": slot.start_hour_of_day,
                        "StartMinuteOfHour": slot.start_minute_of_hour,
                        "EndHourOfDay": slot.end_hour_of_day,
                        "EndMinuteOfHour": slot.end_minute_of_hour,
                        "DaysOfWeek": slot.days_of_week,
                        "AverageUploadRateLimitInBitsPerSec":
                            slot.average_upload_rate_limit_in_bits_per_sec,
                    })
                })
                .collect(),
        );
        let put = call(
            router,
            "PutBandwidthRateLimitSchedule",
            &json!({ "GatewayArn": arn, "BandwidthRateLimitIntervals": sent }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: PutBandwidthRateLimitSchedule must answer 200, got {} {:?}",
            fixture.case, put.status, put.body
        );
    }

    if fixture.associated {
        let imported = call(
            router,
            "ImportHypervisorConfiguration",
            &json!({
                "Name": format!("hv-update-{index}"),
                "Host": format!("10.0.{index}.4"),
            }),
        )
        .await;
        assert_eq!(
            imported.status, 200,
            "case {:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
            fixture.case, imported.status, imported.body
        );
        let server_arn = imported
            .str_at("/HypervisorArn")
            .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", imported.body))
            .to_string();
        let associated = call(
            router,
            "AssociateGatewayToServer",
            &json!({ "GatewayArn": arn, "ServerArn": server_arn }),
        )
        .await;
        assert_eq!(
            associated.status, 200,
            "case {:?}: AssociateGatewayToServer must answer 200, got {} {:?}",
            fixture.case, associated.status, associated.body
        );
    }

    arn
}

/// `object` with its `SoftwareVersion` replaced by `version`: what a read must report once
/// an install has fallen due, if the install moved nothing else.
fn with_version(object: &Value, version: &str) -> Value {
    let mut expected = object.clone();
    expected["SoftwareVersion"] = json!(version);
    expected
}

/// Triggering the update persists it on the gateway the ARN resolves to, whatever else
/// that record carries: the reply is 200 with the record's OWN ARN, the install is on the
/// record with the newest version and a due instant in the future, a read before that
/// instant still reports the version the appliance was activated with, and once the
/// install is due every read reports the newest version with nothing else on the record
/// moved.
#[tokio::test]
async fn an_update_installs_the_newest_version_whatever_else_the_gateway_carries() {
    assert_ne!(
        GATEWAY_SOFTWARE_VERSION, GATEWAY_UPDATED_SOFTWARE_VERSION,
        "the version an update installs must differ from the one a gateway is activated \
         with, or an install is not observable"
    );
    assert!(
        GATEWAY_SOFTWARE_UPDATE_INSTALLS_AFTER > Duration::ZERO,
        "an install must take a non-zero span, so a caller reading straight after the \
         trigger still sees the version the appliance was running"
    );

    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway nobody triggers an update on, created first so it is in the store for
    // every row below: its appliance must stay on the version it was activated with.
    let untouched = create(&router, "gw-untouched").await;

    let rows = fixture_rows();
    let mut arns = Vec::new();
    for (index, fixture) in rows.iter().enumerate() {
        let case = fixture.case;
        let arn = set_up(&router, fixture, index).await;
        let (status, before) = gateway_object(&router, &arn).await;
        assert_eq!(
            (status, before.get("SoftwareVersion").and_then(Value::as_str)),
            (200, Some(GATEWAY_SOFTWARE_VERSION)),
            "case {case:?}: a gateway must start on the version it was activated with, \
             got {status} {before:?}"
        );

        let requested_at = SystemTime::now();
        let reply = trigger(&router, &arn).await;
        assert_eq!(
            (reply.status, reply.str_at("/GatewayArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: UpdateGatewaySoftwareNow must answer 200 with the record's own \
             ARN, got {} {:?}",
            reply.status,
            reply.body
        );
        let answered_at = SystemTime::now();

        let (installing, due) = pending_install(&engine, &arn)
            .unwrap_or_else(|| panic!("case {case:?}: no install was persisted on {arn}"));
        assert_eq!(
            installing, GATEWAY_UPDATED_SOFTWARE_VERSION,
            "case {case:?}: the install persisted must be the newest version, got \
             {installing:?}"
        );
        assert!(
            due > requested_at && due <= answered_at + GATEWAY_SOFTWARE_UPDATE_INSTALLS_AFTER,
            "case {case:?}: the install must fall due within the install span of the \
             trigger, got {due:?} for a trigger at {requested_at:?}"
        );
        assert_eq!(
            stored_version(&engine, &arn),
            GATEWAY_SOFTWARE_VERSION,
            "case {case:?}: the trigger must not install the update on the spot"
        );

        // Before the install falls due the appliance is still running the old version, and
        // nothing else about the gateway has moved.
        let (status, during) = gateway_object(&router, &arn).await;
        assert_eq!(
            (status, &during),
            (200, &before),
            "case {case:?}: a read before the install is due must report the gateway \
             exactly as it was, got {status} {during:?}"
        );

        // The same observation as waiting out the install span.
        rewind_install(&engine, &arn, PAST_THE_INSTALL);
        let (status, after) = gateway_object(&router, &arn).await;
        assert_eq!(
            (status, &after),
            (200, &with_version(&before, GATEWAY_UPDATED_SOFTWARE_VERSION)),
            "case {case:?}: once the install is due the gateway must report the newest \
             version and nothing else may have moved, got {status} {after:?}"
        );

        // Reading again reports the same thing: a read does not carry an install along.
        let (status, again) = gateway_object(&router, &arn).await;
        assert_eq!(
            (status, &again),
            (200, &after),
            "case {case:?}: reading twice must report the same gateway, got {status} \
             {again:?}"
        );
        arns.push(arn);
    }

    // Every record answers from its own appliance, and the gateway nobody updated is still
    // on the version it was activated with.
    for (fixture, arn) in rows.iter().zip(&arns) {
        assert_eq!(
            reported_version(&router, arn).await,
            GATEWAY_UPDATED_SOFTWARE_VERSION,
            "case {:?}: each updated gateway must report its own installed version",
            fixture.case
        );
    }
    assert_eq!(
        reported_version(&router, &untouched).await,
        GATEWAY_SOFTWARE_VERSION,
        "a gateway nobody triggered an update on must stay on the version it was \
         activated with"
    );
    assert_eq!(
        pending_install(&engine, &untouched),
        None,
        "a gateway nobody triggered an update on must carry no install"
    );
}

/// A real wait alone carries the install to the newest version: no second call, and no
/// read, is what installs it.
#[tokio::test]
async fn a_real_wait_alone_carries_the_install_to_the_newest_version() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-real-wait").await;

    let reply = trigger(&router, &arn).await;
    assert_eq!(
        reply.status, 200,
        "UpdateGatewaySoftwareNow must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reported_version(&router, &arn).await,
        GATEWAY_SOFTWARE_VERSION,
        "a read straight after the trigger must still report the version the appliance \
         was running"
    );

    // Blocking `std::thread::sleep` rather than `tokio::time::sleep`: the store's due
    // instant is a wall-clock one, so the wait has to be a wall-clock wait.
    std::thread::sleep(PAST_THE_INSTALL);

    assert_eq!(
        reported_version(&router, &arn).await,
        GATEWAY_UPDATED_SOFTWARE_VERSION,
        "the clock alone must carry the install to the newest version"
    );
}

/// Triggering the update again is ONE update: a second and a third trigger while the
/// install is in flight neither restart nor delay it, and a trigger once the appliance
/// runs the newest version leaves the record exactly as the call before it left it.
#[tokio::test]
async fn triggering_the_update_again_is_one_update() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-idempotent").await;

    let first = trigger(&router, &arn).await;
    assert_eq!(
        (first.status, first.str_at("/GatewayArn")),
        (200, Some(arn.as_str())),
        "the first trigger must answer 200 with the record's own ARN, got {} {:?}",
        first.status,
        first.body
    );
    let due = pending_install(&engine, &arn)
        .unwrap_or_else(|| panic!("no install was persisted on {arn}"))
        .1;

    // Trigger counts of two and three, all while the install is still in flight.
    for count in 2..=3 {
        let again = trigger(&router, &arn).await;
        assert_eq!(
            (again.status, again.str_at("/GatewayArn")),
            (200, Some(arn.as_str())),
            "case trigger {count}: a repeated trigger must answer 200 with the record's \
             own ARN, got {} {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            pending_install(&engine, &arn).map(|install| install.1),
            Some(due),
            "case trigger {count}: a repeated trigger must neither restart nor delay the \
             install already in flight"
        );
        assert_eq!(
            reported_version(&router, &arn).await,
            GATEWAY_SOFTWARE_VERSION,
            "case trigger {count}: the appliance is still installing, so a read must \
             report the version it is running"
        );
    }

    rewind_install(&engine, &arn, PAST_THE_INSTALL);
    assert_eq!(
        reported_version(&router, &arn).await,
        GATEWAY_UPDATED_SOFTWARE_VERSION,
        "once the install is due the gateway must report the newest version"
    );

    // A trigger on an appliance that already runs the newest version has nothing to
    // install: the reported gateway is untouched and no install is left in flight.
    let (_, before) = gateway_object(&router, &arn).await;
    let once_more = trigger(&router, &arn).await;
    assert_eq!(
        (once_more.status, once_more.str_at("/GatewayArn")),
        (200, Some(arn.as_str())),
        "a trigger on an up-to-date appliance must answer 200 with the record's own ARN, \
         got {} {:?}",
        once_more.status,
        once_more.body
    );
    let (status, after) = gateway_object(&router, &arn).await;
    assert_eq!(
        (status, &after),
        (200, &before),
        "a trigger on an up-to-date appliance must leave the reported gateway unchanged, \
         got {status} {after:?}"
    );
    assert_eq!(
        pending_install(&engine, &arn),
        None,
        "an appliance running the newest version must carry no install"
    );
    assert_eq!(
        stored_version(&engine, &arn),
        GATEWAY_UPDATED_SOFTWARE_VERSION,
        "the installed version must be the one the record carries once the install is done"
    );
    assert_eq!(
        gateway_count(&engine),
        1,
        "a trigger must not add or drop a record"
    );
}

/// A well-formed `GatewayArn` the store holds no gateway under is refused with the modelled
/// `ResourceNotFoundException` — HTTP 400 carrying `ErrorCode` `GatewayNotFound` and the
/// fixed sentence this service states for a missing gateway, the way every other gateway
/// operation of this service refuses one —
/// whatever the ARN's partition, resource type, id or length, and nothing is written for it.
#[tokio::test]
async fn an_arn_with_no_stored_gateway_is_refused_as_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway that really exists, with an install of its own in flight, so a refusal
    // below is about the ARN asked for and not about an empty store.
    let live = create(&router, "gw-live-update").await;
    let accepted = trigger(&router, &live).await;
    assert_eq!(
        accepted.status, 200,
        "the trigger on the live gateway must answer 200, got {} {:?}",
        accepted.status, accepted.body
    );
    let live_install = pending_install(&engine, &live);

    let rows: Vec<(&str, String)> = vec![
        ("a gateway ARN that was never created", never_created_arn()),
        (
            "a gateway ARN in the aws-cn partition",
            arn_in("aws-cn", "gateway", "bgw-fffffffffffff102"),
        ),
        (
            "a gateway ARN in the aws-us-gov partition",
            arn_in("aws-us-gov", "gateway", "bgw-fffffffffffff103"),
        ),
        (
            "an ARN naming another resource type of this service",
            arn_in("aws", "hypervisor", "hv-fffffffffffff104"),
        ),
        (
            "the 50-character minimum of the modelled GatewayArn",
            arn_of_length(50),
        ),
        (
            "the 180-character maximum of the modelled GatewayArn",
            arn_of_length(180),
        ),
    ];

    for (case, arn) in &rows {
        let reply = trigger(&router, arn).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply.str_at("/ErrorCode"),
                reply.str_at("/Message"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("GatewayNotFound"),
                Some("The specified gateway was not found."),
            ),
            "case {case:?}: an ARN with no stored gateway must be refused 400 \
             ResourceNotFoundException / GatewayNotFound, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            gateway_count(&engine),
            1,
            "case {case:?}: a refused request must not create a record"
        );
        assert_eq!(
            pending_install(&engine, &live),
            live_install,
            "case {case:?}: a refused request must not touch another gateway's install"
        );
    }

    // A record the store no longer holds is refused exactly like one it never held.
    let deleted = call(&router, "DeleteGateway", &json!({ "GatewayArn": live })).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteGateway must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let gone = trigger(&router, &live).await;
    assert_eq!(
        (gone.status, gone.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "a deleted gateway must be refused like one that never existed, got {} {:?}",
        gone.status,
        gone.body
    );
}

/// A request outside the modelled constraints never reaches the store: it is refused with
/// HTTP 400 `ValidationException`, one past each published bound of `GatewayArn` and for a
/// partition its pattern does not accept, and the gateway in the store keeps the version
/// and the install it had.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-constraints-update").await;
    let before = pending_install(&engine, &arn);

    // `GatewayArn` `@length(min: 50, max: 180)` with pattern
    // `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
    // (`smithy_query(query="operation",operation="UpdateGatewaySoftwareNow")`), plus the
    // required member missing.
    let rows: Vec<(&str, Value)> = vec![
        ("no GatewayArn at all", json!({})),
        (
            "one character under the 50-character minimum of GatewayArn",
            json!({ "GatewayArn": arn_of_length(49) }),
        ),
        (
            "one character over the 180-character maximum of GatewayArn",
            json!({ "GatewayArn": arn_of_length(181) }),
        ),
        (
            "a partition the GatewayArn pattern does not accept",
            json!({ "GatewayArn": arn_in("aws-iso", "gateway", "bgw-0000000000000103") }),
        ),
        (
            "an ARN with no resource id at all",
            json!({ "GatewayArn": format!("arn:aws:backup-gateway:us-west-2:{}:gateway/", engine::EMULATOR_ACCOUNT_ID) }),
        ),
    ];

    for (case, body) in &rows {
        let reply = call(&router, "UpdateGatewaySoftwareNow", body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a request outside the modelled constraints must be refused \
             with 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            pending_install(&engine, &arn),
            before,
            "case {case:?}: a refused request must leave the stored install as it was"
        );
        assert_eq!(
            reported_version(&router, &arn).await,
            GATEWAY_SOFTWARE_VERSION,
            "case {case:?}: a refused request must leave the reported version as it was"
        );
    }
}
