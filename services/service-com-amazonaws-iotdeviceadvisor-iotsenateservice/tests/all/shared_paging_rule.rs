// SPDX-License-Identifier: CC0-1.0

//! The ONE paging rule both of this service's paginated listings answer by.
//!
//! `ListSuiteDefinitions` and `ListSuiteRuns` decide where a page starts and how long it
//! may be from the same decision (`crate::state::page_request`), so the cases below drive
//! BOTH listings with the same paging inputs and assert they answer alike: a token this
//! service could not have issued is the same refusal with the same message on either path,
//! an absent and an empty token are the same request on either path, and an absent
//! `maxResults` means the same page size on either path.
//!
//! The tables span what the rule generalises over rather than one request: token forms that
//! miss the id shape in a different way each (length, case, separator, group count,
//! wrapper, the modelled maximum length), and page sizes from the smallest the model admits
//! through the stored count to the largest, plus the absent one. The last case drives the
//! decision function directly, over `maxResults` values the wire-level boundary validator
//! refuses before a handler sees them, so the clamp that keeps a paging loop terminating is
//! covered too.

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{thing_arn, Emulator, ROLE_ARN};

/// The region every case below signs for.
const REGION: &str = "us-east-1";

/// One paginated listing of this service, as a client addresses it.
pub(crate) struct Listing {
    /// The request path the listing answers on.
    pub(crate) path: &'static str,
    /// The output member the page of entries is carried in.
    pub(crate) list: &'static str,
    /// The member each entry carries its id in — the same id a `nextToken` resumes at.
    pub(crate) id: &'static str,
}

/// Both listings the shared paging rule serves: the two operations that each used to carry
/// their own copy of it.
pub(crate) const LISTINGS: [Listing; 2] = [
    Listing {
        path: "/suiteDefinitions",
        list: "suiteDefinitionInformationList",
        id: "suiteDefinitionId",
    },
    Listing {
        path: "/suiteRuns",
        list: "suiteRunsList",
        id: "suiteRunId",
    },
];

/// How many entries each listing holds in the cases below: enough that every page size in
/// the table pages more than once and the last page is a partial one for some of them.
pub(crate) const SEEDED: usize = 4;

/// Create [`SEEDED`] suite definitions, each with one started suite run, so BOTH listings
/// hold the same number of entries.
///
/// Everything here goes over the wire: one definition per run, because the service
/// serialises the runs of a single definition.
pub(crate) async fn seed(emulator: &Emulator) {
    for index in 0..SEEDED {
        let created = emulator
            .create_definition(
                &json!({
                    "suiteDefinitionName": format!("paging-rule-suite-{index}"),
                    "rootGroup": "g",
                    "devicePermissionRoleArn": ROLE_ARN,
                    "devices": [{ "thingArn": thing_arn(index) }],
                }),
                REGION,
            )
            .await;
        assert_eq!(
            created.status, 200,
            "setup: creating definition {index} failed with {} {}",
            created.status, created.body
        );
        let definition = created.string("suiteDefinitionId");
        let started = emulator
            .call(
                "POST",
                &format!("/suiteDefinitions/{definition}/suiteRuns"),
                REGION,
                Some(json!({
                    "suiteRunConfiguration": {
                        "primaryDevice": { "thingArn": thing_arn(index) },
                    },
                })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "setup: starting a run of definition {index} failed with {} {}",
            started.status, started.body
        );
    }
}

/// The ids one answer of `listing` carries, in the order it listed them.
pub(crate) fn listed_ids(listing: &Listing, body: &Value) -> Vec<String> {
    body[listing.list]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry[listing.id]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("every entry carries a {}, got {entry}", listing.id)
                })
                .to_string()
        })
        .collect()
}

/// A token this service cannot have issued is refused by EVERY listing, with the same
/// status, the same error code and the same message.
#[tokio::test]
async fn every_listing_refuses_a_token_it_could_not_have_issued_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Each row misses the shape of an id this service mints in a different way: group
    // lengths, hex-digit case, the separator, the group count, a wrapper form a UUID parser
    // would accept, and a token at the modelled maximum `Token` length of 2000.
    let longest = "a".repeat(2000);
    let malformed: [(&str, &str); 8] = [
        ("a first group one digit short", "0000000-1111-2222-3333-444444444444"),
        ("a last group one digit long", "00000000-1111-2222-3333-4444444444444"),
        ("upper-case hex digits", "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"),
        ("underscores for separators", "00000000_1111_2222_3333_444444444444"),
        ("one group short", "00000000-1111-2222-444444444444"),
        ("the braced form", "%7B00000000-1111-2222-3333-444444444444%7D"),
        ("the urn form", "urn%3Auuid%3A00000000-1111-2222-3333-444444444444"),
        ("the longest token the model admits", longest.as_str()),
    ];

    for (case, token) in malformed {
        let mut answers: Vec<(u16, Option<String>, Option<String>)> = Vec::new();
        for listing in &LISTINGS {
            let refused = emulator
                .call(
                    "GET",
                    &format!("{}?nextToken={token}", listing.path),
                    REGION,
                    None,
                )
                .await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?} on {}: a token this service never issued must be a 400 \
                 ValidationException, got {} {}",
                listing.path,
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.member("message").as_str(),
                Some("Invalid nextToken"),
                "case {case:?} on {}: unexpected refusal message, got {}",
                listing.path,
                refused.body
            );
            answers.push((
                refused.status,
                refused.error_code.clone(),
                refused.member("message").as_str().map(ToString::to_string),
            ));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {case:?}: both listings must refuse the same token the same way, got \
             {answers:?}"
        );
    }
}

/// An absent and an empty `nextToken` are the SAME request — the first page — for every
/// listing, and a token a listing did issue resumes it where that page stopped.
#[tokio::test]
async fn every_listing_reads_an_absent_empty_or_issued_token_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for listing in &LISTINGS {
        let absent = emulator
            .call("GET", &format!("{}?maxResults=2", listing.path), REGION, None)
            .await;
        let empty = emulator
            .call(
                "GET",
                &format!("{}?maxResults=2&nextToken=", listing.path),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            (absent.status, empty.status),
            (200, 200),
            "case {:?}: an absent and an empty token are both accepted, got {} and {}",
            listing.path,
            absent.status,
            empty.status
        );
        assert_eq!(
            absent.body, empty.body,
            "case {:?}: an empty token carries no position, so it must answer exactly what \
             naming no token answers, got {} and {}",
            listing.path, absent.body, empty.body
        );

        let first = listed_ids(listing, &absent.body);
        let issued = absent
            .member("nextToken")
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a page shorter than the {SEEDED} stored entries carries a \
                     nextToken, got {}",
                    listing.path, absent.body
                )
            })
            .to_string();
        let resumed = emulator
            .call(
                "GET",
                &format!("{}?maxResults=2&nextToken={issued}", listing.path),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {:?}: a token this listing issued must be accepted, got {} {}",
            listing.path, resumed.status, resumed.body
        );
        let next = listed_ids(listing, &resumed.body);
        assert_eq!(
            next.first().map(String::as_str),
            Some(issued.as_str()),
            "case {:?}: the token is the id the next page resumes AT, got {next:?}",
            listing.path
        );
        assert!(
            next.iter().all(|id| !first.contains(id)),
            "case {:?}: the resumed page must not repeat the first page, got {first:?} then \
             {next:?}",
            listing.path
        );
    }
}

/// Every listing takes its page size from the same rule: at most `maxResults` entries per
/// page, an absent `maxResults` meaning one page holds everything stored.
#[tokio::test]
async fn every_listing_sizes_and_defaults_its_pages_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // `maxResults` is `range 1..50`: the smallest page the model admits, sizes around the
    // stored count, the largest page it admits, and the absent one.
    let page_sizes: [Option<usize>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(SEEDED),
        Some(SEEDED + 1),
        Some(usize::try_from(iotdeviceadvisor::state::MAX_RESULTS_MAX).expect("a positive bound")),
        None,
    ];

    for listing in &LISTINGS {
        for page_size in page_sizes {
            let case = format!("{} with maxResults {page_size:?}", listing.path);
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut query = listing.path.to_string();
                let mut separator = '?';
                if let Some(size) = page_size {
                    query.push_str(&format!("{separator}maxResults={size}"));
                    separator = '&';
                }
                if let Some(token) = &token {
                    query.push_str(&format!("{separator}nextToken={token}"));
                }
                let page = emulator.call("GET", &query, REGION, None).await;
                assert_eq!(
                    page.status, 200,
                    "case {case:?}: expected HTTP 200, got {} {}",
                    page.status, page.body
                );
                let ids = listed_ids(listing, &page.body);
                let bound = page_size.unwrap_or(SEEDED);
                assert!(
                    ids.len() <= bound,
                    "case {case:?}: a page may hold at most {bound} entries, got {}",
                    ids.len()
                );
                seen.extend(ids);
                pages += 1;
                assert!(
                    pages <= SEEDED + 1,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
                match page.member("nextToken").as_str() {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }

            let mut sorted = seen.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                seen.len(),
                "case {case:?}: paging listed an entry twice: {seen:?}"
            );
            assert_eq!(
                seen.len(),
                SEEDED,
                "case {case:?}: paging must yield every stored entry exactly once, got {seen:?}"
            );
            let expected_pages = SEEDED.div_ceil(page_size.unwrap_or(SEEDED).max(1));
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages, got {pages}"
            );
        }
    }
}

/// What the shared rule decides for one row of the table below.
enum Decided<'a> {
    /// Start the page at this position (`None` = at the beginning), holding at most this
    /// many entries.
    Page(Option<&'a str>, usize),
    /// The token names no position the listing holds, so no page of it ever ended there.
    Refused,
}

/// One row of the decision table: the paging members a request carries, and what the rule
/// makes of them.
struct Row<'a> {
    case: &'a str,
    next_token: Option<&'a str>,
    max_results: Option<i32>,
    expected: Decided<'a>,
}

/// The shared decision itself, over input classes including ones the wire-level boundary
/// validator refuses before any handler sees them.
#[test]
fn the_shared_paging_decision_maps_every_input_class() {
    // The positions the listing this decision is made for holds: the ids of its entries,
    // which are exactly the values it hands a `nextToken` out for.
    let held: Vec<String> = (0..3).map(|_| iotdeviceadvisor::state::new_uuid()).collect();
    let positions: Vec<&str> = held.iter().map(String::as_str).collect();
    let issued = held[1].clone();
    // A value with the very shape of an id this service mints, naming no entry the listing
    // holds — a forged, stale or cross-listing token.
    let unissued = iotdeviceadvisor::state::new_uuid();
    let max = usize::try_from(iotdeviceadvisor::state::MAX_RESULTS_MAX).expect("a positive bound");

    let rows: [Row; 9] = [
        Row {
            case: "no paging members at all",
            next_token: None,
            max_results: None,
            expected: Decided::Page(None, max),
        },
        Row {
            case: "an empty token",
            next_token: Some(""),
            max_results: None,
            expected: Decided::Page(None, max),
        },
        Row {
            case: "a token this listing could have issued",
            next_token: Some(issued.as_str()),
            max_results: None,
            expected: Decided::Page(Some(issued.as_str()), max),
        },
        Row {
            case: "an issued token with the smallest page",
            next_token: Some(issued.as_str()),
            max_results: Some(1),
            expected: Decided::Page(Some(issued.as_str()), 1),
        },
        Row {
            case: "the largest page the model admits",
            next_token: None,
            max_results: Some(iotdeviceadvisor::state::MAX_RESULTS_MAX),
            expected: Decided::Page(None, max),
        },
        // The two rows below the modelled `range 1..50` are unreachable over the wire, but
        // the rule still has to leave a page size a paging loop makes progress with.
        Row {
            case: "a page size of zero",
            next_token: None,
            max_results: Some(0),
            expected: Decided::Page(None, 1),
        },
        Row {
            case: "a negative page size",
            next_token: None,
            max_results: Some(-7),
            expected: Decided::Page(None, 1),
        },
        Row {
            case: "a token no listing could have issued",
            next_token: Some("not-a-token"),
            max_results: Some(2),
            expected: Decided::Refused,
        },
        Row {
            case: "an id-shaped token naming no position the listing holds",
            next_token: Some(unissued.as_str()),
            max_results: Some(2),
            expected: Decided::Refused,
        },
    ];

    for row in rows {
        let case = row.case;
        let decided = iotdeviceadvisor::state::page_request(
            row.next_token,
            row.max_results,
            positions.iter().copied(),
        );
        match (row.expected, &decided) {
            (Decided::Page(resume_at, page_size), Ok(actual)) => {
                assert_eq!(
                    (actual.resume_at.as_deref(), actual.page_size),
                    (resume_at, page_size),
                    "case {case:?}: unexpected paging decision, got {actual:?}"
                );
            }
            (Decided::Refused, Err(message)) => {
                assert!(
                    message.contains("nextToken"),
                    "case {case:?}: the refusal must name the member it refused, got \
                     {message:?}"
                );
            }
            _ => panic!("case {case:?}: unexpected paging decision {decided:?}"),
        }
    }
}
