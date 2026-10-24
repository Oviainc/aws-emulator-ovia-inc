// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListVersions`.
//!
//! The rule every case below is one row of: `ListVersions` reports the account's STORED
//! Job engine version catalogue — "Retrieve a JSON array of all available Job engine
//! versions and the date they expire" — each entry labelled with its release date in the
//! `YYYY-MM-DD` form `JobEngineVersion$Version` documents and carrying the date it
//! expires, oldest release first, in pages of at most `maxResults` with an opaque
//! `nextToken` that resumes immediately after the last entry of the page just returned;
//! an entry whose expiration date has passed is no longer available, and the listing
//! writes nothing.
//!
//! Cases that are about the catalogue's CONTENT seed their records straight into the
//! store, because only a seeded record can carry a release or expiry instant a case
//! chose. Nothing here reaches past HTTP except that seeding and the store snapshot.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    JobEngineVersionRecord, JOB_ENGINE_VERSION_COUNT, JOB_ENGINE_VERSION_INTERVAL,
    JOB_ENGINE_VERSION_LIFETIME,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::{Emulator, Reply, REGION};

/// `GET /2017-08-29/versions` — the binding
/// `smithy_query(query="operation",operation="ListVersions")` reports, success code 200.
async fn list_versions(emulator: &Emulator, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/2017-08-29/versions".to_string()
    } else {
        format!("/2017-08-29/versions?{query}")
    };
    emulator.send_in(REGION, "GET", &uri, None).await
}

/// `(version, expirationDate)` of every entry in `versions`, in the order returned.
fn listed(reply: &Reply) -> Vec<(String, Option<i64>)> {
    reply
        .at("/versions")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item.pointer("/version")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.pointer("/expirationDate").and_then(Value::as_i64),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `version` label of every entry in `versions`, in the order returned.
fn labels(reply: &Reply) -> Vec<String> {
    listed(reply).into_iter().map(|(label, _)| label).collect()
}

/// The `nextToken`, or `None` when the response carries none.
fn next_token(reply: &Reply) -> Option<String> {
    reply.at("/nextToken").as_str().map(str::to_string)
}

/// Epoch seconds of an instant, the unit `ExpirationDate` is modelled in
/// (`__timestampUnix`, `epoch-seconds`).
fn epoch_secs(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
}

/// The stored catalogue, oldest release first: `(version, released_at, expires_at)`.
fn stored_catalogue(emulator: &Emulator) -> Vec<(String, SystemTime, SystemTime)> {
    let mut records: Vec<(String, SystemTime, SystemTime)> = emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_engine_versions
        .values()
        .map(|record| {
            (
                record.version.clone(),
                record.released_at,
                record.expires_at,
            )
        })
        .collect();
    records.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    records
}

/// Replace the catalogue with records the case chose, so a case can pin release and
/// expiry instants instead of racing the real clock.
fn seed_catalogue(emulator: &Emulator, records: Vec<JobEngineVersionRecord>) {
    let mut catalogue = BTreeMap::new();
    for record in records {
        catalogue.insert(record.version.clone(), record);
    }
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_engine_versions = catalogue;
}

/// A catalogue entry released at `released_at` that is still available: its label is the
/// one the state module derives from the release date, and its expiry is a lifetime from
/// NOW rather than from the release, so a case can seed a long span of releases without
/// any of them having expired.
fn available(released_at: SystemTime) -> JobEngineVersionRecord {
    JobEngineVersionRecord {
        expires_at: SystemTime::now() + JOB_ENGINE_VERSION_LIFETIME,
        ..JobEngineVersionRecord::released(released_at)
    }
}

/// A catalogue entry released `ago` before now and expiring `expires_in` from now, with
/// `label` as its version string. Built field by field so a case can choose an expiry
/// that is not a lifetime after the release.
fn entry(label: &str, ago: Duration, expires_in: Duration, expired: bool) -> JobEngineVersionRecord {
    let now = SystemTime::now();
    JobEngineVersionRecord {
        version: label.to_string(),
        released_at: now - ago,
        expires_at: if expired {
            now - expires_in
        } else {
            now + expires_in
        },
    }
}

/// What the emulator seeds when the account comes up IS the catalogue `ListVersions`
/// reports: as many entries as the state module publishes, one release interval apart,
/// each still inside its lifetime, oldest first, with the expiration date of the stored
/// record.
#[tokio::test]
async fn the_listing_projects_the_seeded_catalogue_in_release_order() {
    let emulator = Emulator::new();
    let stored = stored_catalogue(&emulator);
    assert_eq!(
        stored.len(),
        JOB_ENGINE_VERSION_COUNT,
        "case seeded catalogue: expected {JOB_ENGINE_VERSION_COUNT} stored versions, got \
         {stored:?}"
    );

    let reply = list_versions(&emulator, "").await;
    assert_eq!(
        reply.status, 200,
        "case seeded catalogue: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    let expected: Vec<(String, Option<i64>)> = stored
        .iter()
        .map(|(label, _, expires_at)| (label.clone(), Some(epoch_secs(*expires_at))))
        .collect();
    assert_eq!(
        listed(&reply),
        expected,
        "case seeded catalogue: the listing must project the stored records, got {:?}",
        listed(&reply)
    );
    assert_eq!(
        next_token(&reply),
        None,
        "case seeded catalogue: one page holds the whole catalogue, got {:?}",
        next_token(&reply)
    );

    // Consecutive releases are one interval apart, and each entry expires a lifetime
    // after its own release — both spans the state module publishes.
    for pair in stored.windows(2) {
        let [older, newer] = pair else { continue };
        let gap = newer
            .1
            .duration_since(older.1)
            .unwrap_or(Duration::from_secs(0));
        assert_eq!(
            gap, JOB_ENGINE_VERSION_INTERVAL,
            "case {:?} after {:?}: expected one release interval between them, got {gap:?}",
            newer.0, older.0
        );
    }
    let now = SystemTime::now();
    for (label, released_at, expires_at) in &stored {
        let lifetime = expires_at
            .duration_since(*released_at)
            .unwrap_or(Duration::from_secs(0));
        assert_eq!(
            lifetime, JOB_ENGINE_VERSION_LIFETIME,
            "case {label:?}: expected the published lifetime after its release, got \
             {lifetime:?}"
        );
        assert!(
            *expires_at > now,
            "case {label:?}: a freshly seeded version must still be available, it expires \
             at {:?}",
            epoch_secs(*expires_at)
        );
        assert!(
            *released_at <= now,
            "case {label:?}: no seeded version may be dated in the future, it is released \
             at {:?}",
            epoch_secs(*released_at)
        );
    }
}

/// A version's label is the CALENDAR DATE of its release, in the `YYYY-MM-DD` form
/// `JobEngineVersion$Version` documents ("Job engine versions are in a YYYY-MM-DD
/// format").
///
/// The rows are release instants in epoch seconds — the unit the model's `__timestampUnix`
/// members carry — against the UTC calendar date each one falls on: the epoch itself, the
/// two round epoch-second marks, a leap day and the day after it, and the end of a year.
#[tokio::test]
async fn a_version_label_is_the_calendar_date_of_its_release() {
    let cases: [(u64, &str); 6] = [
        (0, "1970-01-01"),
        (1_000_000_000, "2001-09-09"),
        (1_700_000_000, "2023-11-14"),
        // 2020 is a leap year, so 29 February exists in it.
        (1_582_934_400, "2020-02-29"),
        (1_583_020_800, "2020-03-01"),
        (1_609_459_199, "2020-12-31"),
    ];

    for (released_at_secs, expected) in cases {
        let emulator = Emulator::new();
        let released_at = UNIX_EPOCH + Duration::from_secs(released_at_secs);
        let record = JobEngineVersionRecord::released(released_at);
        assert_eq!(
            record.version, expected,
            "case release at {released_at_secs}: expected label {expected:?}, got {:?}",
            record.version
        );
        // And the label is what the wire carries. The record is given an expiry a lifetime
        // from now so the entry is available whatever the release date was.
        seed_catalogue(&emulator, vec![available(released_at)]);
        let reply = list_versions(&emulator, "").await;
        assert_eq!(
            labels(&reply),
            vec![expected.to_string()],
            "case release at {released_at_secs}: expected the listing to carry {expected:?}, \
             got {:?} from body {}",
            labels(&reply),
            reply.body
        );
    }
}

/// Only the AVAILABLE versions are reported: an entry past the date it expires is left
/// out, one that has not reached it is listed.
///
/// The rows span how far either side of now an expiry can fall, including the entry whose
/// expiry is the longest span the catalogue uses.
#[tokio::test]
async fn an_expired_version_is_no_longer_available() {
    let emulator = Emulator::new();
    let cases: [(&str, Duration, bool); 6] = [
        // (label, how far from now the expiry falls, whether it already passed)
        ("2019-01-01", Duration::from_secs(1), true),
        ("2019-04-01", Duration::from_secs(3_600), true),
        ("2019-07-01", JOB_ENGINE_VERSION_LIFETIME, true),
        ("2026-01-01", Duration::from_secs(60), false),
        ("2026-04-01", Duration::from_secs(86_400), false),
        ("2026-07-01", JOB_ENGINE_VERSION_LIFETIME, false),
    ];

    let mut records = Vec::new();
    // Released oldest-first in the order the rows are written, so the expected listing is
    // the available rows in that same order.
    for (index, (label, span, expired)) in cases.iter().enumerate() {
        let age = JOB_ENGINE_VERSION_INTERVAL * u32::try_from(cases.len() - index).unwrap_or(1);
        records.push(entry(label, age, *span, *expired));
    }
    seed_catalogue(&emulator, records);

    let expected: Vec<String> = cases
        .iter()
        .filter(|(_, _, expired)| !expired)
        .map(|(label, _, _)| (*label).to_string())
        .collect();
    let reply = list_versions(&emulator, "").await;
    assert_eq!(
        reply.status, 200,
        "case mixed expiry catalogue: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        labels(&reply),
        expected,
        "case mixed expiry catalogue: expected only the unexpired versions {expected:?}, got \
         {:?}",
        labels(&reply)
    );

    // An account whose every version has expired has nothing available to report — an
    // empty list, never a fabricated entry.
    let all_expired = Emulator::new();
    seed_catalogue(
        &all_expired,
        cases
            .iter()
            .map(|(label, span, _)| entry(label, JOB_ENGINE_VERSION_INTERVAL, *span, true))
            .collect(),
    );
    let reply = list_versions(&all_expired, "").await;
    assert_eq!(
        labels(&reply),
        Vec::<String>::new(),
        "case every version expired: expected an empty list, got {:?} from body {}",
        labels(&reply),
        reply.body
    );
    assert!(
        reply.at("/versions").is_array(),
        "case every version expired: expected the modelled list member, got body {}",
        reply.body
    );
}

/// Paging walks the ordering exactly once: concatenating the pages reproduces the whole
/// listing, no page is longer than asked for, and the last page carries no token.
///
/// The table spans the page sizes across the `maxResults` range (its published bounds, 1
/// and 20, and sizes in between) over more versions than one page holds.
#[tokio::test]
async fn paging_walks_every_version_exactly_once() {
    let emulator = Emulator::new();
    // 25 versions: more than the 20 a single page holds, so the default page size is
    // exercised too. Released a day apart, oldest first, each still available.
    let seeded = 25_usize;
    let records: Vec<JobEngineVersionRecord> = (0..seeded)
        .map(|index| {
            let age = Duration::from_secs(86_400) * u32::try_from(seeded - index).unwrap_or(1);
            available(SystemTime::now() - age)
        })
        .collect();
    let expected: Vec<String> = records.iter().map(|record| record.version.clone()).collect();
    seed_catalogue(&emulator, records);
    assert_eq!(
        expected.len(),
        seeded,
        "case seeding: every release must get its own label, got {expected:?}"
    );

    for page_size in [1_usize, 2, 7, 20] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match &token {
                Some(token) => format!("{case}&nextToken={token}"),
                None => case.clone(),
            };
            let reply = list_versions(&emulator, &query).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: page {pages} expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let page = labels(&reply);
            assert!(
                page.len() <= page_size,
                "case {case}: page {pages} expected at most {page_size} items, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            token = next_token(&reply);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= seeded + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case}: expected the whole catalogue in release order, got {seen:?}"
        );
        let expected_pages = seeded.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages, got {pages}"
        );
    }

    // No `maxResults`: "Number of valid Job engine versions, up to twenty, that will be
    // returned at one time", and a token for the rest.
    let first = list_versions(&emulator, "").await;
    assert_eq!(
        labels(&first).len(),
        20,
        "case default page size: expected 20 items, got {:?}",
        labels(&first)
    );
    let token = next_token(&first)
        .unwrap_or_else(|| panic!("case default page size: a {seeded}-version listing owes a token"));
    let second = list_versions(&emulator, &format!("nextToken={token}")).await;
    assert_eq!(
        labels(&second),
        expected[20..],
        "case default page size: expected the remaining versions, got {:?}",
        labels(&second)
    );
    assert_eq!(
        next_token(&second),
        None,
        "case default page size: the second page ends the listing, got {:?}",
        next_token(&second)
    );
}

/// `maxResults` carries the model's own range, 1..20: each published bound is accepted and
/// one step past either is refused before the handler runs.
#[tokio::test]
async fn max_results_is_held_to_the_range_the_model_declares() {
    let emulator = Emulator::new();

    for accepted in ["maxResults=1", "maxResults=20"] {
        let reply = list_versions(&emulator, accepted).await;
        assert_eq!(
            reply.status, 200,
            "case {accepted:?}: inside the range, expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
    }
    for refused in ["maxResults=0", "maxResults=21"] {
        let reply = list_versions(&emulator, refused).await;
        assert_eq!(
            reply.status, 400,
            "case {refused:?}: outside the range, expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
    }
}

/// A `nextToken` this service never issued is a bad request, not a panic and not a silent
/// first page. The table spans the ways a token can be wrong.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_a_bad_request() {
    let emulator = Emulator::new();

    let cases = [
        // Not hex at all.
        "not-a-token".to_string(),
        // Hex of a string with no `:` separator.
        ::hex::encode("2024-01-01"),
        // Hex of a separated pair whose first half is not an instant.
        ::hex::encode("soon:2024-01-01"),
        // Hex of an empty cursor.
        ::hex::encode(""),
    ];

    for token in cases {
        let reply = list_versions(&emulator, &format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {token:?}: expected BadRequestException, got {:?}",
            reply.error_type
        );
    }
}

/// Listing is a read: it changes nothing a later caller sees, however often and with
/// whatever paging it is called.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let before = stored_catalogue(&emulator);

    let queries = ["", "maxResults=1", "maxResults=2", "maxResults=20"];
    let mut first_bodies = Vec::new();
    for query in queries {
        first_bodies.push(list_versions(&emulator, query).await.body);
    }
    for (query, first) in queries.iter().zip(first_bodies) {
        let again = list_versions(&emulator, query).await.body;
        assert_eq!(
            again, first,
            "case {query:?}: a repeated listing must answer the same, got {again} then {first}"
        );
    }

    let after = stored_catalogue(&emulator);
    assert_eq!(
        after, before,
        "the store must be untouched by reads: expected {before:?}, got {after:?}"
    );
}
