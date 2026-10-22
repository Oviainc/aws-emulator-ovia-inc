// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the event-source integration DevOps Guru consumes from
//! Amazon CodeGuru Profiler: `UpdateEventSourcesConfig` writes it and
//! `DescribeEventSourcesConfig` reports it.
//!
//! Every test drives the service the way a client does — a buffered HTTP request
//! through the same router the emulator boots — using the harness the sibling
//! `configuration_lifecycle` suite already defines, so the assertions are about wire
//! behaviour rather than handler internals.

use serde_json::{json, Value};

use crate::configuration_lifecycle::{Emulator, Reply};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::EVENT_SOURCE_OPT_IN_STATUS_VALUES;

/// `UpdateEventSourcesConfig`: `PUT /event-sources`, from
/// `smithy_query(query="operation",operation="UpdateEventSourcesConfig")`.
async fn update_event_sources(emulator: &Emulator, request: &Value) -> Reply {
    emulator.call("PUT", "/event-sources", Some(request)).await
}

/// `DescribeEventSourcesConfig`: `POST /event-sources` with no modeled input members,
/// from `smithy_query(query="operation",operation="DescribeEventSourcesConfig")`.
async fn describe_event_sources(emulator: &Emulator) -> Reply {
    emulator
        .call("POST", "/event-sources", Some(&json!({})))
        .await
}

/// The `EventSources.AmazonCodeGuruProfiler.Status` a read reported, or `None` when the
/// reply omitted any step of that path.
fn reported_status(reply: &Reply) -> Option<String> {
    reply
        .at("/EventSources/AmazonCodeGuruProfiler/Status")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The status a read reports, asserted to be present and the read to have succeeded.
async fn status_of(emulator: &Emulator, case: &str) -> String {
    let reply = describe_event_sources(emulator).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DescribeEventSourcesConfig must succeed, got status {} body {}",
        reply.status, reply.raw
    );
    reported_status(&reply).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the read must report EventSources.AmazonCodeGuruProfiler.Status, got {}",
            reply.raw
        )
    })
}

/// An update request carrying `event_sources` as the whole `EventSources` member.
fn request(event_sources: Value) -> Value {
    json!({ "EventSources": event_sources })
}

/// RULE: the event-source configuration is one persisted value per account — an update
/// stores the `EventSourceOptInStatus` it names for an integration, an update that names
/// none leaves the stored one as it was, and a describe reports the last stored value
/// (the opted-out default before any update) without changing it.
///
/// The rows are applied in order to one account and each names the WHOLE expected
/// configuration. They differ along what the rule generalises over: which status value
/// is submitted (every value the enum declares, in both orders), how often it is
/// repeated, and how much of the optional `EventSources` tree the request carries.
#[tokio::test]
async fn event_sources_config_reports_the_last_stored_status() {
    let emulator = Emulator::new();

    // The catalog of statuses, from `EVENT_SOURCE_OPT_IN_STATUS_VALUES` in
    // `src/ops/types/shared_types.rs`, i.e. the `EventSourceOptInStatus` enum
    // (`smithy_query(query="shape",shape="EventSourceOptInStatus")`): DISABLED, ENABLED.
    let catalog: Vec<&str> = EVENT_SOURCE_OPT_IN_STATUS_VALUES.to_vec();
    assert!(
        catalog.contains(&"ENABLED") && catalog.contains(&"DISABLED"),
        "case \"catalog\": EventSourceOptInStatus must declare ENABLED and DISABLED, got {catalog:?}"
    );

    // (case, the request to send first — None to only read, expected reported status)
    let mut steps: Vec<(String, Option<Value>, String)> = vec![(
        "before any update".to_string(),
        None,
        "DISABLED".to_string(),
    )];

    // Every declared value, set in catalog order and then in reverse order, so each one
    // is reached both from the default and from the other value.
    for value in catalog.iter().chain(catalog.iter().rev()) {
        steps.push((
            format!("status set to {value}"),
            Some(request(
                json!({ "AmazonCodeGuruProfiler": { "Status": value } }),
            )),
            (*value).to_string(),
        ));
        // Repeating the same update is idempotent.
        steps.push((
            format!("status set to {value} again"),
            Some(request(
                json!({ "AmazonCodeGuruProfiler": { "Status": value } }),
            )),
            (*value).to_string(),
        ));
    }

    // Requests that name no status leave the last stored one STANDING. The standing
    // value is deliberately one that differs from the `DISABLED` default, so "an absent
    // member leaves the stored value as it was" is distinguishable from "the handler
    // answered the default".
    let standing = catalog
        .iter()
        .find(|value| **value != "DISABLED")
        .expect("EventSourceOptInStatus must declare a value other than DISABLED")
        .to_string();
    steps.push((
        format!("status set to {standing}, the value the next rows must leave standing"),
        Some(request(
            json!({ "AmazonCodeGuruProfiler": { "Status": standing } }),
        )),
        standing.clone(),
    ));
    for (case, event_sources) in [
        ("an update naming no integration", Some(json!({}))),
        (
            "an update naming the integration but no status",
            Some(json!({ "AmazonCodeGuruProfiler": {} })),
        ),
        ("an update carrying no EventSources member", None),
    ] {
        let body = match event_sources {
            Some(event_sources) => request(event_sources),
            None => json!({}),
        };
        steps.push((case.to_string(), Some(body), standing.clone()));
    }

    for (case, update, expected) in steps {
        if let Some(update) = update {
            let reply = update_event_sources(&emulator, &update).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: UpdateEventSourcesConfig must succeed, got status {} body {}",
                reply.status, reply.raw
            );
        }
        let actual = status_of(&emulator, &case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored status {expected:?}, got {actual:?}"
        );
        // A read reports; it must not change what the next caller sees.
        let again = status_of(&emulator, &case).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second read must still report {expected:?}, got {again:?}"
        );
    }
}

/// RULE: `Status` targets the `EventSourceOptInStatus` enum, so a value that enum does
/// not declare is refused with HTTP 400 `ValidationException` — by the boundary
/// validator, before the handler runs — and the stored configuration is left exactly as
/// it was.
///
/// The rows differ in HOW the submitted value misses the enum: case, padding, truncation,
/// emptiness, and a plausible-but-undeclared status.
#[tokio::test]
async fn event_sources_status_outside_the_enum_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    // Put a known value in the store first, so a refusal can be shown to change nothing.
    let seeded = "ENABLED";
    let reply = update_event_sources(
        &emulator,
        &request(json!({ "AmazonCodeGuruProfiler": { "Status": seeded } })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"seed\": the seeding update must succeed, got status {} body {}",
        reply.status, reply.raw
    );

    for case in [
        "enabled",
        "Enabled",
        " ENABLED",
        "ENABLE",
        "",
        "PENDING",
        "ENABLED,DISABLED",
    ] {
        assert!(
            !EVENT_SOURCE_OPT_IN_STATUS_VALUES.contains(&case),
            "case {case:?}: this row must name a value the enum does NOT declare"
        );
        let reply = update_event_sources(
            &emulator,
            &request(json!({ "AmazonCodeGuruProfiler": { "Status": case } })),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400 refusal, got status {} body {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: expected a ValidationException, got {}",
            reply.raw
        );
        let stored = status_of(&emulator, case).await;
        assert_eq!(
            stored, seeded,
            "case {case:?}: the refused update must leave the stored status {seeded:?}, got {stored:?}"
        );
    }
}

/// RULE: the configuration is per engine — a second emulator, built over its own
/// engine, reports the opted-out default whatever another one was told.
#[tokio::test]
async fn event_sources_config_is_per_engine_state_not_a_constant() {
    let configured = Emulator::new();
    let reply = update_event_sources(
        &configured,
        &request(json!({ "AmazonCodeGuruProfiler": { "Status": "ENABLED" } })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"configured\": the update must succeed, got status {} body {}",
        reply.status, reply.raw
    );

    let configured_status = status_of(&configured, "configured").await;
    assert_eq!(
        configured_status, "ENABLED",
        "case \"configured\": expected ENABLED, got {configured_status:?}"
    );

    let fresh = Emulator::new();
    let fresh_status = status_of(&fresh, "fresh engine").await;
    assert_eq!(
        fresh_status, "DISABLED",
        "case \"fresh engine\": expected the opted-out default DISABLED, got {fresh_status:?}"
    );
}
