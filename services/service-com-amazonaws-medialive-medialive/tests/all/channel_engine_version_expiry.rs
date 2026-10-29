// SPDX-License-Identifier: CC0-1.0

//! The engine version a channel is pinned to, as every channel-shaped read
//! reports it, end to end over the restJson1 wire.
//!
//! The rule under test: a channel stores only the build identifier it was pinned
//! with, and `ChannelEngineVersionResponse$ExpirationDate` is "The UTC time when
//! the version expires" — one fact of the account's engine version catalogue — so
//! every reader that reports an engine version reports the expiry THAT catalogue
//! stores for that identifier, the same instant `ListVersions` reports for it, and
//! reports no expiry only for an identifier the catalogue does not hold.
//!
//! The rows differ along what the rule generalises over: which catalogue entry the
//! channel is pinned to (so a different stored expiry per row), whether that entry
//! is still ahead of its expiry, whether the catalogue holds the identifier at all,
//! and whether the channel is pinned at all — and each row is read back through
//! every projection a channel is reported by (the create's own reply and
//! `DescribeChannel`, which answer the full description, and `ListChannels`, which
//! answers the summary).
//!
//! Nothing here invents an engine version: the catalogue the table installs is
//! built through the store's own type, because no operation of this service models
//! a way to create one, and the agreement suite at the end installs nothing at all
//! and pins the versions the account's seeded catalogue publishes through
//! `ListVersions`.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    ChannelEngineVersionCatalog, ChannelEngineVersionRecord,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::engine_version_catalog::{install, shifted};
use crate::support::{epoch_seconds, member, timestamp};
use crate::wire::{call, medialive, medialive_with_engine};

/// How far two reported instants may be apart and still name the same instant: the
/// wire form carries sub-second precision, so the comparison is a tolerance rather
/// than an equality.
const TOLERANCE_SECONDS: f64 = 0.5;

/// The catalogue every row of the table installs: three entries released at
/// different instants with three DIFFERENT expiries, one of them already past, as
/// `(build identifier, seconds from the base instant it was released, seconds from
/// it that it expires)`.
///
/// One catalogue for the whole table, so a row that reports the wrong entry's
/// expiry cannot pass: the three expiries are far apart and only the pinned
/// entry's is the right answer.
const CATALOGUE: &[(&str, i64, i64)] = &[
    ("9.0.0", -300, 1_800),
    ("9.1.0", -200, -60),
    ("9.2.0", -100, 7_200),
];

/// One row: the build identifier the channel is pinned with, and the expiry every
/// read of that channel must then report.
struct Row {
    case: &'static str,
    /// The `ChannelEngineVersionRequest$Version` the channel is created with, or
    /// `None` for a channel created without a pin.
    pinned: Option<&'static str>,
    /// Seconds from the base instant the reported `expirationDate` must name, or
    /// `None` when there is no stored expiry to report.
    expires: Option<i64>,
}

/// `CreateChannel`, answering the channel it reports.
async fn create_channel(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = crate::support::resolved_channel_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateChannel to answer 201, got {status}: {response}"
    );
    response["channel"].clone()
}

/// `DescribeChannel`.
async fn describe_channel(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeChannel to answer 200, got {status}: {body}"
    );
    body
}

/// The `ListChannels` summary of the channel `id`.
async fn listed_channel(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", "/prod/channels", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListChannels to answer 200, got {status}: {body}"
    );
    body["channels"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|channel| channel["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListChannels reported no channel {id}: {body}"))
}

/// Assert that one channel-shaped reply reports the engine version `row` expects:
/// the identifier the channel was pinned with, and the catalogue's stored expiry
/// for it — or no `channelEngineVersion` at all for an unpinned channel.
fn assert_reported(row: &Row, reader: &str, base: SystemTime, channel: &Value) {
    let case = row.case;
    let reported = member(channel, "channelEngineVersion");
    let Some(version) = row.pinned else {
        assert_eq!(
            reported,
            Value::Null,
            "case {case:?}/{reader}: expected an unpinned channel to report no \
             channelEngineVersion, got {reported:?}"
        );
        return;
    };

    assert_eq!(
        member(&reported, "version"),
        json!(version),
        "case {case:?}/{reader}: expected the pinned build identifier {version:?}, got {reported:?}"
    );
    match row.expires {
        Some(expires) => {
            let reported_expiry = epoch_seconds(timestamp(case, "expirationDate", &reported));
            let want = crate::support::unix_seconds(shifted(base, expires));
            assert!(
                (reported_expiry - want).abs() < TOLERANCE_SECONDS,
                "case {case:?}/{reader}: expected expirationDate {want}, got {reported_expiry}"
            );
        }
        None => assert_eq!(
            member(&reported, "expirationDate"),
            Value::Null,
            "case {case:?}/{reader}: expected no expirationDate for an identifier the \
             catalogue does not hold, got {reported:?}"
        ),
    }
}

/// Rule: every channel-shaped read reports the engine version expiry the account's
/// catalogue stores for the build identifier the channel is pinned to.
#[tokio::test]
async fn a_channel_reports_the_catalogue_expiry_of_the_version_it_is_pinned_to() {
    let table = [
        Row {
            case: "the newest catalogue entry",
            pinned: Some("9.2.0"),
            expires: Some(7_200),
        },
        Row {
            case: "an older catalogue entry, which expires sooner",
            pinned: Some("9.0.0"),
            expires: Some(1_800),
        },
        Row {
            case: "a catalogue entry already past its expiry",
            pinned: Some("9.1.0"),
            expires: Some(-60),
        },
        Row {
            case: "an identifier the catalogue does not hold",
            pinned: Some("7.5.0"),
            expires: None,
        },
        Row {
            case: "a channel with no pin at all",
            pinned: None,
            expires: None,
        },
    ];

    for row in table {
        let case = row.case;
        let (router, engine) = medialive_with_engine();
        let base = SystemTime::now();
        install(
            &engine,
            ChannelEngineVersionCatalog::of(CATALOGUE.iter().map(
                |(version, released, expires)| {
                    ChannelEngineVersionRecord::expiring(
                        (*version).to_string(),
                        shifted(base, *released),
                        shifted(base, *expires),
                    )
                },
            )),
        );

        let mut request = json!({ "name": "engine-version-expiry" });
        if let Some(version) = row.pinned {
            request["channelEngineVersion"] = json!({ "version": version });
        }

        let created = create_channel(&router, case, request).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert_reported(&row, "CreateChannel", base, &created);

        let described = describe_channel(&router, case, &id).await;
        assert_reported(&row, "DescribeChannel", base, &described);

        let listed = listed_channel(&router, case, &id).await;
        assert_reported(&row, "ListChannels", base, &listed);

        // Reading the channel changed nothing a later caller sees: the same read
        // reports the same engine version again.
        let again = describe_channel(&router, case, &id).await;
        assert_eq!(
            member(&again, "channelEngineVersion"),
            member(&described, "channelEngineVersion"),
            "case {case:?}: a read must not change the engine version a later caller reads"
        );
    }
}

/// Rule, on the catalogue no test installed: the two readers of one stored instant
/// agree — a caller who takes a version out of `ListVersions` and pins it on a
/// channel reads that version back, with that expiry, from the channel.
///
/// Every version the account's seeded catalogue publishes is a row, so the
/// agreement is asserted across the whole published set rather than on one entry.
#[tokio::test]
async fn a_channel_reports_the_same_version_entry_list_versions_published() {
    let case = "the seeded catalogue";
    let router = medialive();
    let (status, body) = call(&router, "GET", "/prod/versions", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListVersions to answer 200, got {status}: {body}"
    );
    let published = body["versions"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a versions array, got {body}"));
    assert!(
        !published.is_empty(),
        "case {case:?}: expected ListVersions to publish the account's seeded versions, got {body}"
    );

    for entry in published {
        let version = entry["version"].as_str().unwrap_or_default().to_string();
        let created = create_channel(
            &router,
            case,
            json!({
                "name": format!("pinned-{version}"),
                "channelEngineVersion": { "version": version },
            }),
        )
        .await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let described = describe_channel(&router, case, &id).await;
        assert_eq!(
            member(&described, "channelEngineVersion"),
            entry,
            "case {case:?}/{version}: expected the channel to report the published entry {entry:?}, \
             got {:?}",
            member(&described, "channelEngineVersion")
        );
    }
}
