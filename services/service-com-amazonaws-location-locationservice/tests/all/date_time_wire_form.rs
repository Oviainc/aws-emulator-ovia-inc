// SPDX-License-Identifier: CC0-1.0

//! The `date-time` wire form of every timestamp this service reads and writes.
//!
//! RULE: a timestamp member whose model declares `@timestampFormat("date-time")` is
//! carried on the JSON wire as an ISO 8601 / RFC 3339 STRING in both directions — a
//! request may send one and a response reports one — never as the epoch-seconds number
//! the `restJson1` protocol defaults to. This model declares one timestamp shape,
//! `com.amazonaws.location#Timestamp`, and that shape carries the trait
//! (`smithy_query(query="shape",shape="Timestamp")`), so the rule holds for EVERY
//! timestamp member of EVERY shape of this service.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: an instant that survives from the
//! write call to the read call survived because it was PERSISTED.
//!
//! The tables span what the rule generalises over: the form of the instant (whole
//! second, sub-second digits, the epoch itself, a leap day, a far-future year), the
//! position of the member in the response document (a top-level member, a member of a
//! structure inside a list, a member of a list entry), and the operations that carry one
//! (the device-position plane and the tracker and route-calculator control planes).

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, emulator, iso_millis};

/// The instants the tables are driven with, each written in the ISO 8601 form the model
/// documents (`YYYY-MM-DDThh:mm:ss.sssZ`). Rows differ along the FORM of the instant,
/// which is what the wire rule generalises over.
const INSTANTS: [(&str, &str); 5] = [
    ("whole second", "2024-01-01T00:00:00Z"),
    ("sub-second digits", "2024-03-01T12:34:56.789Z"),
    ("the epoch itself", "1970-01-01T00:00:00Z"),
    ("a leap day", "2024-02-29T06:00:00Z"),
    ("a far-future year", "2099-12-31T23:59:59Z"),
];

/// The epoch milliseconds of `member` in `value`, asserting first that it is carried as
/// an ISO 8601 date-time STRING and not as a number.
fn date_time_millis(case: &str, value: &Value, member: &str) -> i64 {
    let found = value
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {value}"));
    let text = found.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected {member} as an ISO 8601 string, got {found}")
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case:?}: expected {member} {text:?} to parse as ISO 8601, got {e}")
    });
    parsed.to_millis().unwrap_or_else(|e| {
        panic!("case {case:?}: expected {member} {text:?} to fit epoch millis, got {e}")
    })
}

async fn create_tracker(router: &MultiServiceRouter, name: &str) -> Value {
    let answer = call(
        router,
        "POST",
        "/tracking/v0/trackers",
        Some(json!({ "TrackerName": name })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "CreateTracker {name:?} must succeed, got {} {:?}",
        answer.status, answer.body
    );
    answer.json()
}

/// RULE: a `DevicePositionUpdate` whose `SampleTime` is an ISO 8601 string is ACCEPTED —
/// 200 with `Errors` present and empty — and the latest-position read reports that same
/// instant, plus a service-assigned `ReceivedTime`, as ISO 8601 strings.
#[tokio::test]
async fn an_iso_8601_sample_time_is_accepted_and_read_back_as_iso_8601() {
    let router = emulator();
    create_tracker(&router, "fleet").await;

    for (case, sample_time) in INSTANTS {
        let device = format!("device-{}", case.replace(' ', "-"));
        let updated = call(
            &router,
            "POST",
            "/tracking/v0/trackers/fleet/positions",
            Some(json!({
                "Updates": [{
                    "DeviceId": device,
                    "SampleTime": sample_time,
                    "Position": [-122.4, 37.8],
                }],
            })),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected BatchUpdateDevicePosition to accept SampleTime \
             {sample_time:?} with 200, got {} {:?}",
            updated.status, updated.body
        );
        let body = updated.json();
        assert_eq!(
            body.get("Errors").and_then(Value::as_array),
            Some(&vec![]),
            "case {case:?}: expected an empty Errors list, got {body}"
        );

        let got = call(
            &router,
            "GET",
            &format!("/tracking/v0/trackers/fleet/devices/{device}/positions/latest"),
            None,
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected GetDevicePosition to serve the position just \
             reported with 200, got {} {:?}",
            got.status, got.body
        );
        let got = got.json();
        assert_eq!(
            got.get("DeviceId").and_then(Value::as_str),
            Some(device.as_str()),
            "case {case:?}: expected DeviceId {device:?}, got {got}"
        );
        assert_eq!(
            got.get("Position").and_then(Value::as_array),
            Some(&vec![json!(-122.4), json!(37.8)]),
            "case {case:?}: expected the [longitude, latitude] that was sent, got {got}"
        );
        assert_eq!(
            date_time_millis(case, &got, "SampleTime"),
            iso_millis(sample_time),
            "case {case:?}: expected SampleTime {sample_time:?}, got {got}"
        );
        // ReceivedTime is the service's own clock reading, so only its wire FORM is
        // asserted here: `date_time_millis` fails unless it is an ISO 8601 string.
        date_time_millis(case, &got, "ReceivedTime");
    }
}

/// RULE: the rule reaches a timestamp wherever it sits in the document, including a
/// member of a structure inside a list — `BatchUpdateDevicePosition`'s per-entry
/// `Errors[].SampleTime`, which echoes the entry's own instant.
#[tokio::test]
async fn a_per_entry_error_carries_its_sample_time_as_iso_8601() {
    let router = emulator();
    create_tracker(&router, "fleet").await;

    for (case, sample_time) in INSTANTS {
        let updated = call(
            &router,
            "POST",
            "/tracking/v0/trackers/fleet/positions",
            Some(json!({
                "Updates": [{
                    "DeviceId": "out-of-domain",
                    "SampleTime": sample_time,
                    // Longitude outside the WGS 84 domain, so this entry is reported
                    // inside `Errors` rather than refusing the request.
                    "Position": [-200.0, 37.8],
                }],
            })),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected a per-entry failure to answer 200, got {} {:?}",
            updated.status, updated.body
        );
        let body = updated.json();
        let errors = body
            .get("Errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Errors list, got {body}"));
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected exactly one entry in Errors, got {body}"
        );
        assert_eq!(
            date_time_millis(case, &errors[0], "SampleTime"),
            iso_millis(sample_time),
            "case {case:?}: expected the entry's SampleTime {sample_time:?}, got {body}"
        );
    }
}

/// RULE: the same wire form holds for the control planes' resource timestamps, read
/// through every operation that reports one — a create response, a describe response and
/// a list entry.
#[tokio::test]
async fn a_resource_reports_its_timestamps_as_iso_8601() {
    let router = emulator();

    // One row per control-plane resource whose shapes carry a timestamp, with the
    // operations that report it: create, describe, and the list whose entries repeat it.
    struct ResourceCase {
        case: &'static str,
        create_path: &'static str,
        create_body: fn() -> Value,
        describe_path: &'static str,
        /// The list operation whose entries repeat the stored timestamps, when this
        /// crate implements one; `None` where it is still a scaffold stub with its own
        /// issue (`ListRouteCalculators`).
        list_path: Option<&'static str>,
    }

    let cases = [
        ResourceCase {
            case: "tracker",
            create_path: "/tracking/v0/trackers",
            create_body: || json!({ "TrackerName": "fleet" }),
            describe_path: "/tracking/v0/trackers/fleet",
            list_path: Some("/tracking/v0/list-trackers"),
        },
        ResourceCase {
            case: "route calculator",
            create_path: "/routes/v0/calculators",
            create_body: || json!({ "CalculatorName": "router", "DataSource": "Esri" }),
            describe_path: "/routes/v0/calculators/router",
            list_path: None,
        },
    ];

    for row in cases {
        let ResourceCase {
            case,
            create_path,
            create_body,
            describe_path,
            list_path,
        } = row;

        let created = call(&router, "POST", create_path, Some(create_body())).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create call must succeed, got {} {:?}",
            created.status, created.body
        );
        let created = created.json();
        let create_time = date_time_millis(case, &created, "CreateTime");

        let described = call(&router, "GET", describe_path, None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the describe call must succeed, got {} {:?}",
            described.status, described.body
        );
        let described = described.json();
        assert_eq!(
            date_time_millis(case, &described, "CreateTime"),
            create_time,
            "case {case:?}: expected the CreateTime the create call reported, got {described}"
        );
        assert_eq!(
            date_time_millis(case, &described, "UpdateTime"),
            create_time,
            "case {case:?}: a never-updated resource's UpdateTime must equal its \
             CreateTime, got {described}"
        );

        let Some(list_path) = list_path else {
            continue;
        };
        let listed = call(&router, "POST", list_path, Some(json!({}))).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the list call must succeed, got {} {:?}",
            listed.status, listed.body
        );
        let listed = listed.json();
        let entries = listed
            .get("Entries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Entries list, got {listed}"));
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected the one created resource in Entries, got {listed}"
        );
        for entry in entries {
            assert_eq!(
                date_time_millis(case, entry, "CreateTime"),
                create_time,
                "case {case:?}: expected the list entry to repeat the stored CreateTime, \
                 got {entry}"
            );
            date_time_millis(case, entry, "UpdateTime");
        }
    }
}
