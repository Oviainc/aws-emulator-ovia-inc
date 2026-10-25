// SPDX-License-Identifier: CC0-1.0

//! What `GetDistributions` reports when the CALLER chooses the page token, and
//! what it leaves behind — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetDistributions` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson envelope are all exercised rather than bypassed. The
//! harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: a paged listing reports the stored records from the
//! caller's `pageToken` onward in the store's own total order by name, no more
//! than a page of them at a time, naming the record the next page starts at and
//! naming none when the page is the last — and, being a REPORTING operation, it
//! changes nothing a later caller sees.
//!
//! `distribution_cdn.rs` walks the pages a listing issues tokens for and pins
//! what one record carries. This suite takes the other half of the same rule: a
//! token is a POSITION IN A TOTAL ORDER, so it is tested as a table of tokens the
//! caller could send — absent, empty, below every stored name, equal to a stored
//! name, between two stored names, equal to the last, and past every name — each
//! against the records that order puts from there on, with and without more
//! records than one page holds. A listing answered twice is tested for being the
//! same listing.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::Emulator;
use crate::paging::names;

/// How many distributions one `GetDistributions` page carries
/// (`crate::state::DISTRIBUTION_PAGE_SIZE`).
const PAGE_SIZE: usize = 10;

/// The instance every distribution in this suite pulls from: the origin's own
/// configuration is not what any case here is about.
const ORIGIN_INSTANCE: &str = "origin-instance";
const ORIGIN_BLUEPRINT: &str = "amazon_linux_2023";
const ORIGIN_INSTANCE_BUNDLE: &str = "nano_3_0";
const ORIGIN_ZONE: &str = "us-east-1a";

/// The distribution bundle every distribution in this suite runs on.
const BUNDLE: &str = "small_1_0";

/// The status a distribution reports once the configuration a call stored has
/// deployed — the one member of a distribution's record the clock still moves,
/// so a case about what a read leaves behind waits for it first.
const STATUS_DEPLOYED: &str = "Deployed";

/// Create the origin instance every case pulls from.
async fn create_origin(emulator: &Emulator) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [ORIGIN_INSTANCE],
                "availabilityZone": ORIGIN_ZONE,
                "blueprintId": ORIGIN_BLUEPRINT,
                "bundleId": ORIGIN_INSTANCE_BUNDLE,
            }),
        )
        .await;
}

/// Create the distribution `name`, and answer the record the create reported.
async fn create_distribution(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": BUNDLE,
            }),
        )
        .await["distribution"]
        .clone()
}

/// One `GetDistributions` page: the records it reported and the token it named
/// for the page after it.
async fn page(emulator: &Emulator, case: &str, token: Option<&str>) -> (Vec<Value>, Option<String>) {
    let request = match token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    let body = emulator.ok("GetDistributions", request).await;
    let records = body["distributions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: a listing answers a distributions array, got {body:?}")
        })
        .clone();
    let next = body["nextPageToken"].as_str().map(str::to_string);
    (records, next)
}

/// One token a caller can send, and the stored names the order puts from there
/// on.
#[derive(Debug, Clone, Copy)]
struct TokenCase {
    case: &'static str,
    /// `None` is a request that carries no `pageToken` at all.
    token: Option<&'static str>,
    expected: &'static [&'static str],
}

/// The stored names this suite's token table runs against, fewer than
/// [`PAGE_SIZE`] of them so a row is about the token alone. Created in an order
/// that is NOT the sorted one, so a row that expects sorted names is asserting
/// the store's order rather than the order of creation.
const STORED: &[&str] = &["cdn-f", "cdn-b", "cdn-j", "cdn-d", "cdn-h"];

/// Every stored name, in the total order a listing reports them in.
const SORTED: &[&str] = &["cdn-b", "cdn-d", "cdn-f", "cdn-h", "cdn-j"];

/// The token table. The rows differ along where the token falls in the stored
/// order — before it, on one of its names, between two of them, on the last one,
/// and past all of them — which is what the rule generalises over.
const TOKEN_CASES: &[TokenCase] = &[
    TokenCase {
        case: "no token at all",
        token: None,
        expected: SORTED,
    },
    TokenCase {
        case: "an empty token",
        token: Some(""),
        expected: SORTED,
    },
    TokenCase {
        case: "a token below every stored name",
        token: Some("cdn-a"),
        expected: SORTED,
    },
    TokenCase {
        case: "a token that is a stored name",
        token: Some("cdn-f"),
        expected: &["cdn-f", "cdn-h", "cdn-j"],
    },
    TokenCase {
        case: "a token between two stored names",
        token: Some("cdn-e"),
        expected: &["cdn-f", "cdn-h", "cdn-j"],
    },
    TokenCase {
        case: "a token that is the last stored name",
        token: Some("cdn-j"),
        expected: &["cdn-j"],
    },
    TokenCase {
        case: "a token past every stored name",
        token: Some("cdn-z"),
        expected: &[],
    },
];

/// Every row: the page is the stored distributions from the token onward, and —
/// because the whole store fits in one page from any of these positions — no
/// next token is named.
#[tokio::test(flavor = "current_thread")]
async fn a_page_token_names_the_position_a_listing_resumes_at() {
    let emulator = Emulator::new();
    create_origin(&emulator).await;
    for name in STORED {
        create_distribution(&emulator, name).await;
    }

    for row in TOKEN_CASES {
        let (records, next) = page(&emulator, row.case, row.token).await;
        let reported = names(&records);
        assert_eq!(
            reported, row.expected,
            "case {}: expected {:?} from the token onward, got {reported:?}",
            row.case, row.expected
        );
        assert_eq!(
            next, None,
            "case {}: a page holding every remaining record names no next token, got {next:?}",
            row.case
        );
    }
}

/// A caller-chosen token is bounded by the page size exactly as an issued one is:
/// from a position with more records left than a page holds, the page is full,
/// names the record the next page starts at, and the walk from there reports
/// every remaining record once.
#[tokio::test(flavor = "current_thread")]
async fn a_caller_chosen_token_is_bounded_by_the_page_size() {
    let emulator = Emulator::new();
    create_origin(&emulator).await;

    // More records than one page holds, so a token two in from the start still
    // leaves a second page behind it.
    let total = PAGE_SIZE + 3;
    let all: Vec<String> = (0..total).map(|index| format!("cdn-pos-{index:03}")).collect();
    for name in &all {
        create_distribution(&emulator, name).await;
    }

    let from = 2;
    let case = "a token two in from the start";
    let (records, next) = page(&emulator, case, Some(&all[from])).await;
    let reported = names(&records);
    let expected: Vec<String> = all[from..from + PAGE_SIZE].to_vec();
    assert_eq!(
        reported, expected,
        "case {case}: expected a full page from the token onward, got {reported:?}"
    );
    assert_eq!(
        next.as_deref(),
        Some(all[from + PAGE_SIZE].as_str()),
        "case {case}: expected the next token to name the record the next page starts at, got \
         {next:?}"
    );

    let case = "the page after it";
    let (records, next) = page(&emulator, case, next.as_deref()).await;
    let reported = names(&records);
    let expected: Vec<String> = all[from + PAGE_SIZE..].to_vec();
    assert_eq!(
        reported, expected,
        "case {case}: expected the remaining records, got {reported:?}"
    );
    assert_eq!(
        next, None,
        "case {case}: expected no token on the last page, got {next:?}"
    );
}

/// A store that holds no distribution at all answers an empty list — for a
/// request with a token as much as for one without — rather than an absent
/// member or a refusal.
#[tokio::test(flavor = "current_thread")]
async fn an_empty_store_reports_an_empty_listing() {
    let emulator = Emulator::new();

    for token in [None, Some(""), Some("cdn-anything")] {
        let case = format!("an empty store with token {token:?}");
        let (records, next) = page(&emulator, &case, token).await;
        assert!(
            records.is_empty(),
            "case {case}: expected an empty list, got {records:?}"
        );
        assert_eq!(
            next, None,
            "case {case}: expected no next token, got {next:?}"
        );
    }
}

/// Poll the listing until `name` reports `STATUS_DEPLOYED`: the one transition
/// the clock owes a created distribution, which arrives without anyone asking
/// more than once.
async fn wait_until_deployed(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (records, _) = page(emulator, "waiting for the deploy", None).await;
        let status = records
            .iter()
            .find(|record| record["name"].as_str() == Some(name))
            .map(|record| record["status"].clone())
            .unwrap_or(Value::Null);
        if status.as_str() == Some(STATUS_DEPLOYED) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the deploy of {name}: expected '{STATUS_DEPLOYED}' within 5s, got \
             {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A listing is a REPORT: reading it, with any token, mints nothing, drops
/// nothing and moves nothing, so a later caller sees exactly what the first one
/// saw.
#[tokio::test(flavor = "current_thread")]
async fn reading_the_listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    create_origin(&emulator).await;
    for name in SORTED.iter().take(3) {
        create_distribution(&emulator, name).await;
    }
    // Past the one transition the clock owes, so what remains is only what a
    // read itself would change.
    for name in SORTED.iter().take(3) {
        wait_until_deployed(&emulator, name).await;
    }

    let (before, _) = page(&emulator, "the first listing", None).await;

    // Read it every way this operation can be read, repeatedly.
    for round in 0..3 {
        for token in [None, Some(""), Some("cdn-d"), Some("cdn-z")] {
            page(&emulator, &format!("round {round} token {token:?}"), token).await;
        }
        for name in SORTED.iter().take(3) {
            let body = emulator
                .ok("GetDistributions", json!({"distributionName": name}))
                .await;
            assert_eq!(
                body["distributions"].as_array().map(Vec::len),
                Some(1),
                "case round {round} reading {name}: a named read answers exactly that \
                 distribution, got {body:?}"
            );
        }
    }

    let (after, next) = page(&emulator, "the listing afterwards", None).await;
    assert_eq!(
        after, before,
        "case repeated reads: expected the listing to be unchanged, got {after:?}"
    );
    assert_eq!(
        next, None,
        "case repeated reads: expected no next token, got {next:?}"
    );
}
