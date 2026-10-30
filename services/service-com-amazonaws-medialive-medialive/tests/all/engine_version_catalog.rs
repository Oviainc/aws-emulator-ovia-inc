// SPDX-License-Identifier: CC0-1.0

//! `ListVersions`, end to end over the restJson1 wire.
//!
//! The rule under test: `ListVersions` "retrieves an array of all the encoder
//! engine versions that are available in this AWS account", so it reports every
//! entry of the account's engine version catalogue whose expiry — `ExpirationDate`
//! is "The UTC time when the version expires" — is still ahead of the instant of
//! the call, each carrying its stored build identifier and that stored expiry as
//! an RFC 3339 date-time string, newest release first; an entry past its expiry is
//! no longer available and is not reported; and the read changes nothing a later
//! caller sees.
//!
//! The rows of the table below differ along what the rule generalises over: how
//! many versions the catalogue holds (none, one, several), which of them have
//! expired (none, some, all), how close an expiry is to the instant of the call,
//! the order the catalogue holds them in versus the order they were released in,
//! and whether two versions share a release instant.
//!
//! NOTHING in this emulator releases an encoder engine, and nothing here invents
//! one: an engine version is published by the SERVICE and no operation of it
//! creates, changes or deletes one, so an account the service has published none
//! to reports none (the fresh-account suite below), and the suites that span the
//! class install the catalogue they are about through the store's own type instead
//! of asking an operation to create a version it models no way to create.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_medialive_medialive::state::{
    self, channel_engine_version_label, ChannelEngineVersionCatalog, ChannelEngineVersionRecord,
    CHANNEL_ENGINE_VERSION_COUNT, CHANNEL_ENGINE_VERSION_INTERVAL, CHANNEL_ENGINE_VERSION_LIFETIME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{epoch_seconds, timestamp, unix_seconds};
use crate::wire::{call, medialive, medialive_with_engine, DEFAULT_REGION};

/// The URI `ListVersions` is bound to (`smithy_query(query="operation",
/// operation="ListVersions")`: GET /prod/versions).
const VERSIONS: &str = "/prod/versions";

/// How far two reported instants may be apart and still name the same instant:
/// the wire form carries sub-second precision, so the comparison is a tolerance
/// rather than an equality.
const TOLERANCE_SECONDS: f64 = 0.5;

/// `ListVersions`.
async fn list_versions(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GET", VERSIONS, None).await
}

/// An instant `offset` seconds away from `base` — negative into the past, which is
/// how a row dates a release, and positive into the future, which is how it dates
/// an expiry that has not come yet.
pub(crate) fn shifted(base: SystemTime, offset: i64) -> SystemTime {
    let span = Duration::from_secs(offset.unsigned_abs());
    if offset < 0 {
        base - span
    } else {
        base + span
    }
}

/// The catalogue a row installs, replacing the seeded one.
pub(crate) fn install(engine: &engine::InMemoryEngine, catalogue: ChannelEngineVersionCatalog) {
    let store = state::service_state_in(engine, DEFAULT_REGION).expect("the medialive store");
    store.lock().channel_engine_versions = catalogue;
}

/// The build identifiers the store holds, expired entries included.
fn stored_versions(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = state::service_state_in(engine, DEFAULT_REGION).expect("the medialive store");
    let held: Vec<String> = store
        .lock()
        .channel_engine_versions
        .versions
        .keys()
        .cloned()
        .collect();
    held
}

/// The `versions` list of a response, as the entries a client reads.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    match body.get("versions") {
        Some(Value::Array(entries)) => entries.clone(),
        other => panic!(
            "case {case:?}: expected a versions array, got {:?}",
            other.cloned().unwrap_or(Value::Null)
        ),
    }
}

/// The build identifiers a response reports, in the order it reports them.
fn reported_versions(case: &str, body: &Value) -> Vec<String> {
    entries(case, body)
        .iter()
        .map(|entry| {
            entry
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported version must carry a build identifier, got {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// One row: the catalogue to install, and the build identifiers `ListVersions` must
/// then report, in order.
struct Row {
    case: &'static str,
    /// The entries of the installed catalogue: `(build identifier, seconds from the
    /// base instant the version was released, seconds from it that it expires)`.
    catalogue: &'static [(&'static str, i64, i64)],
    /// The build identifiers the response must carry, newest release first.
    expected: &'static [&'static str],
}

/// Rule: a call reports exactly the catalogue entries that have not expired at the
/// instant of the call, newest release first, each carrying its stored expiry.
#[tokio::test]
async fn list_versions_reports_the_unexpired_catalogue_newest_first() {
    let table = [
        Row {
            case: "three versions, none expired, catalogue order is not release order",
            catalogue: &[
                ("2.0.0", -200, 3_600),
                ("2.2.0", -100, 7_200),
                ("2.1.0", -300, 1_800),
            ],
            expected: &["2.2.0", "2.0.0", "2.1.0"],
        },
        Row {
            case: "one version, available",
            catalogue: &[("4.0.0", -10, 60)],
            expected: &["4.0.0"],
        },
        Row {
            case: "an expiry a minute away is still available",
            catalogue: &[("5.0.0", -10, 60)],
            expected: &["5.0.0"],
        },
        Row {
            case: "one of three has expired",
            catalogue: &[
                ("6.0.0", -300, 3_600),
                ("6.1.0", -200, -1),
                ("6.2.0", -100, 3_600),
            ],
            expected: &["6.2.0", "6.0.0"],
        },
        Row {
            case: "every version has expired",
            catalogue: &[("7.0.0", -3_600, -1_800), ("7.1.0", -1_800, -60)],
            expected: &[],
        },
        Row {
            case: "an empty catalogue",
            catalogue: &[],
            expected: &[],
        },
        Row {
            case: "two versions released at the same instant order by build identifier",
            catalogue: &[("8.0.0", -100, 3_600), ("8.1.0", -100, 3_600)],
            expected: &["8.1.0", "8.0.0"],
        },
    ];

    for row in table {
        let case = row.case;
        let (router, engine) = medialive_with_engine();
        let base = SystemTime::now();
        install(
            &engine,
            ChannelEngineVersionCatalog::of(row.catalogue.iter().map(
                |(version, released, expires)| {
                    ChannelEngineVersionRecord::expiring(
                        (*version).to_string(),
                        shifted(base, *released),
                        shifted(base, *expires),
                    )
                },
            )),
        );

        let (status, body) = list_versions(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListVersions to answer 200, got {status}: {body}"
        );
        let reported = reported_versions(case, &body);
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected the versions {:?}, got {reported:?}",
            row.expected
        );

        // Each reported entry carries the expiry its record was built with, in the
        // one wire form the modelled `__timestampIso8601` target admits.
        for (entry, version) in entries(case, &body).iter().zip(row.expected) {
            let (_, _, expires) = row
                .catalogue
                .iter()
                .find(|(label, _, _)| label == version)
                .expect("the row names an installed version");
            let reported_expiry = epoch_seconds(timestamp(case, "expirationDate", entry));
            let want = unix_seconds(shifted(base, *expires));
            assert!(
                (reported_expiry - want).abs() < TOLERANCE_SECONDS,
                "case {case:?}/{version}: expected expirationDate {want}, got {reported_expiry}"
            );
        }

        // A read changes nothing: the second call reports the same versions, and the
        // store still holds every record, the expired ones included.
        let (status, again) = list_versions(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second ListVersions to answer 200, got {status}: {again}"
        );
        assert_eq!(
            reported_versions(case, &again),
            reported,
            "case {case:?}: a read must not change what a later caller sees, got {again}"
        );
        let mut held: Vec<String> = row
            .catalogue
            .iter()
            .map(|(version, _, _)| (*version).to_string())
            .collect();
        held.sort();
        assert_eq!(
            stored_versions(&engine),
            held,
            "case {case:?}: a read must leave every stored version where it is"
        );
    }
}

/// Rule, on the catalogue no test installed: an engine version is published by
/// the SERVICE and created by no operation of it, so an account the service has
/// published none to reports none — `ListVersions` answers 200 with an empty set
/// rather than with versions and expiry dates derived from the emulator's own
/// clock, which would differ on every run.
#[tokio::test]
async fn a_fresh_account_publishes_no_versions() {
    let case = "a fresh account";
    let router = medialive();
    let (status, body) = list_versions(&router).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListVersions to answer 200, got {status}: {body}"
    );
    let reported = reported_versions(case, &body);
    assert!(
        reported.is_empty(),
        "case {case:?}: expected a fresh account to publish no versions, got {reported:?}"
    );

    // Reading again reports the same thing: the read published nothing into the
    // catalogue it answers from.
    let (status, again) = list_versions(&router).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the second ListVersions to answer 200, got {status}: {again}"
    );
    assert!(
        reported_versions(case, &again).is_empty(),
        "case {case:?}: expected the second read to publish no versions either, got {again}"
    );
}

/// Rule: a published series of engine versions reports, newest release first,
/// every entry still inside its lifetime — the labels the series is named by
/// ([`channel_engine_version_label`]), one [`CHANNEL_ENGINE_VERSION_INTERVAL`]
/// apart, each expiring a [`CHANNEL_ENGINE_VERSION_LIFETIME`] after its release.
///
/// The series is INSTALLED through the store's own type, because the service is
/// what publishes an account's versions and this emulator invents none.
#[tokio::test]
async fn an_installed_series_reports_newest_first_with_its_stored_expiries() {
    let case = "an installed release series";
    let (router, engine) = medialive_with_engine();
    let released_at = SystemTime::now();
    let series: Vec<ChannelEngineVersionRecord> = (0..CHANNEL_ENGINE_VERSION_COUNT)
        .map(|release| {
            let generations = u32::try_from(CHANNEL_ENGINE_VERSION_COUNT - 1 - release)
                .expect("the series is short");
            ChannelEngineVersionRecord::released(
                channel_engine_version_label(release),
                released_at - CHANNEL_ENGINE_VERSION_INTERVAL * generations,
            )
        })
        .collect();
    install(&engine, ChannelEngineVersionCatalog::of(series));

    let (status, body) = list_versions(&router).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListVersions to answer 200, got {status}: {body}"
    );

    // The installed labels, newest release first: the series is counted from its
    // oldest release, so the newest is the last label.
    let expected: Vec<String> = (0..CHANNEL_ENGINE_VERSION_COUNT)
        .rev()
        .map(channel_engine_version_label)
        .collect();
    let reported = reported_versions(case, &body);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the versions {expected:?}, got {reported:?}"
    );

    // Each entry expires a lifetime after the instant it was released at, and
    // consecutive releases are one interval apart.
    let expiries: Vec<f64> = entries(case, &body)
        .iter()
        .map(|entry| epoch_seconds(timestamp(case, "expirationDate", entry)))
        .collect();
    let lifetime = CHANNEL_ENGINE_VERSION_LIFETIME.as_secs_f64();
    let interval = CHANNEL_ENGINE_VERSION_INTERVAL.as_secs_f64();
    let newest = expiries
        .first()
        .copied()
        .expect("the installed series reports at least one version");
    assert!(
        (newest - (unix_seconds(released_at) + lifetime)).abs() < TOLERANCE_SECONDS,
        "case {case:?}: expected the newest version to expire a lifetime after its release \
         ({}), got {newest}",
        unix_seconds(released_at) + lifetime
    );
    for (index, expiry) in expiries.iter().enumerate() {
        assert!(
            *expiry > unix_seconds(released_at),
            "case {case:?}/{}: every entry of the series must still be available, \
             but it expires at {expiry}",
            reported[index]
        );
        if let Some(previous) = index.checked_sub(1).and_then(|at| expiries.get(at)) {
            let gap = previous - expiry;
            assert!(
                (gap - interval).abs() < TOLERANCE_SECONDS,
                "case {case:?}/{}: expected consecutive releases {interval} seconds apart, \
                 got {gap}",
                reported[index]
            );
        }
    }
}
