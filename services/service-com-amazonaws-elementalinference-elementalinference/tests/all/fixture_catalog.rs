// SPDX-License-Identifier: CC0-1.0

//! The fixture catalog over the wire: `SearchFixtures` and `GetFixture`.
//!
//! Every case here drives the real request path — the router picks the service from
//! the SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * `SearchFixtures` reports the fixtures the data source holds for ONE sport on
//!   every UTC day of the window, and `GetFixture` reports the SAME record for a
//!   `fixtureId` the search handed back — member for member, including the status the
//!   fixture's own scheduled start puts it in;
//! * the window is `startDate` through `endDate`, `endDate` defaulting to
//!   `startDate`, and a window that runs backwards, exceeds seven days, or names
//!   something that is not a UTC calendar date is a `ValidationException` (HTTP 400);
//! * a search is paginated: at most `maxResults` fixtures per page, a `nextToken`
//!   exactly while more remain, and the pages of a window are its whole result set in
//!   order with nothing repeated;
//! * a fixture appears only when it satisfies EVERY filter, and a filter is satisfied
//!   by any one of its values matching any one of the fixture's competitors,
//!   regardless of case;
//! * a `fixtureId` no fixture has is `ValidationException` (HTTP 400) — the recorded
//!   refusal for an id that names no fixture — and so is one past the modelled
//!   length, which the boundary refuses before the handler runs.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    fixtures_on_day, parse_fixture_date, DataSourceSport, FIXTURE_IN_PROGRESS_SECS,
    FIXTURE_STATUS_COMPLETED, FIXTURE_STATUS_IN_PROGRESS, FIXTURE_STATUS_SCHEDULED,
    MAX_FIXTURES_PER_DAY, MAX_SEARCH_WINDOW_DAYS,
};
use service_com_amazonaws_elementalinference_elementalinference::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// This service's SigV4 signing name (`OPERATIONS.md`, "Wire protocol"): the router
/// resolves the claiming service from it.
const SIGNING_NAME: &str = "elemental-inference";

/// The sports a search may name: exactly the `DataSourceSport` enum
/// (`smithy_query(query="shape",shape="DataSourceSport")` — `basketball`,
/// `american-football`). [`sports_are_the_modelled_enum`] pins that.
const SPORTS: [&str; 2] = ["basketball", "american-football"];

/// The days the catalog is read on. They differ along what a `FixtureDate` can be —
/// a leap day, a year end, a day decades in the past, a day decades in the future —
/// and every one of them is far enough from any plausible run date that the
/// fixture statuses below cannot change while a case runs.
const DAYS: [&str; 5] = [
    "1999-12-31",
    "2012-06-15",
    "2024-02-29",
    "2088-02-29",
    "2099-07-04",
];

/// A router serving this service alone over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unsigned placeholder Authorization header, which is
    // there only so the router can resolve the signing name.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and return its status and parsed body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| serde_json::to_vec(&body).unwrap());
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .unwrap();
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {uri}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// `SearchFixtures`: `POST /v1/fixtures` (`OPERATIONS.md`).
async fn search(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/v1/fixtures", Some(body)).await
}

/// `GetFixture`: `GET /v1/fixtures/{fixtureId}` (`OPERATIONS.md`).
async fn get_fixture(router: &MultiServiceRouter, fixture_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/fixtures/{fixture_id}"), None).await
}

/// The fixtures of a response, as a list of JSON objects.
fn fixtures_of(response: &Value) -> Vec<Value> {
    response["fixtures"]
        .as_array()
        .unwrap_or_else(|| panic!("a search response carries a fixtures array, got {response}"))
        .clone()
}

/// A member of a fixture object, as text.
fn text(fixture: &Value, member: &str) -> String {
    fixture[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} is a string on {fixture}"))
        .to_string()
}

/// A fixture's scheduled start, in seconds since the epoch.
///
/// The model gives `scheduledStart` `@timestampFormat("date-time")`, but the
/// generated member schemas drop that trait today, so the framework renders it as the
/// `restJson1` default — epoch seconds. That gap is its own issue
/// (`20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas`); what
/// this suite asserts is the instant, whichever of the two renderings carries it.
fn start_secs(fixture: &Value) -> i64 {
    if let Some(secs) = fixture["scheduledStart"].as_i64() {
        return secs;
    }
    let rendered = fixture["scheduledStart"]
        .as_str()
        .unwrap_or_else(|| panic!("scheduledStart is an instant on {fixture}"));
    DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|e| panic!("{rendered:?} is an RFC-3339 instant: {e}"))
        .secs()
}

/// The UTC day (`YYYY-MM-DD`) an epoch-second count falls on.
fn utc_day(secs: i64) -> String {
    let rendered = DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .unwrap_or_else(|e| panic!("{secs} is an instant: {e}"));
    rendered[..10].to_string()
}

/// Seconds since the epoch now, as the service measures a fixture's status against.
fn now_secs() -> i64 {
    DateTime::from(std::time::SystemTime::now()).secs()
}

/// The status rule: a fixture is `Scheduled` until its scheduled start, `InProgress`
/// for [`FIXTURE_IN_PROGRESS_SECS`] after it, and `Completed` from then on.
fn expected_status(scheduled_start_secs: i64, now: i64) -> &'static str {
    if now < scheduled_start_secs {
        FIXTURE_STATUS_SCHEDULED
    } else if now < scheduled_start_secs + FIXTURE_IN_PROGRESS_SECS {
        FIXTURE_STATUS_IN_PROGRESS
    } else {
        FIXTURE_STATUS_COMPLETED
    }
}

/// The table's sports are exactly the values the model declares, so every case below
/// is a case the service must accept.
#[test]
fn sports_are_the_modelled_enum() {
    for sport in SPORTS {
        assert!(
            DataSourceSport::from_wire(sport).is_some(),
            "case {sport:?}: the table only names DataSourceSport values"
        );
    }
    assert!(
        DataSourceSport::from_wire("cricket").is_none(),
        "case \"cricket\": a value the enum does not declare is not a sport"
    );
}

/// A search reports the day's fixtures, and `GetFixture` reports the same record for
/// each of their ids.
#[tokio::test]
async fn search_reports_the_day_and_get_fixture_agrees_with_it() {
    let router = router();

    for sport in SPORTS {
        for day in DAYS {
            let case = format!("{sport} on {day}");
            let (status, found) = search(&router, json!({"sport": sport, "startDate": day})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: SearchFixtures answers 200, got {status} with {found}"
            );
            assert_eq!(
                found["nextToken"],
                Value::Null,
                "case {case:?}: one day fits in one page, so there is no nextToken: {found}"
            );

            let fixtures = fixtures_of(&found);
            assert!(
                !fixtures.is_empty() && fixtures.len() <= MAX_FIXTURES_PER_DAY,
                "case {case:?}: the data source schedules 1..={MAX_FIXTURES_PER_DAY} fixtures a day, got {}",
                fixtures.len()
            );

            let mut ids = BTreeSet::new();
            let mut previous: Option<(String, i64)> = None;
            for fixture in &fixtures {
                let id = text(fixture, "fixtureId");
                assert!(
                    ids.insert(id.clone()),
                    "case {case:?}: every fixtureId of a day is distinct, {id:?} repeated"
                );
                // The id names the catalog entry: sport, UTC day, then the index.
                let prefix = format!("{sport}-{}-", day.replace('-', ""));
                assert!(
                    id.starts_with(&prefix) && id.len() == prefix.len() + 2,
                    "case {case:?}: the fixtureId is {prefix}NN, got {id:?}"
                );
                assert!(
                    id.len() <= 128,
                    "case {case:?}: a fixtureId is at most 128 characters (modelled length), got {}",
                    id.len()
                );

                // The scheduled start is on the day the search asked for.
                let start = start_secs(fixture);
                assert_eq!(
                    utc_day(start),
                    day,
                    "case {case:?}: the fixture is scheduled on {day}, got {start} ({})",
                    utc_day(start)
                );

                // Two competitors, exactly one of them at home.
                let competitors = fixture["competitors"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: competitors is a list: {fixture}"))
                    .clone();
                assert_eq!(
                    competitors.len(),
                    2,
                    "case {case:?}: a fixture has two competitors, got {competitors:?}"
                );
                let home: Vec<&Value> = competitors
                    .iter()
                    .filter(|competitor| competitor["isHome"] == json!(true))
                    .collect();
                assert_eq!(
                    home.len(),
                    1,
                    "case {case:?}: exactly one competitor is at home, got {competitors:?}"
                );
                let names: BTreeSet<String> = competitors
                    .iter()
                    .map(|competitor| text(competitor, "name"))
                    .collect();
                assert_eq!(
                    names.len(),
                    2,
                    "case {case:?}: no competitor plays itself, got {competitors:?}"
                );

                // The status follows the fixture's own scheduled start.
                let now = now_secs();
                assert_eq!(
                    text(fixture, "status"),
                    expected_status(start, now),
                    "case {case:?}: the status follows the scheduled start {start}, got {fixture}"
                );

                // One order, ascending by id and by scheduled start together: that is
                // the order a `nextToken` names a position in.
                if let Some((earlier_id, earlier_start)) = &previous {
                    assert!(
                        earlier_id.as_str() < id.as_str() && *earlier_start < start,
                        "case {case:?}: fixtures come back in ascending id and start order, got {earlier_id} at {earlier_start} before {id} at {start}"
                    );
                }
                previous = Some((id.clone(), start));

                // And the round trip: GetFixture reports this very record.
                let (status, fetched) = get_fixture(&router, &id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetFixture on {id:?} answers 200, got {status} with {fetched}"
                );
                assert_eq!(
                    &fetched, fixture,
                    "case {case:?}: GetFixture on {id:?} reports the searched fixture member for member, got {fetched}"
                );
            }
        }
    }
}

/// The search window: `endDate` defaults to `startDate`, spans up to seven days, and
/// anything else is refused.
#[tokio::test]
async fn the_window_spans_start_date_through_end_date() {
    let router = router();

    // A window of one day through seven, each covering exactly its own days.
    for span in 1..=MAX_SEARCH_WINDOW_DAYS {
        let start = "2026-03-14";
        // 2026-03-14 .. 2026-03-20: the whole span stays inside one month.
        let end = format!("2026-03-{:02}", 13 + span);
        let case = format!("{start} through {end} ({span} days)");
        let (status, found) = search(
            &router,
            json!({"sport": "basketball", "startDate": start, "endDate": end, "maxResults": 100}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a window of at most {MAX_SEARCH_WINDOW_DAYS} days answers 200, got {status} with {found}"
        );
        let days: BTreeSet<String> = fixtures_of(&found)
            .iter()
            .map(|fixture| utc_day(start_secs(fixture)))
            .collect();
        let expected: BTreeSet<String> = (0..span)
            .map(|offset| format!("2026-03-{:02}", 14 + offset))
            .collect();
        assert_eq!(
            days, expected,
            "case {case:?}: the search covers every day of the window and no other, got {days:?}"
        );
    }

    // An omitted endDate searches only startDate's day.
    let (status, only_start) =
        search(&router, json!({"sport": "basketball", "startDate": "2026-03-14"})).await;
    assert_eq!(
        status, 200,
        "case \"endDate omitted\": answers 200, got {status} with {only_start}"
    );
    let (_, same_day) = search(
        &router,
        json!({"sport": "basketball", "startDate": "2026-03-14", "endDate": "2026-03-14"}),
    )
    .await;
    assert_eq!(
        only_start, same_day,
        "case \"endDate omitted\": that is the same search as endDate == startDate, got {only_start} and {same_day}"
    );

    // A window one day past the published bound, a backwards window, and dates the
    // modelled pattern admits but the calendar does not.
    let refused = [
        ("eight days", json!({"sport": "basketball", "startDate": "2026-03-14", "endDate": "2026-03-21"})),
        ("backwards", json!({"sport": "basketball", "startDate": "2026-03-14", "endDate": "2026-03-13"})),
        ("31 February", json!({"sport": "basketball", "startDate": "2026-02-31"})),
        ("29 February in a common year", json!({"sport": "basketball", "startDate": "2025-02-29"})),
        ("month 13", json!({"sport": "basketball", "startDate": "2026-13-01"})),
        ("endDate not a date", json!({"sport": "basketball", "startDate": "2026-03-14", "endDate": "2026-04-31"})),
        ("not YYYY-MM-DD", json!({"sport": "basketball", "startDate": "2026-3-14"})),
        ("not a sport", json!({"sport": "cricket", "startDate": "2026-03-14"})),
        ("maxResults below the modelled range", json!({"sport": "basketball", "startDate": "2026-03-14", "maxResults": 0})),
        ("maxResults above the modelled range", json!({"sport": "basketball", "startDate": "2026-03-14", "maxResults": 101})),
        ("an unmodelled filter name", json!({"sport": "basketball", "startDate": "2026-03-14", "filters": [{"name": "LEAGUE", "values": ["x"]}]})),
    ];
    for (case, body) in refused {
        let (status, error) = search(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {body} is refused with 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {case:?}: that 400 is a ValidationException, got {error}"
        );
    }

    // 29 February in a leap year is a day, and the bound itself is accepted above.
    let (status, leap) =
        search(&router, json!({"sport": "basketball", "startDate": "2024-02-29"})).await;
    assert_eq!(
        status, 200,
        "case \"29 February in a leap year\": answers 200, got {status} with {leap}"
    );
    assert!(
        !fixtures_of(&leap).is_empty(),
        "case \"29 February in a leap year\": the day has fixtures, got {leap}"
    );
}

/// Paging a window: every page is at most `maxResults` long, a `nextToken` comes back
/// exactly while more remain, and the pages are the whole result set, in order.
#[tokio::test]
async fn pages_of_a_window_are_its_whole_result_set() {
    let router = router();
    let window = json!({"sport": "american-football", "startDate": "2026-05-01", "endDate": "2026-05-07"});

    let mut unpaged = window.clone();
    unpaged["maxResults"] = json!(100);
    let (status, all) = search(&router, unpaged).await;
    assert_eq!(
        status, 200,
        "case \"one page\": answers 200, got {status} with {all}"
    );
    let expected: Vec<String> = fixtures_of(&all)
        .iter()
        .map(|fixture| text(fixture, "fixtureId"))
        .collect();
    assert!(
        expected.len() > 1,
        "case \"one page\": a seven-day window has more than one fixture, got {expected:?}"
    );

    for page_size in [1_usize, 2, 3, expected.len()] {
        let case = format!("maxResults {page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        // The window is bounded, so paging it can never need more requests than it
        // has fixtures plus one.
        for _ in 0..=expected.len() {
            let mut body = window.clone();
            body["maxResults"] = json!(page_size);
            if let Some(token) = &next_token {
                body["nextToken"] = json!(token);
            }
            let (status, page) = search(&router, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a page answers 200, got {status} with {page}"
            );
            let page_ids: Vec<String> = fixtures_of(&page)
                .iter()
                .map(|fixture| text(fixture, "fixtureId"))
                .collect();
            assert!(
                page_ids.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} fixtures, got {page_ids:?}"
            );
            collected.extend(page_ids);
            next_token = page["nextToken"].as_str().map(str::to_string);
            let remaining = expected.len() - collected.len();
            assert_eq!(
                next_token.is_some(),
                remaining > 0,
                "case {case:?}: a nextToken comes back exactly while fixtures remain ({remaining} left), got {page}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: the pages are the window's whole result set, in order"
        );
    }

    // A token that is not a pagination token at all is refused rather than read as
    // the first page.
    for token in ["not-hex", "zzzz", ""] {
        let mut body = window.clone();
        body["nextToken"] = json!(token);
        let (status, error) = search(&router, body).await;
        assert_eq!(
            status, 400,
            "case {token:?}: a malformed nextToken is a 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {token:?}: that 400 is a ValidationException, got {error}"
        );
    }
}

/// Filters: a fixture appears only when it satisfies every filter, and one filter is
/// satisfied by any of its values matching any competitor, whatever the case.
#[tokio::test]
async fn filters_narrow_the_results_to_matching_competitors() {
    let router = router();
    let day = json!({"sport": "basketball", "startDate": "2026-06-20", "maxResults": 100});

    let (_, all) = search(&router, day.clone()).await;
    let fixtures = fixtures_of(&all);
    let first = fixtures
        .first()
        .unwrap_or_else(|| panic!("the day has fixtures: {all}"));
    let wanted_id = text(first, "fixtureId");
    let home = text(&first["competitors"][0], "name");
    let word = home
        .split_whitespace()
        .next_back()
        .expect("a competitor name has a word in it")
        .to_string();

    // Each row: the filters, and whether the fixture above must appear.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "the competitor's own name",
            json!([{"name": "COMPETITOR", "values": [home.clone()]}]),
            true,
        ),
        (
            "that name in lower case",
            json!([{"name": "COMPETITOR", "values": [home.to_lowercase()]}]),
            true,
        ),
        (
            "that name in upper case",
            json!([{"name": "COMPETITOR", "values": [home.to_uppercase()]}]),
            true,
        ),
        (
            "part of that name",
            json!([{"name": "COMPETITOR", "values": [word.clone()]}]),
            true,
        ),
        (
            "one matching value among several",
            json!([{"name": "COMPETITOR", "values": ["Nowhere Nobodies", home.clone()]}]),
            true,
        ),
        (
            "a competitor nobody is",
            json!([{"name": "COMPETITOR", "values": ["Nowhere Nobodies"]}]),
            false,
        ),
        (
            "no values at all",
            json!([{"name": "COMPETITOR", "values": []}]),
            false,
        ),
        (
            "two filters, one of them unsatisfiable",
            json!([
                {"name": "COMPETITOR", "values": [home.clone()]},
                {"name": "COMPETITOR", "values": ["Nowhere Nobodies"]}
            ]),
            false,
        ),
    ];

    for (case, filters, must_appear) in cases {
        let mut body = day.clone();
        body["filters"] = filters.clone();
        let (status, found) = search(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a filtered search answers 200, got {status} with {found}"
        );
        let matched = fixtures_of(&found);
        assert_eq!(
            matched.iter().any(|fixture| text(fixture, "fixtureId") == wanted_id),
            must_appear,
            "case {case:?}: {filters} must{} match {wanted_id:?}, got {found}",
            if must_appear { "" } else { " not" }
        );
        if !must_appear {
            assert!(
                matched.is_empty(),
                "case {case:?}: no fixture satisfies {filters}, got {found}"
            );
        }
        // Whatever came back satisfies every filter, not merely the one asked about.
        for fixture in &matched {
            let names = fixture["competitors"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|competitor| text(competitor, "name").to_lowercase())
                .collect::<Vec<String>>();
            for filter in filters.as_array().cloned().unwrap_or_default() {
                let values = filter["values"].as_array().cloned().unwrap_or_default();
                assert!(
                    values.iter().any(|value| {
                        let value = value.as_str().unwrap_or_default().to_lowercase();
                        names.iter().any(|name| name.contains(&value))
                    }),
                    "case {case:?}: {fixture} satisfies the filter {filter}"
                );
            }
        }
    }
}

/// A `fixtureId` no fixture has, and one past the modelled length.
///
/// Both are HTTP 400 `ValidationException`: the recorded behaviour of `GetFixture` for
/// an id that names no fixture is a validation refusal (corpus unit
/// `elementalinference.get-fixture-not-found`), because a `fixtureId` is an opaque
/// reference the service itself hands out, and an id past the modelled `FixtureId`
/// length never reaches the handler at all — the boundary refuses it with the same
/// status and code.
#[tokio::test]
async fn get_fixture_refuses_an_id_no_fixture_has() {
    let router = router();

    // The catalogued id this table takes its near misses from.
    let (status, found) =
        search(&router, json!({"sport": "basketball", "startDate": "2026-03-14"})).await;
    assert_eq!(status, 200, "a search answers 200, got {found}");
    let catalogued = text(&fixtures_of(&found)[0], "fixtureId");

    // Each row: an id no fixture has, differing in form, length and in how near it is
    // to an id the catalog does have.
    let mut cases: Vec<(String, String)> = vec![
        (
            "an index the day does not have".to_string(),
            "basketball-20260314-99".to_string(),
        ),
        ("index zero".to_string(), "basketball-20260314-00".to_string()),
        (
            "a day that is not a date".to_string(),
            "basketball-20260230-01".to_string(),
        ),
        (
            "a sport the model does not declare".to_string(),
            "cricket-20260314-01".to_string(),
        ),
        (
            "a one-digit index".to_string(),
            "basketball-20260314-1".to_string(),
        ),
        ("a short day".to_string(), "basketball-2026031-01".to_string()),
        ("no structure at all".to_string(), "nonsense".to_string()),
        ("a single character".to_string(), "x".to_string()),
        ("digits only".to_string(), "12345".to_string()),
        ("the modelled maximum length".to_string(), "a".repeat(128)),
        (
            "one past the modelled maximum length".to_string(),
            "a".repeat(129),
        ),
        (
            "a catalogued id in upper case".to_string(),
            catalogued.to_uppercase(),
        ),
        (
            "a catalogued id with a suffix".to_string(),
            format!("{catalogued}0"),
        ),
        (
            "a catalogued id with its last character dropped".to_string(),
            catalogued[..catalogued.len() - 1].to_string(),
        ),
    ];
    // And a row per fixture the day's card does hold, each a near miss on its index,
    // so the refusal is not keyed to one shape of id.
    for fixture in fixtures_of(&found) {
        cases.push((
            format!("a near miss on {}", text(&fixture, "fixtureId")),
            format!("{}9", text(&fixture, "fixtureId")),
        ));
    }

    for (case, id) in cases {
        let (status, error) = get_fixture(&router, &id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetFixture on {id:?} answers 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {case:?}: that 400 is a ValidationException, got {error}"
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap_or_default()
                .contains("fixtureId"),
            "case {case:?}: the refusal names the member at fault, got {error}"
        );
    }

    // A fixture that IS in the catalog is unaffected by the refusals above.
    let (status, fetched) = get_fixture(&router, &catalogued).await;
    assert_eq!(
        status, 200,
        "case {catalogued:?}: a catalogued fixture still answers 200, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["fixtureId"],
        json!(catalogued),
        "case {catalogued:?}: GetFixture reports the id that was asked for, got {fetched}"
    );
}

/// The status rule at each of its boundaries, over every fixture of the table's days:
/// `Scheduled` until the scheduled start, `InProgress` for
/// [`FIXTURE_IN_PROGRESS_SECS`] from it, `Completed` after that. The instants are
/// chosen rather than waited for, which is why this covers the in-progress span that
/// no fixed day can be relied on to be in.
#[test]
fn the_status_rule_holds_at_each_of_its_boundaries() {
    for sport in SPORTS {
        let catalog_sport =
            DataSourceSport::from_wire(sport).expect("the table only names modelled sports");
        for day in DAYS {
            let epoch_day = parse_fixture_date(day)
                .unwrap_or_else(|| panic!("case {day:?}: the table only names real dates"));
            let fixtures = fixtures_on_day(catalog_sport, epoch_day);
            assert!(
                !fixtures.is_empty(),
                "case {sport} on {day}: the catalog holds fixtures for every day"
            );
            for fixture in fixtures {
                let start = fixture.scheduled_start_secs;
                let rows = [
                    ("the instant before the start", start - 1, FIXTURE_STATUS_SCHEDULED),
                    ("the start itself", start, FIXTURE_STATUS_IN_PROGRESS),
                    (
                        "the last instant under way",
                        start + FIXTURE_IN_PROGRESS_SECS - 1,
                        FIXTURE_STATUS_IN_PROGRESS,
                    ),
                    (
                        "the instant it is over",
                        start + FIXTURE_IN_PROGRESS_SECS,
                        FIXTURE_STATUS_COMPLETED,
                    ),
                ];
                for (case, at, expected) in rows {
                    assert_eq!(
                        fixture.status_at(at),
                        expected,
                        "case {:?} at {case:?}: expected {expected}, got {}",
                        fixture.id,
                        fixture.status_at(at)
                    );
                }
            }
        }
    }
}
