// SPDX-License-Identifier: CC0-1.0

//! The two things the service SELECTS for every channel — its maintenance
//! window and the public address each of its pipelines egresses from — end to
//! end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: a channel created with a maintenance block, and a
//! two-pipeline channel, are ONE row of each.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::wire::{call, medialive};

/// Every value `MaintenanceDay` declares, in the model's own order
/// (`smithy_query(query="shape", shape="MaintenanceDay")`: MONDAY | TUESDAY |
/// WEDNESDAY | THURSDAY | FRIDAY | SATURDAY | SUNDAY), which is the set a
/// service-selected maintenance day must come from.
const MAINTENANCE_DAYS: [&str; 7] = [
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
];

async fn create(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "CreateChannel: expected 201, got {status}: {response}"
    );
    response["channel"].clone()
}

/// The `ChannelSummary` `ListChannels` reports for `id`.
async fn summary(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, listed) = call(router, "GET", "/prod/channels", None).await;
    assert_eq!(
        status, 200,
        "ListChannels: expected 200, got {status}: {listed}"
    );
    listed["channels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|channel| channel["id"] == json!(id))
        .unwrap_or(Value::Null)
}

/// The `maintenance` member of one channel-shaped body, as a client reads it.
fn maintenance(body: &Value) -> Value {
    body["maintenance"].clone()
}

/// The `sourceIp` of every `egressEndpoints` entry of one channel-shaped body,
/// in the order the body reports them.
fn source_ips(body: &Value) -> Vec<String> {
    body["egressEndpoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|endpoint| {
            endpoint["sourceIp"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A complete, well-formed maintenance window: a day the enum declares and a
/// whole hour of the day ("Choose the hour that maintenance will start"), with
/// no deadline or scheduled date, since no maintenance has been scheduled for
/// or required of the channel.
fn assert_window_is_well_formed(case: &str, read: &str, window: &Value) {
    let day = window["maintenanceDay"].as_str().unwrap_or_default();
    assert!(
        MAINTENANCE_DAYS.contains(&day),
        "case {case:?}: {read}: expected a maintenanceDay from {MAINTENANCE_DAYS:?}, got {:?}",
        window["maintenanceDay"]
    );
    let start = window["maintenanceStartTime"].as_str().unwrap_or_default();
    let hour = start.strip_suffix(":00").unwrap_or_default();
    assert!(
        hour.len() == 2
            && hour.chars().all(|digit| digit.is_ascii_digit())
            && hour.parse::<u32>().is_ok_and(|hour| hour < 24),
        "case {case:?}: {read}: expected a maintenanceStartTime naming a whole hour (HH:00), got {:?}",
        window["maintenanceStartTime"]
    );
    assert!(
        window["maintenanceDeadline"].is_null() && window["maintenanceScheduledDate"].is_null(),
        "case {case:?}: {read}: expected no maintenance to be scheduled or required yet, got {window:?}"
    );
}

/// One row of the maintenance-window table: the case, the `maintenance` the
/// create request names, and the day and the start time that request chose
/// (`None` where it left the choice to the service).
type WindowRow = (&'static str, Option<Value>, Option<&'static str>, Option<&'static str>);

/// Rule: the service selects a maintenance window for EVERY channel, so every
/// channel-shaped response reports a `maintenance` status carrying the currently
/// selected day and start time — the request's `MaintenanceCreateSettings` only
/// chooses them member by member ("Choose one day of the week for maintenance",
/// "Choose the hour that maintenance will start"), and the window is the same on
/// every later read.
///
/// The rows differ along which members the create request chooses: neither (the
/// case the issue reports), the day alone, the start time alone, both, and an
/// empty settings object. The chosen days and hours are entries of the day enum
/// and of the hours of the day that the issue did not cite, including both
/// published bounds of the hour (`00:00` and `23:00`).
#[tokio::test]
async fn every_channel_reports_the_maintenance_window_the_service_selected() {
    // (case, the `maintenance` the request names, the day it chose, the start
    //  time it chose)
    let table: [WindowRow; 6] = [
        ("no maintenance member", None, None, None),
        ("an empty maintenance object", Some(json!({})), None, None),
        (
            "the day alone",
            Some(json!({ "maintenanceDay": "SUNDAY" })),
            Some("SUNDAY"),
            None,
        ),
        (
            "the start time alone, at the first hour of the day",
            Some(json!({ "maintenanceStartTime": "00:00" })),
            None,
            Some("00:00"),
        ),
        (
            "both, at the last hour of the day",
            Some(json!({ "maintenanceDay": "SATURDAY", "maintenanceStartTime": "23:00" })),
            Some("SATURDAY"),
            Some("23:00"),
        ),
        (
            "both, mid-week and mid-afternoon",
            Some(json!({ "maintenanceDay": "WEDNESDAY", "maintenanceStartTime": "15:00" })),
            Some("WEDNESDAY"),
            Some("15:00"),
        ),
    ];

    for (case, requested, chosen_day, chosen_start) in table {
        let router = medialive();
        let mut body = json!({ "name": format!("channel-{case}") });
        if let Some(requested) = requested {
            body["maintenance"] = requested;
        }

        let created = create(&router, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let window = maintenance(&created);
        assert_window_is_well_formed(case, "CreateChannel", &window);

        // What the request chose is what the channel reports; what it left to
        // the service is still selected.
        if let Some(day) = chosen_day {
            assert_eq!(
                window["maintenanceDay"],
                json!(day),
                "case {case:?}: expected the chosen maintenanceDay {day}, got {:?}",
                window["maintenanceDay"]
            );
        }
        if let Some(start) = chosen_start {
            assert_eq!(
                window["maintenanceStartTime"],
                json!(start),
                "case {case:?}: expected the chosen maintenanceStartTime {start}, got {:?}",
                window["maintenanceStartTime"]
            );
        }

        // The selected window is the channel's, not one answer's: it reads back
        // the same through the describe and through the list summary.
        wait_for_the_clock();
        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel: got {status}: {described}"
        );
        assert_eq!(
            maintenance(&described),
            window,
            "case {case:?}: expected DescribeChannel to report the selected window {window:?}, got {:?}",
            maintenance(&described)
        );
        let listed = summary(&router, &id).await;
        assert_eq!(
            maintenance(&listed),
            window,
            "case {case:?}: expected the ListChannels summary to report the selected window {window:?}, got {:?}",
            maintenance(&listed)
        );
    }
}

/// Rule: the selected window is movable — `UpdateChannel`'s
/// `MaintenanceUpdateSettings` names the members it changes, and the ones it
/// does not name keep the values the channel already had, so the channel always
/// has a complete window.
///
/// The rows differ along which members the update names, over a channel whose
/// window the service selected on its own (the case the issue reports).
#[tokio::test]
async fn an_update_moves_the_selected_window_member_by_member() {
    // (case, the `maintenance` patch the update names)
    let table: [(&str, Value); 4] = [
        ("the day alone", json!({ "maintenanceDay": "FRIDAY" })),
        ("the start time alone", json!({ "maintenanceStartTime": "04:00" })),
        (
            "both",
            json!({ "maintenanceDay": "TUESDAY", "maintenanceStartTime": "21:00" }),
        ),
        ("neither", json!({})),
    ];

    for (case, patch) in table {
        let router = medialive();
        let created = create(&router, json!({ "name": format!("channel-{case}") })).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let selected = maintenance(&created);
        assert_window_is_well_formed(case, "CreateChannel", &selected);

        wait_for_the_clock();
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/prod/channels/{id}"),
            Some(json!({ "maintenance": patch.clone() })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannel: expected 200, got {status}: {updated}"
        );

        // Each member is the patched value where the patch names it and the
        // service's selection where it does not.
        let mut expected = selected.clone();
        for member in ["maintenanceDay", "maintenanceStartTime"] {
            if let Some(value) = patch.get(member) {
                expected[member] = value.clone();
            }
        }
        let window = maintenance(&updated["channel"]);
        assert_window_is_well_formed(case, "UpdateChannel", &window);
        assert_eq!(
            window, expected,
            "case {case:?}: expected the moved window {expected:?}, got {window:?}"
        );

        wait_for_the_clock();
        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel after the update: got {status}: {described}"
        );
        assert_eq!(
            maintenance(&described),
            expected,
            "case {case:?}: expected the moved window to read back, got {:?}",
            maintenance(&described)
        );
    }
}

/// Rule: a channel egresses from one public address per pipeline of its class,
/// so every channel-shaped response reports one `ChannelEgressEndpoint` per
/// pipeline (`SourceIp`: "Public IP of where a channel's output comes from"),
/// whatever state the channel is in — the address is provisioned with the
/// channel, so it is the same through its whole lifetime, from `CREATING` to the
/// terminal `DELETED`.
///
/// The rows span the `ChannelClass` enum (`smithy_query(query="shape",
/// shape="ChannelClass")`: STANDARD | SINGLE_PIPELINE) plus the absent-class
/// row, which the service defaults to the two-pipeline STANDARD class, so the
/// endpoint count varies along exactly what the pipeline count varies along.
#[tokio::test]
async fn a_channel_reports_one_egress_endpoint_per_pipeline_in_every_state() {
    // (case, the requested ChannelClass, the endpoints the channel reports)
    let table: [(&str, Option<&str>, usize); 3] = [
        ("single pipeline", Some("SINGLE_PIPELINE"), 1),
        ("standard", Some("STANDARD"), 2),
        ("class omitted", None, 2),
    ];

    for (case, requested, endpoints) in table {
        let router = medialive();
        let mut body = json!({ "name": format!("channel-{case}") });
        if let Some(class) = requested {
            body["channelClass"] = json!(class);
        }

        let created = create(&router, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let ips = source_ips(&created);
        assert_eq!(
            ips.len(),
            endpoints,
            "case {case:?}: CreateChannel: expected {endpoints} egress endpoint(s), got {:?}",
            created["egressEndpoints"]
        );
        for ip in &ips {
            assert!(
                ip.parse::<Ipv4Addr>().is_ok(),
                "case {case:?}: expected every sourceIp to be an IPv4 address, got {ip:?}"
            );
        }
        assert_eq!(
            ips.iter().collect::<BTreeSet<_>>().len(),
            ips.len(),
            "case {case:?}: expected each pipeline its own egress address, got {ips:?}"
        );

        // The same endpoints through every state the channel passes through,
        // and through every channel-shaped response: the describe, the list
        // summary, the start, the stop and the delete.
        wait_for_the_clock();
        let (status, idle) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {idle}");
        let reads: [(&str, Vec<String>); 2] = [
            ("DescribeChannel while IDLE", source_ips(&idle)),
            ("the ListChannels summary", source_ips(&summary(&router, &id).await)),
        ];
        for (read, reported) in reads {
            assert_eq!(
                reported, ips,
                "case {case:?}: expected {read} to report the channel's egress endpoints {ips:?}, got {reported:?}"
            );
        }

        for (read, method, path) in [
            ("StartChannel", "POST", format!("/prod/channels/{id}/start")),
            ("StopChannel", "POST", format!("/prod/channels/{id}/stop")),
            ("DeleteChannel", "DELETE", format!("/prod/channels/{id}")),
        ] {
            wait_for_the_clock();
            let (status, body) = call(&router, method, &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {read}: expected 200, got {status}: {body}"
            );
            let reported = source_ips(&body);
            assert_eq!(
                reported, ips,
                "case {case:?}: expected {read} to report the channel's egress endpoints {ips:?}, got {reported:?}"
            );
        }

        wait_for_the_clock();
        let (status, deleted) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel after the delete: got {status}: {deleted}"
        );
        assert_eq!(
            source_ips(&deleted), ips,
            "case {case:?}: expected a DELETED channel to still report its egress endpoints {ips:?}, got {:?}",
            source_ips(&deleted)
        );
    }
}

/// Rule: an egress address belongs to one pipeline of one channel, so no two
/// pipelines in an account — in the same channel or in another — report the same
/// `SourceIp`.
///
/// The rows differ along the classes the account's channels are created with, so
/// the pipeline count per channel varies while the uniqueness rule does not.
#[tokio::test]
async fn no_two_pipelines_in_an_account_share_an_egress_address() {
    let table: [(&str, &[&str]); 3] = [
        ("every channel standard", &["STANDARD", "STANDARD", "STANDARD"]),
        (
            "every channel single pipeline",
            &["SINGLE_PIPELINE", "SINGLE_PIPELINE"],
        ),
        (
            "classes mixed",
            &["SINGLE_PIPELINE", "STANDARD", "SINGLE_PIPELINE", "STANDARD"],
        ),
    ];

    for (case, classes) in table {
        let router = medialive();
        let mut all = Vec::new();
        for (index, class) in classes.iter().enumerate() {
            let created = create(
                &router,
                json!({ "name": format!("channel-{index}"), "channelClass": class }),
            )
            .await;
            all.extend(source_ips(&created));
        }
        assert_eq!(
            all.iter().collect::<BTreeSet<_>>().len(),
            all.len(),
            "case {case:?}: expected every pipeline in the account its own egress address, got {all:?}"
        );
    }
}
