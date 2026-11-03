// SPDX-License-Identifier: CC0-1.0

//! `ListPublicKeys`: the public halves of the key pairs CloudTrail signs a Region's digest
//! files with.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production router
//! serves — so what is asserted here is what a client sees, including the HTTP status, the
//! error code and the refusal text.
//!
//! The rules under test, each over a SET of windows rather than one:
//!
//! * the keys a call reports are the ones whose validity range overlaps the window the
//!   request asks about ("returns all public keys whose private keys were used to sign the
//!   digest files within the specified time range"), with an absent bound reading as the
//!   instant of the call ("if not specified, the current time is used, and the current
//!   public key is returned");
//! * no key exists for a period that has not begun, and none predates the service, so a
//!   window wholly in the future or wholly before CloudTrail existed selects nothing;
//! * a key is a real PKCS#1 public key with a fingerprint over its own body, and is stable:
//!   the same Region and the same window answer the same key every time, while two Regions
//!   answer different ones ("CloudTrail uses different private and public key pairs per
//!   Region");
//! * an inverted window is `InvalidTimeRangeException` in CloudTrail's own fixed sentence
//!   for this operation, while a `NextToken` — "reserved for future use" on the request, on
//!   the response and in the error modelled for it — selects nothing and refuses nothing.

use base64::Engine as _;
use md5::Digest as _;
use pkcs1::der::Decode as _;
use serde_json::{json, Value};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// The sentence CloudTrail refuses a window whose start is after its end with, as the real
/// service words it for THIS operation — `LookupEvents` refuses an inverted window in the
/// same error shape and a different sentence (`tests/all/request_validation.rs`).
const INVERTED_WINDOW: &str = "Invalid input parameter! Start time is after end time";

/// The operation under test.
const OPERATION: &str = "ListPublicKeys";

/// The HTTP status this awsJson1_1 API answers a client refusal with.
const REFUSED_STATUS: u16 = 400;

/// Milliseconds in a day, the unit the windows below are written in.
const DAY_MILLIS: i64 = 86_400_000;

/// The modulus size of the keys the service reports: 2048 bits, which is 256 bytes.
const MODULUS_BYTES: usize = 256;

/// The public exponent of an RSA key pair, big-endian: 65537.
const PUBLIC_EXPONENT: [u8; 3] = [0x01, 0x00, 0x01];

/// The slack a comparison of two wire timestamps carries, in seconds. An `awsJson1_1`
/// timestamp is a JSON number of epoch seconds, and the service reads and reports a window
/// bound to the millisecond, so half a millisecond is below the resolution either side can
/// disagree at meaningfully.
const SECOND_SLACK: f64 = 0.0005;

/// One key as a reply reports it.
#[derive(Debug, Clone, PartialEq)]
struct ReportedKey {
    /// `PublicKey$Value`, base64-decoded off the wire: the DER public key.
    value: Vec<u8>,
    /// `PublicKey$Fingerprint`.
    fingerprint: String,
    /// `PublicKey$ValidityStartTime`, in epoch seconds.
    start: f64,
    /// `PublicKey$ValidityEndTime`, in epoch seconds.
    end: f64,
}

impl ReportedKey {
    /// Whether this key was valid at `instant` (epoch seconds).
    fn valid_at(&self, instant: f64) -> bool {
        self.start <= instant && instant <= self.end
    }
}

/// The instant the caller reads the clock at, in whole milliseconds since the epoch — the
/// same reading the service makes, up to the few milliseconds a request takes.
fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_millis(),
    )
    .expect("an instant in this era fits an i64 of milliseconds")
}

/// A wire timestamp (epoch seconds, as `awsJson1_1` writes one) for `millis`.
fn wire_time(millis: i64) -> Value {
    json!(millis as f64 / 1_000.0)
}

/// Call `ListPublicKeys` in `region` for the window `[start, end]`, each bound in epoch
/// milliseconds and omitted from the request when `None`.
async fn list_in(emulator: &Emulator, region: &str, start: Option<i64>, end: Option<i64>) -> Reply {
    let mut body = serde_json::Map::new();
    if let Some(start) = start {
        body.insert("StartTime".to_string(), wire_time(start));
    }
    if let Some(end) = end {
        body.insert("EndTime".to_string(), wire_time(end));
    }
    emulator
        .call_in(OPERATION, region, &Value::Object(body))
        .await
}

/// The keys a successful reply reports, in the order it lists them.
///
/// It also pins what the response carries BESIDE the list: `NextToken` is "reserved for
/// future use", so a reply states none.
fn reported_keys(reply: &Reply, case: &str) -> Vec<ReportedKey> {
    let body = reply.expect_ok(case);
    assert!(
        body.get("NextToken").is_none(),
        "case {case:?}: NextToken is reserved for future use, so no reply states one, \
         got {body:?}"
    );
    let list = body
        .get("PublicKeyList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a PublicKeyList, got {body:?}"));
    list.iter()
        .map(|key| ReportedKey {
            value: base64::engine::general_purpose::STANDARD
                .decode(
                    key.get("Value")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| panic!("case {case:?}: expected a Value, got {key:?}")),
                )
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: expected base64 in Value, got {error} in {key:?}")
                }),
            fingerprint: key
                .get("Fingerprint")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a Fingerprint, got {key:?}"))
                .to_string(),
            start: key
                .get("ValidityStartTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a ValidityStartTime, got {key:?}")
                }),
            end: key
                .get("ValidityEndTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: expected a ValidityEndTime, got {key:?}")),
        })
        .collect()
}

/// Assert `key` is the kind of value the model documents: "the DER encoded public key value
/// in PKCS#1 format" with "the fingerprint of the public key" over it, inside a validity
/// range that runs forwards.
fn assert_well_formed(key: &ReportedKey, case: &str) {
    let parsed = pkcs1::RsaPublicKey::from_der(&key.value).unwrap_or_else(|error| {
        panic!("case {case:?}: expected DER PKCS#1 in Value, got {error} for {key:?}")
    });
    assert_eq!(
        parsed.modulus.as_bytes().len(),
        MODULUS_BYTES,
        "case {case:?}: expected a {}-bit modulus, got {} bytes",
        MODULUS_BYTES * 8,
        parsed.modulus.as_bytes().len()
    );
    assert_eq!(
        parsed.public_exponent.as_bytes(),
        PUBLIC_EXPONENT,
        "case {case:?}: expected the public exponent 65537, got {:?}",
        parsed.public_exponent.as_bytes()
    );
    // The fingerprint identifies the key it is reported with: the MD5 digest of that same
    // DER body, hex-encoded, which is the form a digest file records its signing key by.
    let expected = hex::encode(md5::Md5::digest(&key.value));
    assert_eq!(
        key.fingerprint, expected,
        "case {case:?}: expected the fingerprint of the reported key {expected:?}, got {:?}",
        key.fingerprint
    );
    assert!(
        key.start < key.end,
        "case {case:?}: expected a validity range that runs forwards, got {key:?}"
    );
}

/// Assert the reported keys are one run of a rotation schedule: oldest first, each taking
/// over where the previous one left off, no two the same key, and none valid later than
/// `now` + the request's own window can reach.
fn assert_schedule(keys: &[ReportedKey], case: &str) {
    for key in keys {
        assert_well_formed(key, case);
    }
    for pair in keys.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        assert!(
            earlier.end < later.start,
            "case {case:?}: expected the keys oldest first with no overlap, got {earlier:?} \
             before {later:?}"
        );
        assert!(
            (later.start - earlier.end).abs() <= 1.0,
            "case {case:?}: expected each key to take over where the previous one ended, \
             got a gap of {} seconds between {earlier:?} and {later:?}",
            later.start - earlier.end
        );
        assert_ne!(
            earlier.fingerprint, later.fingerprint,
            "case {case:?}: expected a distinct key per period, got {earlier:?} twice"
        );
        assert!(
            (later.end - later.start - (earlier.end - earlier.start)).abs() <= SECOND_SLACK,
            "case {case:?}: expected every key to be valid for the same span, got {earlier:?} \
             and {later:?}"
        );
    }
}

/// One window a listing is asked about, written as offsets from the instant of the call.
#[derive(Debug)]
struct WindowCase {
    case: &'static str,
    /// `StartTime`, in milliseconds from now; `None` leaves the member out.
    start: Option<i64>,
    /// `EndTime`, in milliseconds from now; `None` leaves the member out.
    end: Option<i64>,
    /// The fewest keys the window must select, given that a key signs for a bounded span.
    at_least: usize,
    /// Whether the window reaches the instant of the call, so the CURRENT key is among the
    /// keys it selects.
    covers_now: bool,
}

/// A listing reports the keys whose validity range overlaps the window asked about: the
/// window's own bounds are covered, every reported key overlaps it, and the run grows with
/// the window.
///
/// The rows differ in what the rule generalises over — which bounds are stated, how wide the
/// window is, and where it sits relative to the instant of the call — rather than in a value.
#[tokio::test]
async fn a_listing_reports_the_keys_whose_validity_overlaps_the_window() {
    let windows = [
        WindowCase {
            case: "neither bound: the current key",
            start: None,
            end: None,
            at_least: 1,
            covers_now: true,
        },
        WindowCase {
            case: "a start a hundred days back, no end",
            start: Some(-100 * DAY_MILLIS),
            end: None,
            at_least: 2,
            covers_now: true,
        },
        WindowCase {
            case: "a year back to now",
            start: Some(-365 * DAY_MILLIS),
            end: None,
            at_least: 5,
            covers_now: true,
        },
        WindowCase {
            case: "a year back, ended now",
            start: Some(-365 * DAY_MILLIS),
            end: Some(0),
            at_least: 5,
            covers_now: true,
        },
        WindowCase {
            case: "a window wholly in the past",
            start: Some(-365 * DAY_MILLIS),
            end: Some(-300 * DAY_MILLIS),
            at_least: 1,
            covers_now: false,
        },
        WindowCase {
            case: "a single past instant",
            start: Some(-200 * DAY_MILLIS),
            end: Some(-200 * DAY_MILLIS),
            at_least: 1,
            covers_now: false,
        },
        WindowCase {
            case: "no start, an end a day ahead",
            start: None,
            end: Some(DAY_MILLIS),
            at_least: 1,
            covers_now: true,
        },
        WindowCase {
            case: "a window that straddles the call into the future",
            start: Some(-10 * DAY_MILLIS),
            end: Some(300 * DAY_MILLIS),
            at_least: 1,
            covers_now: true,
        },
    ];

    let emulator = Emulator::new();
    for window in windows {
        let case = window.case;
        let now = now_millis();
        let reply = list_in(
            &emulator,
            HOME_REGION,
            window.start.map(|offset| now + offset),
            window.end.map(|offset| now + offset),
        )
        .await;
        let keys = reported_keys(&reply, case);
        assert_schedule(&keys, case);
        assert!(
            keys.len() >= window.at_least,
            "case {case:?}: expected at least {} key(s) for this window, got {}",
            window.at_least,
            keys.len()
        );

        // An absent bound is the instant of the call, so the window the service answered is
        // the one the row states, read against that instant.
        let now_secs = now as f64 / 1_000.0;
        let from = window
            .start
            .map_or(now_secs, |offset| now_secs + offset as f64 / 1_000.0);
        let to = window
            .end
            .map_or(now_secs, |offset| now_secs + offset as f64 / 1_000.0);

        for key in &keys {
            assert!(
                key.start - SECOND_SLACK <= to && key.end + SECOND_SLACK >= from,
                "case {case:?}: expected every reported key to overlap [{from}, {to}], \
                 got {key:?}"
            );
            // No key exists for a period CloudTrail has not reached yet, however far into the
            // future the window reaches.
            assert!(
                key.start <= now_secs + SECOND_SLACK,
                "case {case:?}: expected no key whose validity begins after the call, \
                 got {key:?}"
            );
        }

        // The oldest key reported covers the start of the window, and the newest covers its
        // end (or the instant of the call, when the window runs past it) — so the run is the
        // whole overlap rather than a sample of it.
        let first = keys.first().expect("the row asserts at least one key");
        let last = keys.last().expect("the row asserts at least one key");
        assert!(
            first.valid_at(from + SECOND_SLACK),
            "case {case:?}: expected the oldest reported key to cover the window's start \
             {from}, got {first:?}"
        );
        let latest = to.min(now_secs);
        assert!(
            last.valid_at(latest - SECOND_SLACK),
            "case {case:?}: expected the newest reported key to cover {latest}, got {last:?}"
        );
        assert_eq!(
            keys.iter().any(|key| key.valid_at(now_secs)),
            window.covers_now,
            "case {case:?}: expected the current key to be reported: {}",
            window.covers_now
        );
    }
}

/// A window no key of the schedule can fall in selects none: one wholly before CloudTrail
/// existed (its API version is 2013-11-01, so nothing it signed predates that), and one far
/// enough ahead that the period it asks about has not begun.
#[tokio::test]
async fn a_window_outside_the_schedule_reports_no_keys() {
    let now = now_millis();
    let empty: [(&str, i64, i64); 4] = [
        ("the epoch to the end of 1999", 0, 946_684_800_000),
        (
            "a window inside 2001, before the service existed",
            978_307_200_000,
            1_009_843_200_000,
        ),
        (
            "two hundred days ahead, for a hundred more",
            now + 200 * DAY_MILLIS,
            now + 300 * DAY_MILLIS,
        ),
        (
            "a decade ahead",
            now + 3_650 * DAY_MILLIS,
            now + 3_651 * DAY_MILLIS,
        ),
    ];

    let emulator = Emulator::new();
    for (case, start, end) in empty {
        let reply = list_in(&emulator, HOME_REGION, Some(start), Some(end)).await;
        let keys = reported_keys(&reply, case);
        assert!(
            keys.is_empty(),
            "case {case:?}: expected no keys for a window outside the schedule, got {keys:?}"
        );
    }
}

/// A key is a property of its Region and its period, not of the call: the same window
/// answers the same keys however often it is asked, and reading it leaves the next reader the
/// same answer. Two Regions answer the same schedule with different keys, since "CloudTrail
/// uses different private and public key pairs per Region".
#[tokio::test]
async fn keys_are_stable_per_region_and_differ_between_regions() {
    let emulator = Emulator::new();
    // Fixed bounds, so the two calls ask about exactly the same window rather than about two
    // windows a few milliseconds apart.
    let now = now_millis();
    let windows: [(&str, i64, i64); 3] = [
        ("a year back", now - 365 * DAY_MILLIS, now - 1_000),
        (
            "a single past instant",
            now - 200 * DAY_MILLIS,
            now - 200 * DAY_MILLIS,
        ),
        ("three years back", now - 1_095 * DAY_MILLIS, now - 1_000),
    ];
    let other_region = "eu-west-1";

    for (case, start, end) in windows {
        let home = reported_keys(
            &list_in(&emulator, HOME_REGION, Some(start), Some(end)).await,
            case,
        );
        let again = reported_keys(
            &list_in(&emulator, HOME_REGION, Some(start), Some(end)).await,
            case,
        );
        assert_eq!(
            home, again,
            "case {case:?}: expected the same keys on a second reading, got {again:?}"
        );
        assert!(
            !home.is_empty(),
            "case {case:?}: expected this window to select keys, got none"
        );

        let elsewhere = reported_keys(
            &list_in(&emulator, other_region, Some(start), Some(end)).await,
            case,
        );
        assert_eq!(
            elsewhere.len(),
            home.len(),
            "case {case:?}: expected {other_region} to run the same schedule, got \
             {elsewhere:?} against {home:?}"
        );
        for (mine, theirs) in home.iter().zip(&elsewhere) {
            assert_eq!(
                (mine.start, mine.end),
                (theirs.start, theirs.end),
                "case {case:?}: expected the same validity ranges in every Region, got \
                 {mine:?} and {theirs:?}"
            );
            assert_ne!(
                mine.value, theirs.value,
                "case {case:?}: expected a different key pair per Region, got {mine:?} in \
                 both"
            );
            assert_ne!(
                mine.fingerprint, theirs.fingerprint,
                "case {case:?}: expected a different fingerprint per Region, got {mine:?} in \
                 both"
            );
        }
    }
}

/// A window whose start is after its end is `InvalidTimeRangeException`, in CloudTrail's one
/// sentence for that refusal — whatever the two instants are and however far apart.
#[tokio::test]
async fn an_inverted_window_is_refused_in_one_sentence() {
    let now = now_millis();
    let inverted: [(&str, i64, i64); 5] = [
        ("a tenth of a second inverted", now, now - 100),
        ("a second inverted", now, now - 1_000),
        ("a year inverted", now, now - 365 * DAY_MILLIS),
        ("both bounds in the past, inverted", now - 1_000, now - 2_000),
        ("both bounds in the future, inverted", now + 2_000, now + 1_000),
    ];

    let emulator = Emulator::new();
    for (case, start, end) in inverted {
        let reply = list_in(&emulator, HOME_REGION, Some(start), Some(end)).await;
        reply.expect_error(case, REFUSED_STATUS, "InvalidTimeRangeException");
        assert_eq!(
            reply.message(),
            INVERTED_WINDOW,
            "case {case:?}: expected CloudTrail's one sentence for an inverted window, \
             got {reply:?}"
        );
    }
}

/// `NextToken` is "reserved for future use" on the request, on the response and in the
/// `InvalidTokenException` modelled for it: this operation paginates nothing, so a token
/// names no continuation — it neither changes which keys are reported nor is refused.
#[tokio::test]
async fn a_next_token_is_reserved_and_changes_nothing() {
    let tokens = [
        ("a word", "next-please"),
        ("hex that looks minted", "76313a303a30"),
        ("a marker from another listing", "eyJvZmZzZXQiOiAxfQ=="),
        ("a single character", "x"),
        ("the empty string", ""),
    ];

    let emulator = Emulator::new();
    let now = now_millis();
    let (start, end) = (now - 365 * DAY_MILLIS, now - 1_000);
    let without = reported_keys(
        &list_in(&emulator, HOME_REGION, Some(start), Some(end)).await,
        "no token at all",
    );
    assert!(
        !without.is_empty(),
        "the window under test must select keys, got none"
    );

    for (case, token) in tokens {
        let reply = emulator
            .call(
                OPERATION,
                &json!({
                    "StartTime": wire_time(start),
                    "EndTime": wire_time(end),
                    "NextToken": token,
                }),
            )
            .await;
        let keys = reported_keys(&reply, case);
        assert_eq!(
            keys, without,
            "case {case:?}: expected a reserved NextToken to change nothing, got {keys:?}"
        );
    }
}
