// SPDX-License-Identifier: CC0-1.0

//! The CURSOR rule every listing of this service shares, end to end over HTTP, for all
//! FOUR of them.
//!
//! The rule every case here asserts: a listing's `nextToken` is this service's own cursor
//! into the listing, and the service issues one only while elements remain PAST the page
//! it closes — so every token a walk is handed names an element of the listing, a request
//! that names no token gets the listing from its beginning however short it is (including
//! empty), and a token that names no element the service issued — one at or past the end
//! of the listing, or one that is no position at all — is refused with HTTP 400 and the
//! listing's own client-input error code, whichever listing it is handed to.
//!
//! Rows span what the rule generalises over: the operation (`ListProfilingGroups`,
//! `ListFindingsReports`, `GetFindingsReportAccountSummary` and `ListProfileTimes`, which
//! differ in what they list, how they order it and where the cursor is read), the length
//! of the listing (three, five and zero elements), the page size (one, two, exactly the
//! listing length and one above it) and the cursor (every position that names an element,
//! exactly the end, one past it, far past it, and three tokens that name no position).
//! [`crate::list_pagination`] asserts the page CONTENT and ordering side of the same rule.
//!
//! The emulator, the HTTP plumbing and the seeding helpers are the shared
//! [`crate::profiling_group_lifecycle`] and [`crate::profile_times`] ones, not a fork of
//! them.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    ComAmazonawsCodeguruprofilerCodeguruprofilerState, FindingsReport,
};

use crate::interval_validation::create_group;
use crate::profile_times::place_receipts;
use crate::profiling_group_lifecycle::{Answer, Harness, PROFILE_START_MILLIS, json_profile};

/// The ONE sentence every listing of this service states about a token it never issued.
const UNISSUED: &str = "nextToken is not a token this service issued";

/// 2020-06-01T00:00:00Z: a whole number of days past the epoch, so a window aligned to
/// UTC starts here for every aggregation period, and far enough in the past that the
/// aggregation of every window seeded below has long fallen due.
const WINDOW_START: i64 = 1_590_969_600;

/// One five-minute aggregation window, in seconds — the finest period the model's
/// `AggregationPeriod` declares (`PT5M`).
const FIVE_MINUTES: i64 = 300;

/// One day in seconds, for the analysed windows the seeded reports cover.
const DAY: i64 = 24 * 60 * 60;

/// The range the ranged listings name: it covers every window seeded below.
const RANGE_START: i64 = WINDOW_START - 1;
const RANGE_END: i64 = WINDOW_START + 5 * DAY;

/// The group the per-group listings page over, and the one holding the seeded profiles.
const ANALYSED_GROUP: &str = "cursor-alpha";

/// A stored group with no analysis and no profiles, so the group listing is longer than
/// the listings about one group.
const PLAIN_GROUP: &str = "cursor-beta";

/// A second group holding analysis, so the account-wide summary is longer than either
/// group's own listing.
const SECOND_ANALYSED_GROUP: &str = "cursor-zeta";

/// Every group the fixture stores, in NAME order (they are created in another order).
const GROUPS: [&str; 3] = [ANALYSED_GROUP, PLAIN_GROUP, SECOND_ANALYSED_GROUP];

/// Recorded findings reports as (id, the analysed window's start). A `FindingsReportId`
/// is modelled `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`.
const ALPHA_REPORTS: [(&str, i64); 3] = [
    ("aaaaaaaa-0001-4aaa-8aaa-000000000001", WINDOW_START),
    ("aaaaaaaa-0002-4aaa-8aaa-000000000002", WINDOW_START + DAY),
    ("aaaaaaaa-0003-4aaa-8aaa-000000000003", WINDOW_START + 2 * DAY),
];
const ZETA_REPORTS: [(&str, i64); 2] = [
    ("cccccccc-0001-4ccc-8ccc-000000000001", WINDOW_START),
    ("cccccccc-0002-4ccc-8ccc-000000000002", WINDOW_START + DAY),
];

/// Receipt times of the seeded profiles: one per five-minute window, so the group holds
/// as many aggregated profile times as there are receipts.
const RECEIPTS: [i64; 3] = [
    WINDOW_START,
    WINDOW_START + FIVE_MINUTES,
    WINDOW_START + 2 * FIVE_MINUTES,
];

/// The aggregation period the seeded reports analysed, as `AggregationPeriod` spells it.
const DAILY: &str = "P1D";

/// An instant as the ISO 8601 `date-time` text the modelled query members carry.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// The listings of this service that report an already-ordered sequence a page at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listing {
    /// `ListProfilingGroups`, over the stored groups.
    ProfilingGroups,
    /// `ListFindingsReports`, over one group's recorded analysis.
    FindingsReports,
    /// `GetFindingsReportAccountSummary`, over every group's recorded analysis.
    AccountSummary,
    /// `ListProfileTimes`, over the aggregated profiles one group holds.
    ProfileTimes,
}

impl Listing {
    /// Every listing the rule covers.
    const ALL: [Listing; 4] = [
        Listing::ProfilingGroups,
        Listing::FindingsReports,
        Listing::AccountSummary,
        Listing::ProfileTimes,
    ];

    /// The request URI, with `page` (a `maxResults`/`nextToken` fragment, possibly empty)
    /// added to the query members the operation itself requires.
    fn uri(self, page: &str) -> String {
        let range = vec![
            format!("startTime={}", iso(RANGE_START)),
            format!("endTime={}", iso(RANGE_END)),
        ];
        let (path, mut members) = match self {
            Listing::ProfilingGroups => ("/profilingGroups".to_string(), Vec::new()),
            Listing::FindingsReports => (
                format!("/internal/profilingGroups/{ANALYSED_GROUP}/findingsReports"),
                range,
            ),
            Listing::AccountSummary => ("/internal/findingsReports".to_string(), Vec::new()),
            Listing::ProfileTimes => {
                let mut members = range;
                // `period` is modelled required for this operation.
                members.push("period=PT5M".to_string());
                (
                    format!("/profilingGroups/{ANALYSED_GROUP}/profileTimes"),
                    members,
                )
            }
        };
        if !page.is_empty() {
            members.push(page.to_string());
        }
        if members.is_empty() {
            path
        } else {
            format!("{path}?{}", members.join("&"))
        }
    }

    /// The elements of one page, identified: a group by its name, a report by its id, an
    /// aggregated profile by its start instant.
    fn elements(self, answer: &Answer) -> Vec<String> {
        match self {
            Listing::ProfilingGroups => texts_of(answer, "profilingGroupNames"),
            Listing::FindingsReports => ids_of(answer, "findingsReportSummaries"),
            Listing::AccountSummary => ids_of(answer, "reportSummaries"),
            Listing::ProfileTimes => crate::profile_times::profile_times(answer)
                .iter()
                .map(i64::to_string)
                .collect(),
        }
    }

    /// How many elements the seeded listing holds, from the fixture itself.
    fn length(self) -> usize {
        match self {
            Listing::ProfilingGroups => GROUPS.len(),
            Listing::FindingsReports => ALPHA_REPORTS.len(),
            Listing::AccountSummary => ALPHA_REPORTS.len() + ZETA_REPORTS.len(),
            Listing::ProfileTimes => RECEIPTS.len(),
        }
    }

    /// Whether the listing is about ONE group, which therefore has to be stored for the
    /// request to be answered at all rather than refused as a missing resource.
    fn is_about_one_group(self) -> bool {
        matches!(self, Listing::FindingsReports | Listing::ProfileTimes)
    }
}

/// A list of strings carried by `member`.
fn texts_of(answer: &Answer, member: &str) -> Vec<String> {
    array_of(answer, member)
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// `<member>[*].id`, in the order the response lists them.
fn ids_of(answer: &Answer, member: &str) -> Vec<String> {
    array_of(answer, member)
        .iter()
        .map(|element| {
            element
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

fn array_of(answer: &Answer, member: &str) -> Vec<serde_json::Value> {
    answer
        .json()
        .get(member)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "no {member} array in {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
}

fn next_token(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("nextToken")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// One page of `listing`, asked for with the `page` query fragment.
async fn page_of(harness: &Harness, listing: Listing, page: &str) -> Answer {
    harness
        .send("GET", &listing.uri(page), "application/json", Vec::new())
        .await
}

/// Record the seeded analysis through the store's own writer — the only way a findings
/// report exists, since this emulator runs no recommendation analysis of its own.
fn record_reports(harness: &Harness, group: &str, reports: &[(&str, i64)]) {
    let state = harness
        .engine()
        .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
    let mut guard = state.lock();
    for (id, start) in reports {
        let recorded = guard.record_findings_report(
            group,
            FindingsReport {
                id: (*id).to_string(),
                profile_start_time: DateTime::from_secs(*start),
                profile_end_time: DateTime::from_secs(start + DAY),
                total_number_of_findings: 1,
                aggregation_period: DAILY.to_string(),
            },
        );
        assert!(
            recorded,
            "recording analysis {id:?} must find the group {group:?}"
        );
    }
}

/// Three stored groups: one holding analysis and one profile per five-minute window, one
/// holding analysis only, one holding nothing.
async fn seeded() -> Harness {
    let harness = Harness::new();
    // Created out of name order, so each listing's own ordering is what the pages see.
    create_group(&harness, SECOND_ANALYSED_GROUP).await;
    create_group(&harness, ANALYSED_GROUP).await;
    create_group(&harness, PLAIN_GROUP).await;
    record_reports(&harness, ANALYSED_GROUP, &ALPHA_REPORTS);
    record_reports(&harness, SECOND_ANALYSED_GROUP, &ZETA_REPORTS);
    for index in 0..RECEIPTS.len() {
        let posted = harness
            .post_agent_profile(
                ANALYSED_GROUP,
                "application/json",
                None,
                // Distinct bytes per profile, so no two uploads are the same document.
                json_profile(PROFILE_START_MILLIS + index as i64, index),
            )
            .await;
        assert_eq!(
            posted.status,
            204,
            "posting profile {index} must succeed, got {} {}",
            posted.status,
            String::from_utf8_lossy(&posted.body)
        );
    }
    // The receipts the clock stamped are all in one window; the seeded ones spread the
    // profiles over as many closed windows as there are receipts.
    place_receipts(&harness, ANALYSED_GROUP, &RECEIPTS);
    harness
}

/// An emulator whose `listing` is EMPTY: no group is stored at all, except where the
/// listing is about one group, which then holds neither analysis nor profiles.
async fn emptied(listing: Listing) -> Harness {
    let harness = Harness::new();
    if listing.is_about_one_group() {
        create_group(&harness, ANALYSED_GROUP).await;
    }
    harness
}

#[tokio::test]
async fn no_listing_ever_issues_a_cursor_at_its_end() {
    let harness = seeded().await;

    for listing in Listing::ALL {
        let whole = listing.elements(&page_of(&harness, listing, "").await);
        assert_eq!(
            whole.len(),
            listing.length(),
            "case {listing:?}: the fixture must hold {} elements, got {whole:?}",
            listing.length()
        );

        // Page sizes inside the modelled `maxResults` range (1..1000): the smallest, one
        // that leaves a partial last page, exactly the listing length and one above it.
        for size in [1usize, 2, whole.len(), whole.len() + 1] {
            let case = format!("{listing:?} walked with maxResults={size}");
            let mut seen: Vec<String> = Vec::new();
            let mut issued: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut page = format!("maxResults={size}");
                if let Some(cursor) = token.as_deref() {
                    page.push_str(&format!("&nextToken={cursor}"));
                }
                let answer = page_of(&harness, listing, &page).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: expected 200, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                seen.extend(listing.elements(&answer));
                token = next_token(&answer);
                match token.as_deref() {
                    Some(cursor) => issued.push(cursor.to_string()),
                    None => break,
                }
                assert!(
                    issued.len() <= whole.len(),
                    "case {case:?}: paging did not terminate, tokens {issued:?}"
                );
            }
            assert_eq!(
                seen, whole,
                "case {case:?}: a walk must visit the whole listing exactly once, got {seen:?}"
            );
            // The rule under test: a token comes with elements still after it, so no
            // walk is ever handed the listing's own length (nor anything past it).
            for cursor in &issued {
                let position: usize = cursor
                    .parse()
                    .unwrap_or_else(|error| panic!("case {case:?}: token {cursor:?}: {error}"));
                assert!(
                    position < whole.len(),
                    "case {case:?}: every issued token must name an element of a \
                     {}-element listing, got {position}",
                    whole.len()
                );
            }
        }
    }
}

#[tokio::test]
async fn every_listing_answers_one_cursor_position_one_way() {
    let harness = seeded().await;

    for listing in Listing::ALL {
        let whole = listing.elements(&page_of(&harness, listing, "").await);
        let length = whole.len();

        // Every position that names an element is served, from the first to the last.
        for start in 0..length {
            let case = format!("{listing:?}: a cursor at position {start} of {length}");
            let answer = page_of(&harness, listing, &format!("nextToken={start}")).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                listing.elements(&answer),
                whole[start..].to_vec(),
                "case {case:?}: wrong page, got {:?}",
                listing.elements(&answer)
            );
            assert_eq!(
                next_token(&answer),
                None,
                "case {case:?}: a page that reaches the end carries no nextToken, got {:?}",
                next_token(&answer)
            );
        }

        // (label, the cursor, the sentence the refusal states)
        let refused: Vec<(String, String, String)> = vec![
            (
                "exactly the end of the listing".to_string(),
                length.to_string(),
                UNISSUED.to_string(),
            ),
            (
                "one position past the end".to_string(),
                (length + 1).to_string(),
                UNISSUED.to_string(),
            ),
            (
                "far past the end".to_string(),
                "9999".to_string(),
                UNISSUED.to_string(),
            ),
            (
                "a token that names no position".to_string(),
                "not_a_cursor".to_string(),
                format!("{UNISSUED}: not_a_cursor"),
            ),
            (
                "a token no position could be".to_string(),
                "-1".to_string(),
                format!("{UNISSUED}: -1"),
            ),
            (
                "a position written in another base".to_string(),
                "0x1".to_string(),
                format!("{UNISSUED}: 0x1"),
            ),
        ];

        for (label, cursor, sentence) in refused {
            let case = format!("{listing:?}: {label} (nextToken={cursor})");
            let answer = page_of(&harness, listing, &format!("nextToken={cursor}")).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected 400, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: wrong error code, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_message(),
                sentence,
                "case {case:?}: wrong refusal sentence, got {:?}",
                answer.error_message()
            );
        }

        // A refused cursor changed nothing a later caller sees.
        let again = listing.elements(&page_of(&harness, listing, "").await);
        assert_eq!(
            again, whole,
            "case {listing:?}: refusals must not change what a later listing reports, \
             got {again:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_listing_is_served_without_a_token_and_refuses_every_cursor() {
    for listing in Listing::ALL {
        let harness = emptied(listing).await;

        let answer = page_of(&harness, listing, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {listing:?} (empty): expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let elements = listing.elements(&answer);
        assert!(
            elements.is_empty(),
            "case {listing:?} (empty): expected no elements, got {elements:?}"
        );
        assert_eq!(
            next_token(&answer),
            None,
            "case {listing:?} (empty): an empty listing carries no nextToken, got {:?}",
            next_token(&answer)
        );

        // An empty listing has no position to continue from, so the service issued no
        // token for it — not even the first position.
        for cursor in ["0", "1"] {
            let case = format!("{listing:?} (empty): nextToken={cursor}");
            let refused = page_of(&harness, listing, &format!("nextToken={cursor}")).await;
            assert_eq!(
                refused.status,
                400,
                "case {case:?}: expected 400, got {} {}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: wrong error code, got {}",
                String::from_utf8_lossy(&refused.body)
            );
            assert_eq!(
                refused.error_message(),
                UNISSUED,
                "case {case:?}: wrong refusal sentence, got {:?}",
                refused.error_message()
            );
        }
    }
}
