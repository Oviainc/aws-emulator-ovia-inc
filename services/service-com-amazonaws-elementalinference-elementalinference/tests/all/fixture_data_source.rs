// SPDX-License-Identifier: CC0-1.0

//! The data source's own catalog: what `SearchFixtures` and `GetFixture` answer out of a
//! store nothing has seeded.
//!
//! Every wire case here drives the real request path — the router picks the service from
//! the SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: the fixtures pair reports the events the data source HOLDS, and
//! this emulator's data source holds a schedule around the present day, so a search of a
//! recent window for a sport the `DataSourceSport` enum declares reports fixture
//! summaries — a client that has never touched the state seam can obtain a `fixtureId`
//! and retrieve it. Which is what makes the pair usable at all: a `fixtureId` is only
//! ever learnt from a search, so a catalog holding nothing leaves `GetFixture` and a
//! clipping output's `dataSourceConfiguration` with nothing to name.
//!
//! What the catalog holds is the data source's, not the emulator's opinion of a request:
//! every row below varies the sport, the width and the placement of the window, and the
//! instant the catalog is built around, and each asserts the reported summaries against
//! the retrieved records rather than against any value a request carried. The span the
//! schedule covers is a BOUND, so a window outside it still selects nothing.

use crate::harness::{call, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    current_epoch_secs, default_fixture_catalog, fixture_date_string, DataSourceSport, Fixture,
    CATALOG_SPAN_DAYS, FIXTURES_PER_SPORT_PER_DAY, FIXTURE_STATUS_COMPLETED,
    FIXTURE_STATUS_SCHEDULED, MAX_SEARCH_WINDOW_DAYS,
};
use smithy_server_runtime::MultiServiceRouter;

/// Seconds in a UTC day: a UTC day has no offset changes, so an instant's day number is
/// its epoch-second count divided by this.
const SECONDS_PER_DAY: i64 = 86_400;

/// Today, as a day number since the Unix epoch.
fn today() -> i64 {
    current_epoch_secs().div_euclid(SECONDS_PER_DAY)
}

/// The `FixtureDate` text (`YYYY-MM-DD`) of a day number.
fn date(day: i64) -> String {
    fixture_date_string(day).unwrap_or_else(|| panic!("day {day} is a real UTC day"))
}

/// `SearchFixtures`: `POST /v1/fixtures`.
async fn search(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/v1/fixtures", Some(body)).await
}

/// `GetFixture`: `GET /v1/fixtures/{fixtureId}`.
async fn get_fixture(router: &MultiServiceRouter, fixture_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/fixtures/{fixture_id}"), None).await
}

/// A search body for `sport` over `start`..`end`, with the page size at the modelled
/// maximum so a window's whole selection comes back in one page.
fn window(sport: &str, start: i64, end: i64) -> Value {
    json!({
        "sport": sport,
        "startDate": date(start),
        "endDate": date(end),
        "maxResults": 100,
    })
}

/// The `fixtures` list of a 200 answer.
fn fixtures_of(answer: &Value) -> Vec<Value> {
    answer["fixtures"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("fixtures is a list on {answer}"))
}

/// A string member of a reported fixture.
fn text(fixture: &Value, member: &str) -> String {
    fixture[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} is a string on {fixture}"))
        .to_string()
}

/// One row of the window table: a sport and a recent window to search.
struct WindowCase {
    label: String,
    sport: &'static str,
    start: i64,
    end: i64,
}

/// A row per sport the enum declares, crossed with windows that differ in width (one
/// day, two days, the documented seven-day maximum) and in placement (ending today,
/// starting today, and wholly in the past and the future) — every shape of recent window
/// a caller can ask for.
fn window_cases() -> Vec<WindowCase> {
    let today = today();
    let mut cases = Vec::new();
    for sport in DataSourceSport::ALL.iter().copied() {
        for (shape, start, end) in [
            ("today alone", today, today),
            ("today and tomorrow", today, today + 1),
            (
                "the week up to today",
                today - (MAX_SEARCH_WINDOW_DAYS - 1),
                today,
            ),
            (
                "the week from today",
                today,
                today + MAX_SEARCH_WINDOW_DAYS - 1,
            ),
            ("a past day inside the span", today - 5, today - 4),
            ("a future day inside the span", today + 5, today + 6),
        ] {
            cases.push(WindowCase {
                label: format!("{} over {shape}", sport.as_wire()),
                sport: sport.as_wire(),
                start,
                end,
            });
        }
    }
    cases
}

/// A store nothing has seeded reports fixtures for every recent window, and `GetFixture`
/// agrees with each summary member for member.
#[tokio::test]
async fn a_fresh_store_reports_fixtures_a_client_can_then_retrieve() {
    let router = router();

    for case in window_cases() {
        let label = case.label;
        let body = window(case.sport, case.start, case.end);
        let (status, answer) = search(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the search answers 200, got {status} with {answer}"
        );
        let reported = fixtures_of(&answer);
        let days = (case.end - case.start + 1) as usize;
        assert_eq!(
            reported.len(),
            days * FIXTURES_PER_SPORT_PER_DAY,
            "case {label:?}: the window reports the schedule's fixtures for each of its \
             {days} days, got {answer}"
        );

        for fixture in &reported {
            let id = text(fixture, "fixtureId");
            // Every required member of a FixtureSummary is there and non-empty.
            assert!(
                !id.is_empty() && id.len() <= 128,
                "case {label:?}: the fixtureId satisfies the modelled length 1..128, got \
                 {fixture}"
            );
            assert!(
                !text(fixture, "name").is_empty(),
                "case {label:?}: the fixture carries a name, got {fixture}"
            );
            let status_text = text(fixture, "status");
            assert!(
                [FIXTURE_STATUS_SCHEDULED, FIXTURE_STATUS_COMPLETED].contains(&status_text.as_str()),
                "case {label:?}: the status is one the data source reports, got {fixture}"
            );
            let competitors = fixture["competitors"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                competitors.len(),
                2,
                "case {label:?}: the fixture reports its competitors, got {fixture}"
            );
            assert_eq!(
                competitors
                    .iter()
                    .filter(|competitor| competitor["isHome"] == json!(true))
                    .count(),
                1,
                "case {label:?}: exactly one competitor is the home side, got {fixture}"
            );

            // The retrieval leg: a client that only ever searched can fetch the id it was
            // given, and the record agrees with the summary.
            let (status, fetched) = get_fixture(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetFixture on a reported id answers 200, got {status} with \
                 {fetched}"
            );
            assert_eq!(
                fetched, *fixture,
                "case {label:?}: GetFixture agrees with the summary member for member, got \
                 {fetched}"
            );
        }

        // Reporting only: the same search answers the same thing afterwards.
        let (_, again) = search(&router, body).await;
        assert_eq!(
            again, answer,
            "case {label:?}: the reads changed nothing a later caller sees, got {again}"
        );
    }
}

/// The schedule is a BOUND: a window outside the span the data source covers selects
/// nothing, and no id from such a window exists.
#[tokio::test]
async fn a_window_outside_the_covered_span_selects_nothing() {
    let router = router();
    let today = today();
    // Each row: a window the schedule cannot cover — just past each edge of the span,
    // and decades away on both sides.
    for (label, start) in [
        ("the day after the covered span", today + CATALOG_SPAN_DAYS + 1),
        (
            "the day before the covered span",
            today - CATALOG_SPAN_DAYS - 1,
        ),
        ("a day decades in the past", today - 9_000),
        ("a day decades in the future", today + 9_000),
    ] {
        for sport in DataSourceSport::ALL.iter().copied() {
            let case = format!("{} on {label}", sport.as_wire());
            let (status, answer) = search(&router, window(sport.as_wire(), start, start)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the search answers 200, got {status} with {answer}"
            );
            assert_eq!(
                fixtures_of(&answer),
                Vec::<Value>::new(),
                "case {case:?}: a window the catalog holds no event in selects nothing, got \
                 {answer}"
            );
            assert_eq!(
                answer["nextToken"],
                Value::Null,
                "case {case:?}: and there is nothing to page, got {answer}"
            );
        }
    }
}

/// The schedule itself, over a table of instants it could be built at: the same instant
/// always yields the same catalog, every sport and every day of the span is covered, no
/// two fixtures share an id, and a fixture's status is decided by its own start.
#[test]
fn the_schedule_covers_every_day_of_the_span_for_every_sport() {
    // Each row: the instant the catalog is built at — the epoch itself, a leap day, a
    // year end, a mid-year instant and one far in the future — so no row depends on the
    // day the suite happens to run on.
    let instants: [(&str, i64); 5] = [
        ("the Unix epoch", 0),
        ("a leap day", 1_709_208_000),      // 2024-02-29T12:00:00Z
        ("a year end", 1_735_689_599),      // 2024-12-31T23:59:59Z
        ("a mid-year instant", 1_751_328_000), // 2025-07-01T00:00:00Z
        ("a day far ahead", 4_102_444_800),  // 2100-01-01T00:00:00Z
    ];

    for (label, now_secs) in instants {
        let catalog = default_fixture_catalog(now_secs);
        let center = now_secs.div_euclid(SECONDS_PER_DAY);
        let days = (2 * CATALOG_SPAN_DAYS + 1) as usize;
        assert_eq!(
            catalog.len(),
            days * FIXTURES_PER_SPORT_PER_DAY * DataSourceSport::ALL.len(),
            "case {label:?}: the schedule holds every day of the span for every sport, got \
             {} records",
            catalog.len()
        );

        let mut ids: Vec<&str> = catalog.iter().map(|fixture| fixture.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "case {label:?}: no two fixtures share a fixtureId"
        );

        for sport in DataSourceSport::ALL.iter().copied() {
            for day in (center - CATALOG_SPAN_DAYS)..=(center + CATALOG_SPAN_DAYS) {
                let held: Vec<&Fixture> = catalog
                    .iter()
                    .filter(|fixture| fixture.sport == sport && fixture.scheduled_day() == day)
                    .collect();
                assert_eq!(
                    held.len(),
                    FIXTURES_PER_SPORT_PER_DAY,
                    "case {label:?}: {} holds its fixtures on day {day}, got {held:?}",
                    sport.as_wire()
                );
                for fixture in held {
                    let expected_status = if fixture.scheduled_start_secs <= now_secs {
                        FIXTURE_STATUS_COMPLETED
                    } else {
                        FIXTURE_STATUS_SCHEDULED
                    };
                    assert_eq!(
                        fixture.status, expected_status,
                        "case {label:?}: a fixture's status follows its own start, got \
                         {fixture:?}"
                    );
                    let names: Vec<&str> = fixture
                        .competitors
                        .iter()
                        .map(|competitor| competitor.name.as_deref().unwrap_or_default())
                        .collect();
                    assert_eq!(
                        names.len(),
                        2,
                        "case {label:?}: a fixture has two competitors, got {fixture:?}"
                    );
                    assert_ne!(
                        names[0], names[1],
                        "case {label:?}: no team plays itself, got {fixture:?}"
                    );
                    assert!(
                        names
                            .iter()
                            .all(|name| fixture.name.contains(name) && !name.is_empty()),
                        "case {label:?}: the fixture's name is its two competitors, got \
                         {fixture:?}"
                    );
                }
            }
        }

        // Deterministic: the same instant builds the same catalog, so an id a client was
        // handed keeps naming the same event.
        let repeated = default_fixture_catalog(now_secs);
        let first: Vec<(&str, i64, &str)> = catalog
            .iter()
            .map(|fixture| {
                (
                    fixture.id.as_str(),
                    fixture.scheduled_start_secs,
                    fixture.name.as_str(),
                )
            })
            .collect();
        let second: Vec<(&str, i64, &str)> = repeated
            .iter()
            .map(|fixture| {
                (
                    fixture.id.as_str(),
                    fixture.scheduled_start_secs,
                    fixture.name.as_str(),
                )
            })
            .collect();
        assert_eq!(
            first, second,
            "case {label:?}: the schedule is derived, so it is identical on every build"
        );
    }
}
