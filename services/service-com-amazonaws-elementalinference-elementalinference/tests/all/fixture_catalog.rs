// SPDX-License-Identifier: CC0-1.0

//! The fixture catalog over the wire: `SearchFixtures` and `GetFixture`.
//!
//! Every case here drives the real request path — the router picks the service from
//! the SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The RULE under test: a search reports exactly the fixtures the data-source catalog
//! HOLDS that satisfy the request — the required `sport`, the `startDate` through
//! `endDate` window of UTC days a fixture is scheduled on, and every `filters` entry —
//! and nothing else, so a window the catalog holds no event in answers with an empty
//! list ("The array is empty if no fixtures match", `SearchFixturesResponse$fixtures`);
//! `GetFixture` answers for a held id and refuses any other.
//!
//! The catalog is STATE: no modelled operation creates a fixture (AWS relays them from
//! an external sports data source), so a store comes up holding this emulator's own
//! schedule (`state::default_fixture_catalog`, the days around the present one) and the
//! state seam
//! (`ComAmazonawsElementalinferenceElementalinferenceState::seed_fixtures` /
//! `clear_fixtures`) is what installs or drops records. Every suite below EMPTIES the
//! catalog first and installs exactly the rows it asserts against, so what it asserts
//! is the selection rule and nothing else; that the store comes up holding a schedule at
//! all is `fixture_data_source.rs`. The ids below are
//! deliberately opaque and sort in a different order from the fixtures' scheduled
//! starts, so no case can pass by deriving a fixture from the shape of an id.

use crate::harness::{call, router_with_engine};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::ops::types::Competitor;
use service_com_amazonaws_elementalinference_elementalinference::state::{
    fixture_start_secs, parse_fixture_date,
    ComAmazonawsElementalinferenceElementalinferenceState, DataSourceSport, Fixture,
    FIXTURE_STATUS_COMPLETED, FIXTURE_STATUS_SCHEDULED, MAX_SEARCH_WINDOW_DAYS,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// One fixture to install in the catalog: a row of the data source's schedule.
struct SeedRow {
    /// The `FixtureId` the fixture is held under (modelled length 1..128).
    id: &'static str,
    sport: DataSourceSport,
    /// The UTC day the fixture is scheduled on, as a `FixtureDate` (`YYYY-MM-DD`).
    day: &'static str,
    /// The kick-off time on that day, in UTC.
    hour: i64,
    minute: i64,
    name: &'static str,
    /// `None` for a fixture the data source gives no group for.
    group: Option<&'static str>,
    status: &'static str,
    /// The competitors, each with its name and whether it is the home side.
    competitors: &'static [(&'static str, bool)],
}

impl SeedRow {
    /// The day number this row's `day` names.
    fn epoch_day(&self) -> i64 {
        parse_fixture_date(self.day)
            .unwrap_or_else(|| panic!("row {:?}: {:?} is a real UTC day", self.id, self.day))
    }

    /// The instant this row's fixture starts at, in seconds since the epoch.
    fn start_secs(&self) -> i64 {
        fixture_start_secs(self.epoch_day(), self.hour, self.minute)
            .unwrap_or_else(|| panic!("row {:?}: the start instant is representable", self.id))
    }

    /// The stored record this row installs.
    fn to_fixture(&self) -> Fixture {
        Fixture {
            id: self.id.to_string(),
            sport: self.sport,
            name: self.name.to_string(),
            group: self.group.map(str::to_string),
            scheduled_start_secs: self.start_secs(),
            status: self.status.to_string(),
            competitors: self
                .competitors
                .iter()
                .map(|(name, is_home)| Competitor {
                    name: Some((*name).to_string()),
                    is_home: Some(*is_home),
                })
                .collect(),
        }
    }
}

/// The catalog these suites install.
///
/// The rows differ along everything the rule generalises over: both sports the
/// `DataSourceSport` enum declares, days decades apart (a leap day, a year end, a day
/// in the past and one in the future), days holding one, two or no fixture of a sport,
/// kick-offs at both ends of a UTC day (midnight and the last minute), a group present
/// and absent, two and four competitors, and statuses both inside and outside the two
/// the model's documentation names ("For example, Scheduled or Completed",
/// `FixtureSummary$status`) — the member is a free-form string, so a held value is
/// reported verbatim.
fn seed_rows() -> Vec<SeedRow> {
    vec![
        SeedRow {
            id: "alpha-opener",
            sport: DataSourceSport::Basketball,
            day: "2024-02-29",
            hour: 19,
            minute: 30,
            name: "Alpha Antelopes at Omega Owls",
            group: None,
            status: FIXTURE_STATUS_COMPLETED,
            competitors: &[("Omega Owls", true), ("Alpha Antelopes", false)],
        },
        SeedRow {
            id: "zeta-nightcap",
            sport: DataSourceSport::Basketball,
            day: "2024-02-29",
            hour: 17,
            minute: 0,
            name: "Beacon Bears at Cobalt Comets",
            group: Some("Leap Day Invitational"),
            status: "Postponed",
            competitors: &[("Cobalt Comets", true), ("Beacon Bears", false)],
        },
        SeedRow {
            id: "mid-day-start",
            sport: DataSourceSport::Basketball,
            day: "2024-03-01",
            hour: 0,
            minute: 0,
            name: "Delta Dolphins at Echo Eagles",
            group: Some("Spring Series"),
            status: FIXTURE_STATUS_SCHEDULED,
            competitors: &[("Echo Eagles", true), ("Delta Dolphins", false)],
        },
        SeedRow {
            id: "night-owl",
            sport: DataSourceSport::Basketball,
            day: "2024-03-01",
            hour: 23,
            minute: 59,
            name: "Echo Eagles at Beacon Bears",
            group: None,
            status: FIXTURE_STATUS_SCHEDULED,
            competitors: &[
                ("Beacon Bears", true),
                ("Echo Eagles", false),
                ("Cobalt Comets", false),
                ("Delta Dolphins", false),
            ],
        },
        SeedRow {
            id: "gridiron-kickoff",
            sport: DataSourceSport::AmericanFootball,
            day: "2024-02-29",
            hour: 18,
            minute: 0,
            name: "Harbor Hornets at Granite Gladiators",
            group: Some("Gridiron Conference"),
            status: FIXTURE_STATUS_COMPLETED,
            competitors: &[("Granite Gladiators", true), ("Harbor Hornets", false)],
        },
        SeedRow {
            id: "gridiron-finale",
            sport: DataSourceSport::AmericanFootball,
            day: "2024-03-05",
            hour: 20,
            minute: 15,
            name: "Iron Ibises at Juno Jackals",
            group: None,
            status: FIXTURE_STATUS_SCHEDULED,
            competitors: &[("Juno Jackals", true), ("Iron Ibises", false)],
        },
        SeedRow {
            id: "legacy-millennium",
            sport: DataSourceSport::Basketball,
            day: "1999-12-31",
            hour: 23,
            minute: 0,
            name: "Cobalt Comets at Omega Owls",
            group: Some("Millennium Classic"),
            status: FIXTURE_STATUS_COMPLETED,
            competitors: &[("Omega Owls", true), ("Cobalt Comets", false)],
        },
        SeedRow {
            id: "future-independence",
            sport: DataSourceSport::AmericanFootball,
            day: "2099-07-04",
            hour: 12,
            minute: 0,
            name: "Juno Jackals at Granite Gladiators",
            group: None,
            status: FIXTURE_STATUS_SCHEDULED,
            competitors: &[
                ("Granite Gladiators", true),
                ("Juno Jackals", false),
                ("Iron Ibises", false),
            ],
        },
    ]
}

/// The row with this id.
fn row_of(id: &str) -> SeedRow {
    seed_rows()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("the seed table holds a row for {id:?}"))
}

/// A router over a catalog holding exactly [`seed_rows`].
fn seeded_router() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    clear(&engine);
    seed(&engine, seed_rows());
    router
}

/// Empty the catalog of the schedule a store comes up with, so a suite asserts against
/// exactly the rows it installs.
fn clear(engine: &Arc<engine::InMemoryEngine>) {
    engine
        .state::<ComAmazonawsElementalinferenceElementalinferenceState>()
        .lock()
        .clear_fixtures();
}

/// Install `rows` through the state seam — the only authoring path a fixture has.
/// ADDITIVE, as the seam is: a held id is replaced, everything else is left alone.
fn seed(engine: &Arc<engine::InMemoryEngine>, rows: Vec<SeedRow>) {
    let state = engine.state::<ComAmazonawsElementalinferenceElementalinferenceState>();
    state
        .lock()
        .seed_fixtures(rows.iter().map(SeedRow::to_fixture));
}

/// `SearchFixtures`: `POST /v1/fixtures` (`OPERATIONS.md`).
async fn search(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/v1/fixtures", Some(body)).await
}

/// `GetFixture`: `GET /v1/fixtures/{fixtureId}` (`OPERATIONS.md`).
async fn get_fixture(router: &MultiServiceRouter, fixture_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/fixtures/{fixture_id}"), None).await
}

/// A search body for one window, with the page size at the modelled maximum so a
/// selection is never split unless the case is about paging.
fn window_body(sport: &str, start: &str, end: Option<&str>) -> Value {
    let mut body = json!({"sport": sport, "startDate": start, "maxResults": 100});
    if let Some(end) = end {
        body["endDate"] = json!(end);
    }
    body
}

/// The `fixtures` list of a 200 answer.
fn fixtures_of(answer: &Value) -> Vec<Value> {
    answer["fixtures"]
        .as_array()
        .unwrap_or_else(|| panic!("fixtures is a list on {answer}"))
        .clone()
}

/// The ids of the `fixtures` list, in the order the answer carries them.
fn ids_of(answer: &Value) -> Vec<String> {
    fixtures_of(answer)
        .iter()
        .map(|fixture| text(fixture, "fixtureId"))
        .collect()
}

/// A required string member of a fixture.
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
    aws_smithy_types::DateTime::from_str(rendered, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|e| panic!("{rendered:?} is an RFC-3339 instant: {e}"))
        .secs()
}

/// Assert that `fixture` is the projection of the seeded row `id`, member for member.
fn assert_is_row(case: &str, fixture: &Value, id: &str) {
    let row = row_of(id);
    assert_eq!(
        text(fixture, "fixtureId"),
        row.id,
        "case {case:?}: the fixtureId is the held id, got {fixture}"
    );
    assert_eq!(
        text(fixture, "name"),
        row.name,
        "case {case:?}: the name is the held name, got {fixture}"
    );
    assert_eq!(
        text(fixture, "status"),
        row.status,
        "case {case:?}: the status is the held status, got {fixture}"
    );
    match row.group {
        Some(group) => assert_eq!(
            fixture["fixtureGroup"],
            json!(group),
            "case {case:?}: the fixtureGroup is the held group, got {fixture}"
        ),
        None => assert_eq!(
            fixture["fixtureGroup"],
            Value::Null,
            "case {case:?}: a fixture the data source gives no group for carries none, got {fixture}"
        ),
    }
    assert_eq!(
        start_secs(fixture),
        row.start_secs(),
        "case {case:?}: the scheduledStart is the held instant, got {fixture}"
    );
    let expected_competitors: Vec<Value> = row
        .competitors
        .iter()
        .map(|(name, is_home)| json!({"name": name, "isHome": is_home}))
        .collect();
    assert_eq!(
        fixture["competitors"],
        Value::Array(expected_competitors),
        "case {case:?}: the competitors are the held competitors, in the held order, got {fixture}"
    );
}

/// One window case: the request, and the ids the catalog selects for it.
struct WindowCase {
    label: &'static str,
    sport: &'static str,
    start: &'static str,
    end: Option<&'static str>,
    /// The held fixtures this window selects, in `FixtureId` order.
    expected: &'static [&'static str],
}

/// Windows that differ along what the rule generalises over: the sport, the number of
/// days (one, two, and the documented seven-day bound), whether the selected days hold
/// one, several or no event, the day either side of a held event, and days decades
/// apart. The expected ids are written out rather than computed, so a case fails if the
/// service selects anything the catalog does not hold for it.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            label: "one day holding two fixtures of the sport",
            sport: "basketball",
            start: "2024-02-29",
            end: None,
            expected: &["alpha-opener", "zeta-nightcap"],
        },
        WindowCase {
            label: "that same day, for the other sport",
            sport: "american-football",
            start: "2024-02-29",
            end: None,
            expected: &["gridiron-kickoff"],
        },
        WindowCase {
            label: "a two-day window holding four fixtures",
            sport: "basketball",
            start: "2024-02-29",
            end: Some("2024-03-01"),
            expected: &[
                "alpha-opener",
                "mid-day-start",
                "night-owl",
                "zeta-nightcap",
            ],
        },
        WindowCase {
            label: "the day before the first held event",
            sport: "basketball",
            start: "2024-02-28",
            end: None,
            expected: &[],
        },
        WindowCase {
            label: "three days the catalog holds no event on",
            sport: "basketball",
            start: "2024-03-02",
            end: Some("2024-03-04"),
            expected: &[],
        },
        WindowCase {
            label: "a window whose last day holds the only event",
            sport: "american-football",
            start: "2024-02-28",
            end: Some("2024-02-29"),
            expected: &["gridiron-kickoff"],
        },
        WindowCase {
            label: "a seven-day window over held and empty days: the documented bound",
            sport: "american-football",
            start: "2024-02-29",
            end: Some("2024-03-06"),
            expected: &["gridiron-finale", "gridiron-kickoff"],
        },
        WindowCase {
            label: "a day decades in the past",
            sport: "basketball",
            start: "1999-12-31",
            end: None,
            expected: &["legacy-millennium"],
        },
        WindowCase {
            label: "the day after that one",
            sport: "basketball",
            start: "2000-01-01",
            end: None,
            expected: &[],
        },
        WindowCase {
            label: "a day decades in the future",
            sport: "american-football",
            start: "2099-07-04",
            end: None,
            expected: &["future-independence"],
        },
        WindowCase {
            label: "that future day, for the other sport",
            sport: "basketball",
            start: "2099-07-04",
            end: None,
            expected: &[],
        },
    ]
}

/// The table's sports are exactly the values the model declares, so every case below
/// is a case the service must accept.
#[test]
fn sports_are_the_modelled_enum() {
    let declared: Vec<&str> = DataSourceSport::ALL
        .iter()
        .map(|sport| sport.as_wire())
        .collect();
    assert_eq!(
        declared,
        vec!["basketball", "american-football"],
        "the DataSourceSport enum declares exactly these two sports \
         (smithy_query(query=\"shape\",shape=\"DataSourceSport\")), got {declared:?}"
    );
    for sport in &declared {
        assert!(
            DataSourceSport::from_wire(sport).is_some(),
            "case {sport:?}: a declared sport is a sport"
        );
    }
    assert!(
        DataSourceSport::from_wire("cricket").is_none(),
        "case \"cricket\": a value the enum does not declare is not a sport"
    );
    assert_eq!(
        MAX_SEARCH_WINDOW_DAYS, 7,
        "the windows above are written against the documented seven-day bound, got \
         {MAX_SEARCH_WINDOW_DAYS}"
    );
}

/// A catalog that holds NOTHING selects nothing: every window answers 200 with an
/// empty `fixtures` list and no `nextToken`, and no id can be retrieved.
///
/// "The array is empty if no fixtures match" (`SearchFixturesResponse$fixtures`): a
/// search of a data source with no events reports no rows at all, rather than a card of
/// its own.
#[tokio::test]
async fn a_catalog_that_holds_nothing_selects_nothing() {
    let (router, engine) = router_with_engine();
    clear(&engine);

    for case in window_cases() {
        let body = window_body(case.sport, case.start, case.end);
        let (status, answer) = search(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: a search of an empty catalog answers 200, got {status} with {answer}",
            case.label
        );
        assert_eq!(
            fixtures_of(&answer),
            Vec::<Value>::new(),
            "case {:?}: an empty catalog selects no fixture for {body}, got {answer}",
            case.label
        );
        assert_eq!(
            answer["nextToken"],
            Value::Null,
            "case {:?}: nothing remains to page, so there is no nextToken, got {answer}",
            case.label
        );
    }

    for row in seed_rows() {
        let (status, error) = get_fixture(&router, row.id).await;
        assert_eq!(
            status, 400,
            "case {:?}: an id an empty catalog does not hold is refused, got {status} with {error}",
            row.id
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {:?}: that refusal is a ValidationException, got {error}",
            row.id
        );
    }
}

/// A search reports exactly the held fixtures its sport and window select, in the
/// catalog's id order, each one member for member as the catalog holds it.
#[tokio::test]
async fn a_search_reports_exactly_the_held_fixtures_the_window_selects() {
    let router = seeded_router();

    for case in window_cases() {
        let body = window_body(case.sport, case.start, case.end);
        let (status, answer) = search(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: {body} answers 200, got {status} with {answer}",
            case.label
        );
        assert_eq!(
            ids_of(&answer),
            case.expected,
            "case {:?}: {body} selects exactly the held fixtures of that sport and window, \
             in id order, got {answer}",
            case.label
        );
        assert_eq!(
            answer["nextToken"],
            Value::Null,
            "case {:?}: the whole selection fits one page of 100, so there is no nextToken, \
             got {answer}",
            case.label
        );
        for fixture in fixtures_of(&answer) {
            let id = text(&fixture, "fixtureId");
            assert_is_row(case.label, &fixture, &id);
        }
    }
}

/// `GetFixture` reports the held record for every id a search handed back — the same
/// members the search reported — and refuses every id the catalog does not hold.
#[tokio::test]
async fn get_fixture_reports_the_held_record_and_refuses_any_other_id() {
    let router = seeded_router();

    // The round trip, over every held fixture: search its day, then fetch it by id.
    for row in seed_rows() {
        let case = format!("{} on {}", row.id, row.day);
        let (status, answer) = search(&router, window_body(row.sport.as_wire(), row.day, None)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the search answers 200, got {status} with {answer}"
        );
        let searched = fixtures_of(&answer)
            .into_iter()
            .find(|fixture| text(fixture, "fixtureId") == row.id)
            .unwrap_or_else(|| panic!("case {case:?}: the search reports the held fixture: {answer}"));

        let (status, fetched) = get_fixture(&router, row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetFixture on a held id answers 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched, searched,
            "case {case:?}: GetFixture reports the searched fixture member for member, got {fetched}"
        );
        assert_is_row(&case, &fetched, row.id);
    }

    // Ids the catalog does not hold, differing in form, length and in how near they
    // are to an id it does hold.
    let held = "alpha-opener";
    let cases: Vec<(String, String)> = vec![
        (
            "an id built like a sport and a day".to_string(),
            "basketball-20240229-01".to_string(),
        ),
        (
            "another such id, for a held day".to_string(),
            "american-football-20240229-01".to_string(),
        ),
        ("no structure at all".to_string(), "nonsense".to_string()),
        ("a single character".to_string(), "x".to_string()),
        ("digits only".to_string(), "12345".to_string()),
        (
            "a held id in upper case".to_string(),
            held.to_uppercase(),
        ),
        (
            "a held id with a suffix".to_string(),
            format!("{held}0"),
        ),
        (
            "a held id with its last character dropped".to_string(),
            held[..held.len() - 1].to_string(),
        ),
        (
            "the modelled maximum length".to_string(),
            "a".repeat(128),
        ),
        (
            "one past the modelled maximum length".to_string(),
            "a".repeat(129),
        ),
    ];
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

    // And the refusals disturbed nothing: the held fixture still answers.
    let (status, fetched) = get_fixture(&router, held).await;
    assert_eq!(
        status, 200,
        "case {held:?}: a held fixture still answers 200, got {status} with {fetched}"
    );
    assert_is_row("after the refusals", &fetched, held);
}

/// One filter case: the `filters` array, and the ids it narrows the window to.
struct FilterCase {
    label: &'static str,
    /// One entry per `SearchFilter`, each a list of `COMPETITOR` values.
    filters: &'static [&'static [&'static str]],
    expected: &'static [&'static str],
}

/// Filters over the two-day basketball window, which holds four fixtures among six
/// competitors. The rows differ along the value's case and completeness, how many
/// values one filter carries, how many fixtures a value matches, and how many filters
/// the request carries: "A fixture must satisfy every filter in the array in order to
/// appear in the results. Within one filter, a fixture must match at least one of the
/// values" (`SearchFixturesRequest$filters`).
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "a competitor's full name",
            filters: &[&["Omega Owls"]],
            expected: &["alpha-opener"],
        },
        FilterCase {
            label: "that name in lower case",
            filters: &[&["omega owls"]],
            expected: &["alpha-opener"],
        },
        FilterCase {
            label: "that name in upper case",
            filters: &[&["OMEGA OWLS"]],
            expected: &["alpha-opener"],
        },
        FilterCase {
            label: "part of that name",
            filters: &[&["Owls"]],
            expected: &["alpha-opener"],
        },
        FilterCase {
            label: "a competitor two fixtures share",
            filters: &[&["Beacon Bears"]],
            expected: &["night-owl", "zeta-nightcap"],
        },
        FilterCase {
            label: "one matching value among several",
            filters: &[&["Nowhere Nobodies", "Echo Eagles"]],
            expected: &["mid-day-start", "night-owl"],
        },
        FilterCase {
            label: "a competitor nobody is",
            filters: &[&["Nowhere Nobodies"]],
            expected: &[],
        },
        FilterCase {
            label: "a filter with no values at all",
            filters: &[&[]],
            expected: &[],
        },
        FilterCase {
            label: "two filters, both satisfied by two fixtures",
            filters: &[&["Beacon Bears"], &["Cobalt Comets"]],
            expected: &["night-owl", "zeta-nightcap"],
        },
        FilterCase {
            label: "three filters, satisfied by one fixture only",
            filters: &[&["Echo Eagles"], &["Delta Dolphins"], &["Beacon Bears"]],
            expected: &["night-owl"],
        },
        FilterCase {
            label: "two filters, one of them unsatisfiable",
            filters: &[&["Beacon Bears"], &["Nowhere Nobodies"]],
            expected: &[],
        },
    ]
}

/// A fixture appears only when it satisfies EVERY filter, and one filter is satisfied
/// by any of its values matching any of the fixture's competitors, whatever the case.
#[tokio::test]
async fn filters_narrow_a_window_to_the_fixtures_that_satisfy_every_one() {
    let router = seeded_router();

    for case in filter_cases() {
        let filters: Vec<Value> = case
            .filters
            .iter()
            .map(|values| json!({"name": "COMPETITOR", "values": values}))
            .collect();
        let mut body = window_body("basketball", "2024-02-29", Some("2024-03-01"));
        body["filters"] = Value::Array(filters);
        let (status, answer) = search(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: a filtered search answers 200, got {status} with {answer}",
            case.label
        );
        assert_eq!(
            ids_of(&answer),
            case.expected,
            "case {:?}: {body} selects exactly these fixtures, got {answer}",
            case.label
        );
        for fixture in fixtures_of(&answer) {
            let id = text(&fixture, "fixtureId");
            assert_is_row(case.label, &fixture, &id);
        }
    }
}

/// Paging a window: every page is at most `maxResults` long, a `nextToken` comes back
/// exactly while more remain, and the pages are the window's whole selection, in
/// order, with nothing repeated or skipped.
#[tokio::test]
async fn pages_of_a_window_are_its_whole_selection() {
    let router = seeded_router();
    let window = json!({"sport": "basketball", "startDate": "2024-02-29", "endDate": "2024-03-01"});
    let expected = [
        "alpha-opener",
        "mid-day-start",
        "night-owl",
        "zeta-nightcap",
    ];

    for page_size in [1_usize, 2, 3, 4, 100] {
        let case = format!("maxResults {page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        // The selection is bounded, so paging it can never need more requests than it
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
            let page_ids = ids_of(&page);
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
                "case {case:?}: a nextToken comes back exactly while fixtures remain \
                 ({remaining} left), got {page}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: the pages are the window's whole selection, in order"
        );
    }

    // A window the catalog holds nothing in pages nothing, whatever the page size.
    for page_size in [1_usize, 100] {
        let case = format!("an empty window with maxResults {page_size}");
        let mut body = window_body("basketball", "2024-03-02", Some("2024-03-04"));
        body["maxResults"] = json!(page_size);
        let (status, answer) = search(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: answers 200, got {status} with {answer}"
        );
        assert_eq!(
            ids_of(&answer),
            Vec::<String>::new(),
            "case {case:?}: an empty selection pages nothing, got {answer}"
        );
        assert_eq!(
            answer["nextToken"],
            Value::Null,
            "case {case:?}: and carries no nextToken, got {answer}"
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

/// Seconds in a UTC day: a UTC day has no offset changes, so an instant's day number
/// is its epoch-second count divided by this.
const SECONDS_PER_DAY: i64 = 86_400;

/// The seam's instant builder keeps a fixture ON the UTC day it names, so the day a
/// search window selects on is the day the caller meant.
#[test]
fn a_held_start_instant_must_be_a_time_of_the_day_it_names() {
    let day = parse_fixture_date("2024-02-29").expect("2024-02-29 is a real UTC day");
    // Each row: the time of day, and whether a UTC day has it.
    let rows: [(&str, i64, i64, bool); 8] = [
        ("midnight", 0, 0, true),
        ("a time in the afternoon", 17, 30, true),
        ("the last minute of the day", 23, 59, true),
        ("the first hour past the day", 24, 0, false),
        ("an hour no day has", 30, 0, false),
        ("a negative hour", -1, 0, false),
        ("the first minute past the hour range", 12, 60, false),
        ("a negative minute", 12, -1, false),
    ];
    for (case, hour, minute, accepted) in rows {
        let built = fixture_start_secs(day, hour, minute);
        assert_eq!(
            built.is_some(),
            accepted,
            "case {case:?}: {hour}:{minute} is{} a time of a UTC day, got {built:?}",
            if accepted { "" } else { " not" }
        );
        if let Some(secs) = built {
            assert_eq!(
                secs.div_euclid(SECONDS_PER_DAY),
                day,
                "case {case:?}: the instant falls on the day that was named, got {secs}"
            );
        }
    }
    assert!(
        fixture_start_secs(i64::MAX, 0, 0).is_none(),
        "case \"a day number no calendar date has\": there is no start instant on it"
    );
}

/// The seam's other half: clearing the catalog puts every window back to selecting
/// nothing, which is what a data source holding no event reports.
#[tokio::test]
async fn clearing_the_catalog_returns_every_window_to_an_empty_selection() {
    let (router, engine) = router_with_engine();
    clear(&engine);
    seed(&engine, seed_rows());
    let held = window_body("basketball", "2024-02-29", Some("2024-03-01"));
    let (status, before) = search(&router, held).await;
    assert_eq!(
        status, 200,
        "case \"before clearing\": the search answers 200, got {status} with {before}"
    );
    assert_eq!(
        ids_of(&before),
        ["alpha-opener", "mid-day-start", "night-owl", "zeta-nightcap"],
        "case \"before clearing\": the seeded window selects its fixtures, got {before}"
    );

    engine
        .state::<ComAmazonawsElementalinferenceElementalinferenceState>()
        .lock()
        .clear_fixtures();

    for case in window_cases() {
        let body = window_body(case.sport, case.start, case.end);
        let (status, answer) = search(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: a search of a cleared catalog answers 200, got {status} with {answer}",
            case.label
        );
        assert_eq!(
            ids_of(&answer),
            Vec::<String>::new(),
            "case {:?}: a cleared catalog selects no fixture for {body}, got {answer}",
            case.label
        );
        assert_eq!(
            answer["nextToken"],
            Value::Null,
            "case {:?}: and pages nothing, got {answer}",
            case.label
        );
    }
    for row in seed_rows() {
        let (status, error) = get_fixture(&router, row.id).await;
        assert_eq!(
            status, 400,
            "case {:?}: a cleared catalog holds no id to retrieve, got {status} with {error}",
            row.id
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {:?}: that refusal is a ValidationException, got {error}",
            row.id
        );
    }
}

/// Reporting only: searching and retrieving fixtures changes nothing a later caller
/// sees, and re-seeding a held id REPLACES that record rather than duplicating it.
#[tokio::test]
async fn reads_leave_the_catalog_as_it_was_and_re_seeding_replaces_in_place() {
    let (router, engine) = router_with_engine();
    clear(&engine);
    seed(&engine, seed_rows());
    let window = window_body("basketball", "2024-02-29", Some("2024-03-01"));

    let (_, first) = search(&router, window.clone()).await;
    let (_, fetched) = get_fixture(&router, "night-owl").await;
    assert_eq!(
        fetched["fixtureId"],
        json!("night-owl"),
        "GetFixture must report the id asked for, got {fetched}"
    );
    let (_, again) = search(&router, window.clone()).await;
    assert_eq!(
        first, again,
        "case \"repeat the search\": a search and a retrieval change nothing a later caller \
         sees, got {first} then {again}"
    );

    // The seam's replace-in-place: the same id with a different status and name.
    seed(
        &engine,
        vec![SeedRow {
            status: "Abandoned",
            name: "Echo Eagles at Beacon Bears (abandoned)",
            ..row_of("night-owl")
        }],
    );
    let (status, updated) = search(&router, window).await;
    assert_eq!(
        status, 200,
        "case \"after re-seeding\": the search answers 200, got {status} with {updated}"
    );
    assert_eq!(
        ids_of(&updated),
        ["alpha-opener", "mid-day-start", "night-owl", "zeta-nightcap"],
        "case \"after re-seeding\": the record is replaced, not duplicated, got {updated}"
    );
    let replaced = fixtures_of(&updated)
        .into_iter()
        .find(|fixture| text(fixture, "fixtureId") == "night-owl")
        .unwrap_or_else(|| panic!("the replaced fixture is reported: {updated}"));
    assert_eq!(
        text(&replaced, "status"),
        "Abandoned",
        "case \"after re-seeding\": the search reports the replaced record, got {replaced}"
    );
    let (status, refetched) = get_fixture(&router, "night-owl").await;
    assert_eq!(
        status, 200,
        "case \"after re-seeding\": GetFixture answers 200, got {status} with {refetched}"
    );
    assert_eq!(
        refetched, replaced,
        "case \"after re-seeding\": GetFixture agrees with the search, got {refetched}"
    );
}
