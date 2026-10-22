// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover of the Sustainability reporting loop, driven over the wire this
//! service speaks (`restJson1`: `POST /v1/…` with a SigV4 credential scope naming
//! `sustainability`) through the same `MultiServiceRouter` the production front door
//! serves.
//!
//! Every test is a TABLE whose rows differ along what the rule under test generalises
//! over — the dimension asked for, the time window, the grouping, the granularity and its
//! fiscal offset, the page size, the filter — so no single request is the specification.
//! Where a bound or a value set is written out, the comment beside it names the modelled
//! shape or the seeded-dataset constant it comes from.
//!
//! Timestamps go on the wire as epoch seconds, which is what this emulator's JSON codec
//! reads and writes for a `Timestamp` member.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sustainability_awssustainabilityapiservice::state as sustainability;
use service_com_amazonaws_sustainability_awssustainabilityapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service: that scope is
/// how the router picks the service which claims a request. The signature itself is never
/// verified here — [`Emulator`] turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/sustainability/aws4_request, SignedHeaders=host, Signature=not-verified";

/// The four modelled routes (`OPERATIONS.md`).
const CARBON: &str = "/v1/estimated-carbon-emissions";
const CARBON_DIMENSIONS: &str = "/v1/estimated-carbon-emissions-dimension-values";
const WATER: &str = "/v1/estimated-water-allocation";
const WATER_DIMENSIONS: &str = "/v1/estimated-water-allocation-dimension-values";

/// The response member each data operation keys its values by.
const CARBON_VALUES: &str = "EmissionsValues";
const WATER_VALUES: &str = "AllocationValues";

/// One day in seconds, for a window that starts or ends inside a month.
const DAY: i64 = 86_400;

/// The instant a UTC calendar month starts, as epoch seconds.
fn month_start(year: i32, month: u8) -> i64 {
    sustainability::month_start_secs(year, month)
}

/// A six-month window whose every month start the range includes: 2025-01 .. 2025-06.
fn first_half_of_2025() -> (i64, i64) {
    (month_start(2025, 1), month_start(2025, 7))
}

/// The whole seeded range, from the first seeded month to just past the last one.
fn whole_dataset() -> (i64, i64) {
    (
        month_start(sustainability::SEED_FIRST_YEAR, 1),
        month_start(sustainability::SEED_LAST_YEAR + 1, 1),
    )
}

/// Every period boundary the service reports is a UTC month start, so the conversion that
/// produces them is pinned against epoch instants known independently of this crate.
///
/// The rows span the leap-year rules (a common year, a leap year, a century that is not a
/// leap year, a century that is), the epoch itself and a pre-epoch date, plus each end of
/// the seeded range — the cases a civil-date conversion gets wrong when it is wrong.
#[test]
fn month_starts_are_the_epoch_instants_of_utc_month_boundaries() {
    // (case, year, month, epoch seconds)
    let cases: [(&str, i32, u8, i64); 12] = [
        ("the epoch itself", 1970, 1, 0),
        ("one month after the epoch", 1970, 2, 31 * DAY),
        ("before the epoch", 1969, 12, -31 * DAY),
        ("a common year", 2025, 1, 1_735_689_600),
        ("March of a common year", 2025, 3, 1_740_787_200),
        ("a leap year", 2024, 1, 1_704_067_200),
        // 2024-03-01 is 31 + 29 days into a leap year.
        ("March of a leap year", 2024, 3, 1_704_067_200 + 60 * DAY),
        ("a century that is a leap year", 2000, 3, 951_868_800),
        ("a century that is not a leap year", 1900, 3, -2_203_891_200),
        ("the first seeded month", 2024, 1, 1_704_067_200),
        ("the last seeded month", 2026, 12, 1_796_083_200),
        ("just past the last seeded month", 2027, 1, 1_798_761_600),
    ];

    for (case, year, month, expected) in cases {
        let reported = month_start(year, month);
        assert_eq!(
            reported, expected,
            "case {case}: {year}-{month:02}-01T00:00:00Z is {expected} epoch seconds, got {reported}"
        );
        // Every month start is a whole number of days, and the next month start is 28..=31
        // days later.
        assert_eq!(
            reported % DAY,
            0,
            "case {case}: a month start must be midnight, got {reported}"
        );
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        let length = (month_start(next_year, next_month) - reported) / DAY;
        assert!(
            (28..=31).contains(&length),
            "case {case}: the month must be 28..=31 days long, got {length}"
        );
    }
}

/// A router with only this service mounted — one emulator "session".
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
        Self { router }
    }

    /// POST `path` with a JSON body; returns the status and the parsed body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://localhost{path}"))
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// POST and require HTTP 200.
    async fn ok(&self, path: &str, body: &Value) -> Value {
        let (status, parsed) = self.post(path, body).await;
        assert_eq!(
            status, 200,
            "case {path} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// POST and require a 400 `ValidationException`.
    async fn refused(&self, case: &str, path: &str, body: &Value) {
        let (status, parsed) = self.post(path, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for {path} {body}, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected a ValidationException for {path} {body}, got {parsed}"
        );
    }
}

/// The wire error code of a failure response: the last segment of its `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A request body: the required `TimePeriod` plus whatever members a case adds.
fn request(start: i64, end: i64, members: &[(&str, Value)]) -> Value {
    let mut body = serde_json::Map::new();
    body.insert(
        "TimePeriod".to_string(),
        json!({ "Start": start, "End": end }),
    );
    for (name, value) in members {
        body.insert((*name).to_string(), value.clone());
    }
    Value::Object(body)
}

/// The `Results` list of a response.
fn results(case: &str, body: &Value) -> Vec<Value> {
    body.get("Results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: response carries no Results list: {body}"))
}

/// One row's `TimePeriod`, as (Start, End) epoch seconds.
fn row_period(case: &str, row: &Value) -> (i64, i64) {
    let period = row
        .get("TimePeriod")
        .unwrap_or_else(|| panic!("case {case}: row carries no TimePeriod: {row}"));
    let member = |name: &str| -> i64 {
        let seconds = period
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: TimePeriod.{name} is not a number: {row}"));
        seconds.round() as i64
    };
    (member("Start"), member("End"))
}

/// One row's value for one metric, in the member the operation keys metrics by.
fn row_value(case: &str, row: &Value, values_member: &str, metric: &str) -> f64 {
    let entry = row
        .get(values_member)
        .and_then(|values| values.get(metric))
        .unwrap_or_else(|| panic!("case {case}: row has no {values_member}.{metric}: {row}"));
    entry.get("Value").and_then(Value::as_f64).unwrap_or_else(|| {
        panic!("case {case}: {values_member}.{metric}.Value is not a number: {row}")
    })
}

/// The sum of one metric over a set of rows.
fn total(case: &str, rows: &[Value], values_member: &str, metric: &str) -> f64 {
    rows.iter()
        .map(|row| row_value(case, row, values_member, metric))
        .sum()
}

/// The metric a data operation's rows are compared on: its first documented default type.
fn default_metric(values_member: &str) -> &'static str {
    if values_member == CARBON_VALUES {
        sustainability::DEFAULT_EMISSIONS_TYPES[0]
    } else {
        sustainability::DEFAULT_WATER_ALLOCATION_TYPES[0]
    }
}

/// Equality for aggregated doubles: summing the same seeded values one monthly row at a
/// time or in one yearly bucket differs only in float rounding.
fn close(left: f64, right: f64) -> bool {
    (left - right).abs() <= 1e-9 * left.abs().max(right.abs()).max(1.0)
}

/// A JSON string list.
fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|value| json!(value)).collect())
}

/// One row of the grouping table: case, route, values member, `Granularity`, `GroupBy`,
/// `FilterBy`, and how many rows the request must answer with.
type GroupingCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Vec<&'a str>,
    Option<Value>,
    usize,
);

/// One row of the bucketing table: case, `Granularity`, `FiscalYearStartMonth`, `GroupBy`,
/// and how many rows the request must answer with.
type BucketingCase<'a> = (&'a str, &'a str, Option<i64>, Vec<&'a str>, usize);

/// The keys of one row's values map.
fn value_keys(row: &Value, values_member: &str) -> BTreeSet<String> {
    row.get(values_member)
        .and_then(Value::as_object)
        .map(|values| values.keys().cloned().collect())
        .unwrap_or_default()
}

/// A set of `&str` as owned strings, for comparing against what a row reports.
fn owned(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(ToString::to_string).collect()
}

/// Walk every page of a paginated operation, following `NextToken`, and return the rows in
/// the order the service returned them.
///
/// Checks the pagination contract as it goes: no page is larger than the `MaxResults` asked
/// for, every token is within the modelled length, and the walk ends on a page with no
/// token.
async fn drain(
    emulator: &Emulator,
    case: &str,
    path: &str,
    body: &Value,
    page_size: usize,
) -> Vec<Value> {
    /// More pages than any seeded request can produce, so a token that fails to advance
    /// fails the test instead of looping forever.
    const PAGE_LIMIT: usize = 2_000;

    let mut rows: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut page_body = body.clone();
        {
            let members = page_body
                .as_object_mut()
                .expect("the request body is a JSON object");
            members.insert("MaxResults".to_string(), json!(page_size));
            match token.as_deref() {
                Some(token) => {
                    members.insert("NextToken".to_string(), json!(token));
                }
                None => {
                    members.remove("NextToken");
                }
            }
        }

        let page = emulator.ok(path, &page_body).await;
        let page_rows = results(case, &page);
        assert!(
            page_rows.len() <= page_size,
            "case {case}: a page of {} rows exceeds the MaxResults {page_size} asked for",
            page_rows.len()
        );
        rows.extend(page_rows);
        pages += 1;
        assert!(
            pages <= PAGE_LIMIT,
            "case {case}: pagination did not terminate"
        );

        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        match token.as_deref() {
            // The modelled `NextToken` length is 1..2000.
            Some(token) => assert!(
                (1..=2000).contains(&token.len()),
                "case {case}: a NextToken of {} characters is outside the modelled 1..2000",
                token.len()
            ),
            None => return rows,
        }
    }
}

/// The distinct values one dimension takes, as the dimension-values operations report them.
async fn dimension_values(
    emulator: &Emulator,
    path: &str,
    start: i64,
    end: i64,
    dimension: &str,
) -> Vec<String> {
    let case = format!("{path} {dimension}");
    let body = request(
        start,
        end,
        &[
            ("Dimensions", strings(&[dimension])),
            ("MaxResults", json!(5000)),
        ],
    );
    let body = emulator.ok(path, &body).await;
    results(&case, &body)
        .iter()
        .map(|entry| {
            assert_eq!(
                entry.get("Dimension").and_then(Value::as_str),
                Some(dimension),
                "case {case}: entry reports another dimension: {entry}"
            );
            entry
                .get("Value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: entry carries no Value: {entry}"))
                .to_string()
        })
        .collect()
}

/// The dimension values each operation reports are exactly the distinct values its OWN
/// dataset takes — so the carbon and water surfaces answer from independent datasets, and
/// `USAGE_ACCOUNT_ID` is the account this emulator presents.
#[tokio::test]
async fn dimension_values_are_the_distinct_values_of_the_operations_dataset() {
    let emulator = Emulator::new();
    let (start, end) = whole_dataset();
    // Expected sets are the seeded dataset's own constants (`crate::state`), not a
    // hand-copied list: SEED_REGIONS, SEED_CARBON_SERVICES, SEED_WATER_SERVICES, and the
    // account `engine::emulator_account_id()` presents.
    let accounts = vec![sustainability::seed_account_id()];
    let regions: Vec<String> = sustainability::SEED_REGIONS
        .iter()
        .map(ToString::to_string)
        .collect();
    let carbon_services: Vec<String> = sustainability::SEED_CARBON_SERVICES
        .iter()
        .map(ToString::to_string)
        .collect();
    let water_services: Vec<String> = sustainability::SEED_WATER_SERVICES
        .iter()
        .map(ToString::to_string)
        .collect();

    let cases: Vec<(&str, &str, &str, &Vec<String>)> = vec![
        ("carbon regions", CARBON_DIMENSIONS, "REGION", &regions),
        ("water regions", WATER_DIMENSIONS, "REGION", &regions),
        (
            "carbon services",
            CARBON_DIMENSIONS,
            "SERVICE",
            &carbon_services,
        ),
        ("water services", WATER_DIMENSIONS, "SERVICE", &water_services),
        (
            "carbon accounts",
            CARBON_DIMENSIONS,
            "USAGE_ACCOUNT_ID",
            &accounts,
        ),
        (
            "water accounts",
            WATER_DIMENSIONS,
            "USAGE_ACCOUNT_ID",
            &accounts,
        ),
    ];

    for (case, path, dimension, expected) in cases {
        let reported = dimension_values(&emulator, path, start, end, dimension).await;
        let reported_set: BTreeSet<&String> = reported.iter().collect();
        let expected_set: BTreeSet<&String> = expected.iter().collect();
        assert_eq!(
            reported_set, expected_set,
            "case {case}: expected the dataset's distinct {dimension} values {expected:?}, got {reported:?}"
        );
        assert_eq!(
            reported.len(),
            reported_set.len(),
            "case {case}: {dimension} values repeat: {reported:?}"
        );
        let mut sorted = reported.clone();
        sorted.sort();
        assert_eq!(
            reported, sorted,
            "case {case}: {dimension} values are not reported in a stable order: {reported:?}"
        );
    }

    // The two datasets are independent, so the water surface does not report a service only
    // the carbon dataset covers.
    let carbon_only: Vec<&&str> = sustainability::SEED_CARBON_SERVICES
        .iter()
        .filter(|service| !sustainability::SEED_WATER_SERVICES.contains(service))
        .collect();
    assert!(
        !carbon_only.is_empty(),
        "the seeded datasets must differ for this assertion to mean anything"
    );
    let water_reported = dimension_values(&emulator, WATER_DIMENSIONS, start, end, "SERVICE").await;
    for service in carbon_only {
        assert!(
            !water_reported.contains(&(*service).to_string()),
            "case water services: {service} is not in the water dataset but was reported: {water_reported:?}"
        );
    }
}

/// A request's `Dimensions` order is the order the entries come back in, and every value a
/// dimension-values call reports is a value `FilterBy` then accepts on the data operation —
/// the whole point of calling it first.
#[tokio::test]
async fn dimension_values_pair_with_what_filter_by_accepts() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    let cases: [(&str, &str, &str, &str, &str); 2] = [
        (
            "carbon",
            CARBON_DIMENSIONS,
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
        ),
        // Water supports only YEARLY_CALENDAR.
        (
            "water",
            WATER_DIMENSIONS,
            WATER,
            WATER_VALUES,
            "YEARLY_CALENDAR",
        ),
    ];

    for (case, dimensions_path, data_path, values_member, granularity) in cases {
        let body = request(
            start,
            end,
            &[
                ("Dimensions", strings(&["SERVICE", "REGION"])),
                ("MaxResults", json!(5000)),
            ],
        );
        let entries = results(case, &emulator.ok(dimensions_path, &body).await);
        let order: Vec<&str> = entries
            .iter()
            .filter_map(|entry| entry.get("Dimension").and_then(Value::as_str))
            .collect();
        let first_region = order.iter().position(|dimension| *dimension == "REGION");
        let last_service = order.iter().rposition(|dimension| *dimension == "SERVICE");
        assert!(
            last_service < first_region,
            "case {case}: entries must follow the requested Dimensions order, got {order:?}"
        );

        let metric = default_metric(values_member);
        let unfiltered = results(
            case,
            &emulator
                .ok(
                    data_path,
                    &request(
                        start,
                        end,
                        &[
                            ("Granularity", json!(granularity)),
                            ("MaxResults", json!(5000)),
                        ],
                    ),
                )
                .await,
        );
        let whole = total(case, &unfiltered, values_member, metric);

        let services = dimension_values(&emulator, dimensions_path, start, end, "SERVICE").await;
        let mut per_service = 0.0;
        for service in &services {
            let filtered = results(
                case,
                &emulator
                    .ok(
                        data_path,
                        &request(
                            start,
                            end,
                            &[
                                ("Granularity", json!(granularity)),
                                ("MaxResults", json!(5000)),
                                (
                                    "FilterBy",
                                    json!({ "Dimensions": { "SERVICE": [service] } }),
                                ),
                            ],
                        ),
                    )
                    .await,
            );
            assert!(
                !filtered.is_empty(),
                "case {case}: filtering by the reported value {service} selected no rows"
            );
            per_service += total(case, &filtered, values_member, metric);
        }
        assert!(
            close(per_service, whole),
            "case {case}: the per-service totals {per_service} must add up to the unfiltered total {whole}"
        );
    }
}

/// A row is in the answer exactly when the requested range includes the START of the period
/// that row covers (`Start` inclusive, `End` exclusive) — the rule each operation's
/// modelled `TimePeriod` documents: the start of a month for carbon, whose smallest
/// supported granularity is MONTHLY, and the start of a year for water, which supports only
/// YEARLY_CALENDAR.
#[tokio::test]
async fn a_row_is_selected_by_the_start_of_the_period_it_covers() {
    let emulator = Emulator::new();
    let january = month_start(2025, 1);
    let march = month_start(2025, 3);

    // Carbon: "The range must include the start date of a month for that month's data to be
    // included in the response." (case, Start, End, rows at MONTHLY granularity)
    let monthly_windows: [(&str, i64, i64, usize); 6] = [
        ("one whole month", march, month_start(2025, 4), 1),
        ("two whole months", march, month_start(2025, 5), 2),
        (
            "inside one month, no month start",
            january + DAY,
            january + 19 * DAY,
            0,
        ),
        (
            "ends exactly on a month start",
            january + 14 * DAY,
            month_start(2025, 2),
            0,
        ),
        (
            "crosses one month start",
            january + 14 * DAY,
            month_start(2025, 2) + DAY,
            1,
        ),
        (
            "before the dataset",
            month_start(2019, 1),
            month_start(2020, 1),
            0,
        ),
    ];

    // Water: "The range must include the start date of a year for that year's data to be
    // included in the response." (case, Start, End, rows at YEARLY_CALENDAR granularity)
    let yearly_windows: [(&str, i64, i64, usize); 6] = [
        (
            "one whole year",
            month_start(2025, 1),
            month_start(2026, 1),
            1,
        ),
        ("two whole years", month_start(2024, 1), month_start(2026, 1), 2),
        (
            "inside one year, no year start",
            month_start(2025, 2),
            month_start(2025, 12),
            0,
        ),
        (
            "ends exactly on a year start",
            month_start(2025, 2),
            month_start(2026, 1),
            0,
        ),
        (
            // The example in the modelled Granularity doc: "will return all the data for
            // 2026 only".
            "crosses one year start",
            month_start(2025, 4),
            month_start(2026, 4),
            1,
        ),
        (
            "before the dataset",
            month_start(2019, 1),
            month_start(2020, 1),
            0,
        ),
    ];

    for (path, dimensions_path, granularity, windows) in [
        (CARBON, CARBON_DIMENSIONS, "MONTHLY", monthly_windows),
        (WATER, WATER_DIMENSIONS, "YEARLY_CALENDAR", yearly_windows),
    ] {
        for (case, start, end, expected) in windows {
            // Ungrouped, at the dataset's own granularity: one row per selected period.
            let body = request(
                start,
                end,
                &[
                    ("Granularity", json!(granularity)),
                    ("MaxResults", json!(5000)),
                ],
            );
            let rows = results(case, &emulator.ok(path, &body).await);
            assert_eq!(
                rows.len(),
                expected,
                "case {case} on {path}: expected {expected} rows, got {}",
                rows.len()
            );

            // The matching dimension-values operation selects rows by the same rule, so it
            // reports nothing exactly when nothing is selected.
            let entries = dimension_values(&emulator, dimensions_path, start, end, "REGION").await;
            assert_eq!(
                entries.is_empty(),
                expected == 0,
                "case {case} on {dimensions_path}: expected {} dimension values, got {entries:?}",
                if expected == 0 { "no" } else { "some" }
            );
        }
    }
}

/// `GroupBy` decides the rows: one row per (period, combination of the grouped dimensions'
/// values), with `DimensionsValues` carrying exactly those dimensions; `FilterBy` restricts
/// which rows are summed into them.
#[tokio::test]
async fn rows_are_one_per_period_and_group_combination() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    let months = 6; // 2025-01 .. 2025-06
    let regions = sustainability::SEED_REGIONS.len();
    let carbon_services = sustainability::SEED_CARBON_SERVICES.len();
    let water_services = sustainability::SEED_WATER_SERVICES.len();

    // Carbon rows are monthly and water rows annual, so each case asks at the granularity
    // its operation supports. (case, path, values member, Granularity, GroupBy, FilterBy,
    // expected rows)
    let years = 1; // the range includes the start of 2025 only
    let cases: Vec<GroupingCase<'_>> = vec![
        (
            "carbon, no grouping",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec![],
            None,
            months,
        ),
        (
            "carbon by region",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["REGION"],
            None,
            months * regions,
        ),
        (
            "carbon by service",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["SERVICE"],
            None,
            months * carbon_services,
        ),
        (
            "carbon by region and service",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["REGION", "SERVICE"],
            None,
            months * regions * carbon_services,
        ),
        (
            "carbon by account",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["USAGE_ACCOUNT_ID"],
            None,
            months,
        ),
        (
            "carbon by region, one service",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["REGION"],
            Some(json!({ "Dimensions": { "SERVICE": ["AmazonEC2"] } })),
            months * regions,
        ),
        (
            "carbon by service, two regions",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["SERVICE"],
            Some(json!({ "Dimensions": { "REGION": ["us-east-1", "us-west-2"] } })),
            months * carbon_services,
        ),
        (
            "carbon by service, one service",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["SERVICE"],
            Some(json!({ "Dimensions": { "SERVICE": ["AmazonEC2"] } })),
            months,
        ),
        (
            "carbon, a filter nothing matches",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec!["SERVICE"],
            Some(json!({ "Dimensions": { "SERVICE": ["AmazonNotSeeded"] } })),
            0,
        ),
        (
            "carbon, two filters",
            CARBON,
            CARBON_VALUES,
            "MONTHLY",
            vec![],
            Some(json!({
                "Dimensions": { "SERVICE": ["AmazonEC2", "AmazonS3"], "REGION": ["eu-west-1"] }
            })),
            months,
        ),
        (
            "carbon by quarter and region",
            CARBON,
            CARBON_VALUES,
            "QUARTERLY_CALENDAR",
            vec!["REGION"],
            None,
            2 * regions,
        ),
        (
            "water, no grouping",
            WATER,
            WATER_VALUES,
            "YEARLY_CALENDAR",
            vec![],
            None,
            years,
        ),
        (
            "water by service",
            WATER,
            WATER_VALUES,
            "YEARLY_CALENDAR",
            vec!["SERVICE"],
            None,
            years * water_services,
        ),
        (
            "water by region and service",
            WATER,
            WATER_VALUES,
            "YEARLY_CALENDAR",
            vec!["REGION", "SERVICE"],
            None,
            years * regions * water_services,
        ),
        (
            "water by region, one service",
            WATER,
            WATER_VALUES,
            "YEARLY_CALENDAR",
            vec!["REGION"],
            Some(json!({ "Dimensions": { "SERVICE": ["AmazonEC2"] } })),
            years * regions,
        ),
        (
            "water, a filter nothing matches",
            WATER,
            WATER_VALUES,
            "YEARLY_CALENDAR",
            vec!["SERVICE"],
            Some(json!({ "Dimensions": { "SERVICE": ["AWSLambda"] } })),
            0,
        ),
    ];

    for (case, path, values_member, granularity, group_by, filter, expected) in cases {
        let mut members: Vec<(&str, Value)> = vec![
            ("Granularity", json!(granularity)),
            ("MaxResults", json!(5000)),
        ];
        if !group_by.is_empty() {
            members.push(("GroupBy", strings(&group_by)));
        }
        if let Some(filter) = filter {
            members.push(("FilterBy", filter));
        }
        let rows = results(case, &emulator.ok(path, &request(start, end, &members)).await);
        assert_eq!(
            rows.len(),
            expected,
            "case {case}: expected {expected} rows, got {}",
            rows.len()
        );

        let metric = default_metric(values_member);
        let expected_unit = if values_member == CARBON_VALUES {
            sustainability::EMISSIONS_UNIT
        } else {
            sustainability::WATER_ALLOCATION_UNIT
        };
        let mut combinations: BTreeSet<(i64, Vec<String>)> = BTreeSet::new();
        for row in &rows {
            let dimensions: BTreeMap<String, String> = row
                .get("DimensionsValues")
                .and_then(Value::as_object)
                .map(|values| {
                    values
                        .iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_else(|| panic!("case {case}: row carries no DimensionsValues: {row}"));
            assert_eq!(
                dimensions.len(),
                group_by.len(),
                "case {case}: DimensionsValues must carry exactly the grouped dimensions {group_by:?}, got {dimensions:?}"
            );
            for dimension in &group_by {
                assert!(
                    dimensions.contains_key(*dimension),
                    "case {case}: DimensionsValues is missing {dimension}: {dimensions:?}"
                );
            }
            assert_eq!(
                row.get("ModelVersion").and_then(Value::as_str),
                Some(sustainability::MODEL_VERSION),
                "case {case}: every row carries the dataset's model version: {row}"
            );
            let value = row_value(case, row, values_member, metric);
            assert!(
                value > 0.0,
                "case {case}: a summed value must be positive, got {value} in {row}"
            );
            let unit = row
                .get(values_member)
                .and_then(|values| values.get(metric))
                .and_then(|entry| entry.get("Unit"))
                .and_then(Value::as_str);
            assert_eq!(
                unit,
                Some(expected_unit),
                "case {case}: the value's unit must be the modelled one: {row}"
            );
            let (period_start, period_end) = row_period(case, row);
            assert!(
                period_start < end && period_end > start && period_start < period_end,
                "case {case}: the row's period must be non-empty and overlap the requested range: {row}"
            );
            assert!(
                combinations.insert((
                    period_start,
                    dimensions.values().cloned().collect::<Vec<String>>()
                )),
                "case {case}: the (period, group) combination at {period_start} is reported twice: {dimensions:?}"
            );
        }
    }
}

/// Bucketing and grouping only redistribute the months the range selects: whatever the
/// granularity, the fiscal offset or the grouping, the total of a metric over the answer is
/// the total of those months.
#[tokio::test]
async fn bucketing_and_grouping_preserve_the_total() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    let metric = sustainability::DEFAULT_EMISSIONS_TYPES[0];

    let monthly = results(
        "monthly baseline",
        &emulator
            .ok(
                CARBON,
                &request(
                    start,
                    end,
                    &[
                        ("Granularity", json!("MONTHLY")),
                        ("MaxResults", json!(5000)),
                    ],
                ),
            )
            .await,
    );
    let baseline = total("monthly baseline", &monthly, CARBON_VALUES, metric);

    // (case, Granularity, FiscalYearStartMonth, GroupBy, expected rows over 2025-01..2025-07)
    // The fiscal buckets are the calendar ones shifted by
    // GranularityConfiguration.FiscalYearStartMonth (range 1..12, default 1), so a fiscal
    // year starting in month 1 is exactly the calendar one.
    let cases: Vec<BucketingCase<'_>> = vec![
        ("monthly", "MONTHLY", None, vec![], 6),
        ("calendar quarters", "QUARTERLY_CALENDAR", None, vec![], 2),
        ("calendar year", "YEARLY_CALENDAR", None, vec![], 1),
        (
            "fiscal year from month 1",
            "YEARLY_FISCAL",
            Some(1),
            vec![],
            1,
        ),
        (
            "fiscal year from month 4",
            "YEARLY_FISCAL",
            Some(4),
            vec![],
            2,
        ),
        (
            // A fiscal year starting in month 12 runs 2024-12..2025-12, so it contains the
            // whole range: one row, clipped to the range.
            "fiscal year from month 12",
            "YEARLY_FISCAL",
            Some(12),
            vec![],
            1,
        ),
        (
            "fiscal quarters from month 1",
            "QUARTERLY_FISCAL",
            Some(1),
            vec![],
            2,
        ),
        (
            "fiscal quarters from month 2",
            "QUARTERLY_FISCAL",
            Some(2),
            vec![],
            3,
        ),
        (
            "fiscal quarters from month 7",
            "QUARTERLY_FISCAL",
            Some(7),
            vec![],
            2,
        ),
        (
            "calendar year by region",
            "YEARLY_CALENDAR",
            None,
            vec!["REGION"],
            3,
        ),
        (
            "calendar quarters by region and service",
            "QUARTERLY_CALENDAR",
            None,
            vec!["REGION", "SERVICE"],
            2 * 3 * 4,
        ),
        ("monthly by service", "MONTHLY", None, vec!["SERVICE"], 6 * 4),
    ];

    for (case, granularity, fiscal, group_by, expected) in cases {
        let mut members: Vec<(&str, Value)> = vec![
            ("Granularity", json!(granularity)),
            ("MaxResults", json!(5000)),
        ];
        if let Some(fiscal) = fiscal {
            members.push((
                "GranularityConfiguration",
                json!({ "FiscalYearStartMonth": fiscal }),
            ));
        }
        if !group_by.is_empty() {
            members.push(("GroupBy", strings(&group_by)));
        }
        let rows = results(case, &emulator.ok(CARBON, &request(start, end, &members)).await);
        assert_eq!(
            rows.len(),
            expected,
            "case {case}: expected {expected} rows, got {}",
            rows.len()
        );
        let summed = total(case, &rows, CARBON_VALUES, metric);
        assert!(
            close(summed, baseline),
            "case {case}: the total {summed} must equal the monthly total {baseline}"
        );

        // An ungrouped answer's periods tile the requested range exactly once: consecutive,
        // no gap, no overlap, clipped to the range at both ends.
        let mut periods: Vec<(i64, i64)> = rows.iter().map(|row| row_period(case, row)).collect();
        periods.sort_unstable();
        periods.dedup();
        if group_by.is_empty() {
            assert_eq!(
                periods.first().map(|period| period.0),
                Some(start),
                "case {case}: the first period must start at the requested Start: {periods:?}"
            );
            assert_eq!(
                periods.last().map(|period| period.1),
                Some(end),
                "case {case}: the last period must end at the requested End: {periods:?}"
            );
            for window in periods.windows(2) {
                assert_eq!(
                    window[0].1, window[1].0,
                    "case {case}: periods must be consecutive with no gap or overlap: {periods:?}"
                );
            }
        }
    }
}

/// A period the range only partly covers is reported clipped to the range and carries only
/// the months inside it — the behaviour the modelled `Granularity` documents with
/// "requesting 2025-04-01T00:00:00Z to 2026-04-01T00:00:00Z with YEARLY_CALENDAR
/// granularity will return the last 9 months for 2025 and the first 3 months of 2026".
#[tokio::test]
async fn a_partly_covered_period_is_clipped_and_carries_only_its_months() {
    let emulator = Emulator::new();
    let start = month_start(2025, 4);
    let boundary = month_start(2026, 1);
    let end = month_start(2026, 4);
    let metric = sustainability::DEFAULT_EMISSIONS_TYPES[0];

    let rows = results(
        "documented example",
        &emulator
            .ok(
                CARBON,
                &request(
                    start,
                    end,
                    &[
                        ("Granularity", json!("YEARLY_CALENDAR")),
                        ("MaxResults", json!(5000)),
                    ],
                ),
            )
            .await,
    );
    let periods: Vec<(i64, i64)> = rows
        .iter()
        .map(|row| row_period("documented example", row))
        .collect();
    assert_eq!(
        periods,
        vec![(start, boundary), (boundary, end)],
        "the two calendar years must be reported clipped to the requested range"
    );

    let monthly = results(
        "documented example monthly",
        &emulator
            .ok(
                CARBON,
                &request(
                    start,
                    end,
                    &[
                        ("Granularity", json!("MONTHLY")),
                        ("MaxResults", json!(5000)),
                    ],
                ),
            )
            .await,
    );
    assert_eq!(monthly.len(), 12, "the range covers twelve month starts");

    // Each clipped row sums exactly the monthly rows inside it: the last 9 months of 2025,
    // then the first 3 of 2026.
    for (case, row, from, to, months) in [
        ("2025 part", &rows[0], start, boundary, 9),
        ("2026 part", &rows[1], boundary, end, 3),
    ] {
        let inside: Vec<&Value> = monthly
            .iter()
            .filter(|row| {
                let (row_start, _) = row_period(case, row);
                row_start >= from && row_start < to
            })
            .collect();
        assert_eq!(
            inside.len(),
            months,
            "case {case}: expected {months} monthly rows inside the clipped period"
        );
        let expected: f64 = inside
            .iter()
            .map(|row| row_value(case, row, CARBON_VALUES, metric))
            .sum();
        let reported = row_value(case, row, CARBON_VALUES, metric);
        assert!(
            close(reported, expected),
            "case {case}: the clipped row reports {reported}, but its months sum to {expected}"
        );
    }
}

/// Water allocation is annual: the same range the carbon doc splits across two clipped
/// years answers with ONE whole year here — "requesting 2025-04-01T00:00:00Z to
/// 2026-04-01T00:00:00Z with YEARLY_CALENDAR will return all the data for 2026 only"
/// (`GetEstimatedWaterAllocation`'s modelled documentation of `Granularity`), 2026 being the
/// only year whose start the range includes.
#[tokio::test]
async fn a_water_range_answers_with_the_whole_year_whose_start_it_includes() {
    let emulator = Emulator::new();
    let start = month_start(2025, 4);
    let year_2026 = month_start(2026, 1);
    let year_2027 = month_start(2027, 1);
    let end = month_start(2026, 4);
    let metric = sustainability::DEFAULT_WATER_ALLOCATION_TYPES[0];

    for (case, group_by, expected_rows) in [
        ("ungrouped", vec![], 1),
        (
            "by service",
            vec!["SERVICE"],
            sustainability::SEED_WATER_SERVICES.len(),
        ),
    ] {
        let mut members: Vec<(&str, Value)> = vec![
            ("Granularity", json!("YEARLY_CALENDAR")),
            ("MaxResults", json!(5000)),
        ];
        if !group_by.is_empty() {
            members.push(("GroupBy", strings(&group_by)));
        }
        let partial = results(case, &emulator.ok(WATER, &request(start, end, &members)).await);
        assert_eq!(
            partial.len(),
            expected_rows,
            "case {case}: only 2026's data is selected, so expected {expected_rows} rows: {partial:?}"
        );
        for row in &partial {
            assert_eq!(
                row_period(case, row),
                (year_2026, year_2027),
                "case {case}: the row must report the whole year 2026, not a clipped range: {row}"
            );
        }

        // And it is exactly what a request for the whole of 2026 answers.
        let whole_year = results(
            case,
            &emulator
                .ok(WATER, &request(year_2026, year_2027, &members))
                .await,
        );
        assert_eq!(
            whole_year.len(),
            partial.len(),
            "case {case}: the partial range must answer with the same rows as the whole year"
        );
        assert!(
            close(
                total(case, &partial, WATER_VALUES, metric),
                total(case, &whole_year, WATER_VALUES, metric)
            ),
            "case {case}: the partial range must report the whole year's values"
        );
    }
}

/// Walking the pages of a request yields exactly the unpaginated answer: every row once, in
/// the same order, whatever the page size.
#[tokio::test]
async fn following_next_token_yields_every_row_exactly_once() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    let (all_start, all_end) = whole_dataset();

    // (case, path, the request whose answer is paged)
    let requests: Vec<(&str, &str, Value)> = vec![
        (
            "carbon rows",
            CARBON,
            request(
                start,
                end,
                &[
                    ("Granularity", json!("MONTHLY")),
                    ("GroupBy", strings(&["REGION", "SERVICE"])),
                ],
            ),
        ),
        (
            "water rows",
            WATER,
            request(
                all_start,
                all_end,
                &[
                    ("Granularity", json!("YEARLY_CALENDAR")),
                    ("GroupBy", strings(&["REGION", "SERVICE"])),
                ],
            ),
        ),
        (
            "carbon dimension values",
            CARBON_DIMENSIONS,
            request(
                all_start,
                all_end,
                &[(
                    "Dimensions",
                    strings(&["REGION", "SERVICE", "USAGE_ACCOUNT_ID"]),
                )],
            ),
        ),
        (
            "water dimension values",
            WATER_DIMENSIONS,
            request(
                all_start,
                all_end,
                &[("Dimensions", strings(&["SERVICE", "REGION"]))],
            ),
        ),
    ];

    for (case, path, body) in requests {
        // One page big enough for everything is the reference answer.
        let whole = drain(&emulator, case, path, &body, 5000).await;
        assert!(
            !whole.is_empty(),
            "case {case}: the reference answer must not be empty"
        );
        for page_size in [1usize, 2, 5, 7, 23] {
            let walked = drain(&emulator, case, path, &body, page_size).await;
            assert_eq!(
                walked.len(),
                whole.len(),
                "case {case} at page size {page_size}: walked {} rows, the whole answer has {}",
                walked.len(),
                whole.len()
            );
            assert_eq!(
                walked, whole,
                "case {case} at page size {page_size}: the concatenated pages must equal the whole answer"
            );
        }
    }
}

/// The two carbon totals are the sums of their scope components, at every granularity and
/// grouping: the answer is computed from the dataset's rows, not read off a canned row.
#[tokio::test]
async fn the_carbon_totals_are_the_sums_of_their_scope_components() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    // Every modelled `EmissionsType`, so one response carries both totals and all
    // components (the `EmissionsType` enum, all seven values).
    let all_types: Vec<&str> = vec![
        "TOTAL_LBM_CARBON_EMISSIONS",
        "TOTAL_MBM_CARBON_EMISSIONS",
        "TOTAL_SCOPE_1_CARBON_EMISSIONS",
        "TOTAL_SCOPE_2_LBM_CARBON_EMISSIONS",
        "TOTAL_SCOPE_2_MBM_CARBON_EMISSIONS",
        "TOTAL_SCOPE_3_LBM_CARBON_EMISSIONS",
        "TOTAL_SCOPE_3_MBM_CARBON_EMISSIONS",
    ];

    for (case, granularity, group_by) in [
        ("monthly, ungrouped", "MONTHLY", vec![]),
        (
            "monthly by region and service",
            "MONTHLY",
            vec!["REGION", "SERVICE"],
        ),
        ("yearly by service", "YEARLY_CALENDAR", vec!["SERVICE"]),
        ("fiscal quarters", "QUARTERLY_FISCAL", vec![]),
    ] {
        let mut members: Vec<(&str, Value)> = vec![
            ("Granularity", json!(granularity)),
            ("MaxResults", json!(5000)),
            ("EmissionsTypes", strings(&all_types)),
        ];
        if !group_by.is_empty() {
            members.push(("GroupBy", strings(&group_by)));
        }
        let rows = results(case, &emulator.ok(CARBON, &request(start, end, &members)).await);
        assert!(!rows.is_empty(), "case {case}: expected rows");
        for row in &rows {
            for (total_type, components) in [
                (
                    "TOTAL_LBM_CARBON_EMISSIONS",
                    sustainability::LOCATION_BASED_COMPONENTS,
                ),
                (
                    "TOTAL_MBM_CARBON_EMISSIONS",
                    sustainability::MARKET_BASED_COMPONENTS,
                ),
            ] {
                let reported = row_value(case, row, CARBON_VALUES, total_type);
                let composed: f64 = components
                    .iter()
                    .map(|component| row_value(case, row, CARBON_VALUES, component))
                    .sum();
                assert!(
                    close(reported, composed),
                    "case {case}: {total_type} is {reported} but its components sum to {composed}: {row}"
                );
            }
            assert_eq!(
                value_keys(row, CARBON_VALUES),
                owned(&all_types),
                "case {case}: the row must carry exactly the requested emissions types: {row}"
            );
        }
    }
}

/// An omitted member takes its modelled default: `EmissionsTypes` the two documented
/// totals, `Granularity` `MONTHLY` for carbon and `YEARLY_CALENDAR` for water, `GroupBy`
/// no grouping at all, and `MaxResults` 1000.
#[tokio::test]
async fn omitted_members_take_their_modelled_defaults() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();

    let carbon = results(
        "carbon defaults",
        &emulator.ok(CARBON, &request(start, end, &[])).await,
    );
    assert_eq!(
        carbon.len(),
        6,
        "carbon defaults: Granularity defaults to MONTHLY, so a six-month range is six rows: {carbon:?}"
    );
    for row in &carbon {
        assert_eq!(
            value_keys(row, CARBON_VALUES),
            owned(sustainability::DEFAULT_EMISSIONS_TYPES),
            "carbon defaults: an absent EmissionsTypes reports the two documented totals: {row}"
        );
        assert!(
            row.get("DimensionsValues")
                .and_then(Value::as_object)
                .is_some_and(serde_json::Map::is_empty),
            "carbon defaults: an absent GroupBy yields one aggregate row per period with an empty DimensionsValues: {row}"
        );
    }

    // Water's modelled `Granularity` default is YEARLY_CALENDAR, not carbon's MONTHLY, so
    // the same range answers with the one year whose start it includes rather than six
    // months.
    let water = results(
        "water defaults",
        &emulator
            .ok(
                WATER,
                &request(start, end, &[("GroupBy", strings(&["SERVICE"]))]),
            )
            .await,
    );
    assert_eq!(
        water.len(),
        sustainability::SEED_WATER_SERVICES.len(),
        "water defaults: YEARLY_CALENDAR buckets the range into one period: {water:?}"
    );
    for row in &water {
        assert_eq!(
            row_period("water defaults", row),
            (month_start(2025, 1), month_start(2026, 1)),
            "water defaults: the yearly row reports the whole year 2025: {row}"
        );
        assert_eq!(
            value_keys(row, WATER_VALUES),
            owned(sustainability::DEFAULT_WATER_ALLOCATION_TYPES),
            "water defaults: an absent AllocationTypes reports the modelled allocation type: {row}"
        );
    }

    // An absent MaxResults is the modelled default of 1000 — more rows than this request
    // has, so the answer is one page with no token.
    let body = request(start, end, &[("GroupBy", strings(&["REGION", "SERVICE"]))]);
    let page = emulator.ok(CARBON, &body).await;
    assert_eq!(
        results("absent MaxResults", &page).len(),
        6 * sustainability::SEED_REGIONS.len() * sustainability::SEED_CARBON_SERVICES.len(),
        "absent MaxResults: the default page size holds the whole answer: {page}"
    );
    assert!(
        page.get("NextToken").is_none(),
        "absent MaxResults: the last page carries no NextToken: {page}"
    );
}

/// These operations only report: the same request answers the same way however often it is
/// called, and calling one does not change what another later reports.
#[tokio::test]
async fn reading_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    let carbon = request(
        start,
        end,
        &[
            ("Granularity", json!("MONTHLY")),
            ("GroupBy", strings(&["REGION"])),
            ("MaxResults", json!(5000)),
        ],
    );
    let water = request(
        start,
        end,
        &[
            ("GroupBy", strings(&["SERVICE"])),
            ("MaxResults", json!(5000)),
        ],
    );
    let dimensions = request(
        start,
        end,
        &[
            ("Dimensions", strings(&["REGION", "SERVICE"])),
            ("MaxResults", json!(5000)),
        ],
    );

    let first_carbon = emulator.ok(CARBON, &carbon).await;
    let first_water = emulator.ok(WATER, &water).await;
    let first_dimensions = emulator.ok(CARBON_DIMENSIONS, &dimensions).await;

    // Interleave every operation, including a paged walk, before repeating the reads.
    let _ = drain(&emulator, "interleaved", CARBON, &carbon, 2).await;
    let _ = emulator.ok(WATER_DIMENSIONS, &dimensions).await;

    for (case, path, body, first) in [
        ("carbon", CARBON, &carbon, &first_carbon),
        ("water", WATER, &water, &first_water),
        (
            "carbon dimension values",
            CARBON_DIMENSIONS,
            &dimensions,
            &first_dimensions,
        ),
    ] {
        let again = emulator.ok(path, body).await;
        assert_eq!(
            &again, first,
            "case {case}: a reporting operation must answer identically on a repeated call"
        );
    }
}

/// A token is a position in ONE operation's answer to ONE request, so a token this service
/// did not mint for this very request is refused rather than silently paging other rows.
#[tokio::test]
async fn a_token_is_only_valid_for_the_request_that_minted_it() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    let (all_start, all_end) = whole_dataset();
    let one_row_at_a_time: Vec<(&str, Value)> = vec![
        ("Granularity", json!("MONTHLY")),
        ("GroupBy", strings(&["REGION"])),
        ("MaxResults", json!(1)),
    ];
    let body = request(start, end, &one_row_at_a_time);
    let first = emulator.ok(CARBON, &body).await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("an eighteen-row answer paged one row at a time has a next page")
        .to_string();

    // The request that minted it resumes with it.
    let mut resumed = body.clone();
    resumed
        .as_object_mut()
        .expect("the request body is a JSON object")
        .insert("NextToken".to_string(), json!(token));
    let second = emulator.ok(CARBON, &resumed).await;
    assert_ne!(
        results("resume", &second),
        results("resume", &first),
        "resuming with the token must return the next row, not the first again"
    );

    // Every other use of it is refused.
    let changed = |member: &str, value: Value| -> Value {
        let mut body = resumed.clone();
        body.as_object_mut()
            .expect("the request body is a JSON object")
            .insert(member.to_string(), value);
        body
    };
    let other_granularity = changed("Granularity", json!("YEARLY_CALENDAR"));
    let other_grouping = changed("GroupBy", strings(&["SERVICE"]));
    let other_window = changed(
        "TimePeriod",
        json!({ "Start": all_start, "End": all_end }),
    );
    let other_filter = changed(
        "FilterBy",
        json!({ "Dimensions": { "SERVICE": ["AmazonEC2"] } }),
    );
    let mut other_operation = request(start, end, &one_row_at_a_time);
    other_operation
        .as_object_mut()
        .expect("the request body is a JSON object")
        .insert("NextToken".to_string(), resumed["NextToken"].clone());

    for (case, path, body) in [
        ("another granularity", CARBON, &other_granularity),
        ("another grouping", CARBON, &other_grouping),
        ("another window", CARBON, &other_window),
        ("another filter", CARBON, &other_filter),
        ("another operation", WATER, &other_operation),
    ] {
        emulator.refused(case, path, body).await;
    }
}

/// Requests the service cannot answer are refused with a 400 `ValidationException`, never
/// with an empty 200 and never with a 501.
#[tokio::test]
async fn malformed_requests_are_refused_with_a_validation_exception() {
    let emulator = Emulator::new();
    let (start, end) = first_half_of_2025();
    let monthly: (&str, Value) = ("Granularity", json!("MONTHLY"));

    // (case, path, body)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "End equal to Start",
            CARBON,
            request(start, start, &[("Granularity", json!("MONTHLY"))]),
        ),
        (
            "End before Start",
            CARBON,
            request(end, start, &[("Granularity", json!("MONTHLY"))]),
        ),
        ("End before Start, water", WATER, request(end, start, &[])),
        (
            "End before Start, dimension values",
            CARBON_DIMENSIONS,
            request(end, start, &[("Dimensions", strings(&["REGION"]))]),
        ),
        // MaxResults range 1..5000 (the modelled `MaxResults` shape): one past each bound.
        (
            "MaxResults below the range",
            CARBON,
            request(start, end, &[monthly.clone(), ("MaxResults", json!(0))]),
        ),
        (
            "MaxResults above the range",
            CARBON,
            request(start, end, &[monthly.clone(), ("MaxResults", json!(5001))]),
        ),
        (
            "MaxResults above the range, dimension values",
            WATER_DIMENSIONS,
            request(
                start,
                end,
                &[
                    ("Dimensions", strings(&["REGION"])),
                    ("MaxResults", json!(5001)),
                ],
            ),
        ),
        (
            "a NextToken this service never minted",
            CARBON,
            request(
                start,
                end,
                &[monthly.clone(), ("NextToken", json!("not-a-token"))],
            ),
        ),
        (
            "a NextToken that is valid hex but no cursor",
            CARBON,
            request(
                start,
                end,
                &[monthly.clone(), ("NextToken", json!("6162636465"))],
            ),
        ),
        (
            "an unknown GroupBy dimension",
            CARBON,
            request(
                start,
                end,
                &[monthly.clone(), ("GroupBy", strings(&["AVAILABILITY_ZONE"]))],
            ),
        ),
        (
            "an unknown Dimensions value",
            CARBON_DIMENSIONS,
            request(start, end, &[("Dimensions", strings(&["AVAILABILITY_ZONE"]))]),
        ),
        (
            "an unknown EmissionsTypes value",
            CARBON,
            request(
                start,
                end,
                &[
                    monthly.clone(),
                    ("EmissionsTypes", strings(&["TOTAL_WATER_WITHDRAWALS"])),
                ],
            ),
        ),
        (
            "an unknown AllocationTypes value",
            WATER,
            request(
                start,
                end,
                &[("AllocationTypes", strings(&["TOTAL_LBM_CARBON_EMISSIONS"]))],
            ),
        ),
        (
            "an unknown Granularity",
            CARBON,
            request(start, end, &[("Granularity", json!("DAILY"))]),
        ),
        // "Only YEARLY_CALENDAR time granularity is currently supported for water
        // allocation" — every other modelled granularity is refused on that operation.
        (
            "MONTHLY on water",
            WATER,
            request(start, end, &[("Granularity", json!("MONTHLY"))]),
        ),
        (
            "QUARTERLY_CALENDAR on water",
            WATER,
            request(start, end, &[("Granularity", json!("QUARTERLY_CALENDAR"))]),
        ),
        (
            "QUARTERLY_FISCAL on water",
            WATER,
            request(start, end, &[("Granularity", json!("QUARTERLY_FISCAL"))]),
        ),
        (
            "YEARLY_FISCAL on water",
            WATER,
            request(start, end, &[("Granularity", json!("YEARLY_FISCAL"))]),
        ),
        (
            "an unknown FilterBy dimension",
            CARBON,
            request(
                start,
                end,
                &[
                    monthly.clone(),
                    (
                        "FilterBy",
                        json!({ "Dimensions": { "AVAILABILITY_ZONE": ["use1-az1"] } }),
                    ),
                ],
            ),
        ),
        (
            "a FilterBy dimension with no values",
            CARBON,
            request(
                start,
                end,
                &[
                    monthly.clone(),
                    ("FilterBy", json!({ "Dimensions": { "REGION": [] } })),
                ],
            ),
        ),
        // GranularityConfiguration.FiscalYearStartMonth range 1..12: one past each bound.
        (
            "FiscalYearStartMonth below the range",
            CARBON,
            request(
                start,
                end,
                &[
                    ("Granularity", json!("YEARLY_FISCAL")),
                    (
                        "GranularityConfiguration",
                        json!({ "FiscalYearStartMonth": 0 }),
                    ),
                ],
            ),
        ),
        (
            "FiscalYearStartMonth above the range",
            CARBON,
            request(
                start,
                end,
                &[
                    ("Granularity", json!("YEARLY_FISCAL")),
                    (
                        "GranularityConfiguration",
                        json!({ "FiscalYearStartMonth": 13 }),
                    ),
                ],
            ),
        ),
    ];

    for (case, path, body) in cases {
        emulator.refused(case, path, &body).await;
    }

    // Each published bound itself is accepted, so what the refusals above pin is the bound
    // and not a blanket rejection.
    for (case, path, body) in [
        (
            "MaxResults at the minimum",
            CARBON,
            request(start, end, &[monthly.clone(), ("MaxResults", json!(1))]),
        ),
        (
            "MaxResults at the maximum",
            CARBON,
            request(start, end, &[monthly.clone(), ("MaxResults", json!(5000))]),
        ),
        (
            "YEARLY_CALENDAR on water",
            WATER,
            request(start, end, &[("Granularity", json!("YEARLY_CALENDAR"))]),
        ),
        (
            "FiscalYearStartMonth at the minimum",
            CARBON,
            request(
                start,
                end,
                &[
                    ("Granularity", json!("YEARLY_FISCAL")),
                    (
                        "GranularityConfiguration",
                        json!({ "FiscalYearStartMonth": 1 }),
                    ),
                ],
            ),
        ),
        (
            "FiscalYearStartMonth at the maximum",
            CARBON,
            request(
                start,
                end,
                &[
                    ("Granularity", json!("YEARLY_FISCAL")),
                    (
                        "GranularityConfiguration",
                        json!({ "FiscalYearStartMonth": 12 }),
                    ),
                ],
            ),
        ),
        (
            // A GranularityConfiguration carrying no FiscalYearStartMonth takes the
            // modelled @default of 1, so the fiscal year is the calendar one.
            "GranularityConfiguration with no FiscalYearStartMonth",
            CARBON,
            request(
                start,
                end,
                &[
                    ("Granularity", json!("YEARLY_FISCAL")),
                    ("GranularityConfiguration", json!({})),
                ],
            ),
        ),
    ] {
        let answer = emulator.ok(path, &body).await;
        assert!(
            !results(case, &answer).is_empty(),
            "case {case}: a request at the published bound must be answered: {answer}"
        );
    }
}
