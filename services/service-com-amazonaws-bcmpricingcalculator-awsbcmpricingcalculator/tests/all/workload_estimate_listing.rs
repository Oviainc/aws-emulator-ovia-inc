// SPDX-License-Identifier: CC0-1.0

//! `ListWorkloadEstimates`, driven end-to-end over this service's awsJson1_0 wire
//! (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`) through the same
//! `MultiServiceRouter` the production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded request:
//! the rows differ along what the rule generalises over (the number of estimates, the
//! page size, the filter name, its match option and its values, the window bound, and
//! which selection a page token was measured against).
//!
//! The wire harness is the one `bill_scenario_lifecycle` defines; a second copy could
//! drift from the front door that one exercises.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    state, ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState as ServiceState,
};

use crate::bill_scenario_lifecycle::{error_code, Emulator};

/// Names spanning what `WorkloadEstimateName` admits (`len ..64`, pattern
/// `^[a-zA-Z0-9-]+$`): a plain word, hyphenated words sharing a prefix, mixed case and
/// digits.
const NAMES: [&str; 5] = [
    "alpha",
    "alpha-one",
    "alpha-two",
    "BetaTwo9",
    "gamma-delta-epsilon",
];

/// Create every name in `names`, in order, and return their ids in that order.
async fn seed(emulator: &Emulator, names: &[&str]) -> Vec<String> {
    let mut ids = Vec::new();
    for name in names {
        let (status, body) = emulator
            .call("CreateWorkloadEstimate", json!({ "name": name }))
            .await;
        assert_eq!(
            status, 200,
            "seeding {name}: expected 200, got {status} with {body}"
        );
        ids.push(
            body["id"]
                .as_str()
                .unwrap_or_else(|| panic!("seeding {name}: no id in {body}"))
                .to_string(),
        );
    }
    ids
}

/// The `items` of a list response.
fn items(body: &Value) -> Vec<Value> {
    body["items"].as_array().cloned().unwrap_or_default()
}

fn names_of(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: the list reports one summary per LIVE stored estimate, in creation order, and
/// every member of a summary is the one the single-resource reader reports for the same
/// estimate — so a caller cannot tell the two readers apart. A request the service
/// refused created nothing, so it is in neither.
///
/// The table spans the number of estimates in the store, from empty upwards.
#[tokio::test]
async fn the_list_reports_every_stored_estimate_as_the_reader_does() {
    for count in 0..=NAMES.len() {
        let case = format!("count={count}");
        let emulator = Emulator::new();
        let ids = seed(&emulator, &NAMES[..count]).await;

        // A refused create (this name violates the member's `@pattern`) must leave
        // nothing behind for the list to report.
        let (status, refused) = emulator
            .call("CreateWorkloadEstimate", json!({ "name": "bad name!" }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the invalid name to be refused, got {status} with {refused}"
        );

        let (status, listed) = emulator.call("ListWorkloadEstimates", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {listed}"
        );
        assert_eq!(
            names_of(&listed),
            NAMES[..count]
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case}: expected the names in creation order, got {listed}"
        );
        assert!(
            !names_of(&listed).iter().any(|name| name == "bad name!"),
            "case {case}: the refused name reached the list: {listed}"
        );
        assert!(
            listed["nextToken"].is_null(),
            "case {case}: a single page must carry no nextToken, got {listed}"
        );

        for (index, id) in ids.iter().enumerate() {
            let (status, fetched) = emulator
                .call("GetWorkloadEstimate", json!({ "identifier": id }))
                .await;
            assert_eq!(
                status, 200,
                "case {case} index={index}: expected 200 from the reader, got {status} with {fetched}"
            );
            let summary = &items(&listed)[index];
            for field in [
                "id",
                "name",
                "status",
                "createdAt",
                "expiresAt",
                "rateType",
                "rateTimestamp",
                "totalCost",
                "costCurrency",
                "failureMessage",
            ] {
                assert_eq!(
                    summary[field], fetched[field],
                    "case {case} index={index}: expected the listed {field} {:?} to be the reader's {:?}",
                    summary[field], fetched[field]
                );
            }
        }
    }
}

/// RULE: `maxResults` is the page size and `nextToken` is emitted only while items
/// remain, so walking the pages reports every estimate exactly once, in creation order,
/// whatever the page size.
///
/// The table spans the page size, from one item per page to the model's maximum
/// (`MaxResults` is `@range 1..25`), including sizes that do and do not divide the
/// store's size, plus the absent page size.
#[tokio::test]
async fn walking_the_pages_reports_every_estimate_exactly_once() {
    let page_sizes: [Option<i64>; 6] = [None, Some(1), Some(2), Some(3), Some(5), Some(25)];
    let expected: Vec<String> = NAMES.iter().map(|name| (*name).to_string()).collect();

    for size in page_sizes {
        let case = format!("maxResults={size:?}");
        let emulator = Emulator::new();
        seed(&emulator, &NAMES).await;

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more iteration than the smallest page size could need, so a token that
        // never clears fails the test instead of looping forever.
        for page in 0..=NAMES.len() {
            let mut request = json!({});
            if let Some(size) = size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, body) = emulator.call("ListWorkloadEstimates", request).await;
            assert_eq!(
                status, 200,
                "case {case} page={page}: expected 200, got {status} with {body}"
            );
            let page_items = items(&body);
            if let Some(size) = size {
                assert!(
                    i64::try_from(page_items.len()).unwrap_or(i64::MAX) <= size,
                    "case {case} page={page}: expected at most {size} items, got {}",
                    page_items.len()
                );
            }
            seen.extend(names_of(&body));
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            token, None,
            "case {case}: the walk did not reach a last page without a token"
        );
        assert_eq!(
            seen, expected,
            "case {case}: expected every name exactly once in creation order, got {seen:?}"
        );
    }
}

/// One filter row: what it names, how it matches, and the names it must select.
struct FilterCase {
    case: &'static str,
    name: &'static str,
    match_option: Option<&'static str>,
    values: Vec<&'static str>,
    expected: Vec<&'static str>,
}

/// RULE: a filter selects on the estimate attribute its name denotes, under the
/// `matchOption` it carries (absent means an exact match), with the values of one filter
/// OR-ed and several filters AND-ed.
///
/// The table spans both `ListWorkloadEstimatesFilterName` values (smithy_query:
/// `shape="ListWorkloadEstimatesFilterName"` — `STATUS` and `NAME`), every `MatchOption`
/// (`shape="MatchOption"` — `EQUALS`, `STARTS_WITH`, `CONTAINS`) and its absence, one
/// and several values, and a value nothing carries.
#[tokio::test]
async fn a_filter_selects_on_the_attribute_it_names() {
    let cases = vec![
        FilterCase {
            case: "NAME, exact, no matchOption",
            name: "NAME",
            match_option: None,
            values: vec!["alpha-one"],
            expected: vec!["alpha-one"],
        },
        FilterCase {
            case: "NAME, EQUALS",
            name: "NAME",
            match_option: Some("EQUALS"),
            values: vec!["BetaTwo9"],
            expected: vec!["BetaTwo9"],
        },
        FilterCase {
            case: "NAME, EQUALS, several values are OR-ed",
            name: "NAME",
            match_option: Some("EQUALS"),
            values: vec!["alpha", "gamma-delta-epsilon"],
            expected: vec!["alpha", "gamma-delta-epsilon"],
        },
        FilterCase {
            case: "NAME, STARTS_WITH",
            name: "NAME",
            match_option: Some("STARTS_WITH"),
            values: vec!["alpha-"],
            expected: vec!["alpha-one", "alpha-two"],
        },
        FilterCase {
            case: "NAME, CONTAINS",
            name: "NAME",
            match_option: Some("CONTAINS"),
            values: vec!["two"],
            expected: vec!["alpha-two"],
        },
        FilterCase {
            case: "NAME, CONTAINS, case-sensitively",
            name: "NAME",
            match_option: Some("CONTAINS"),
            values: vec!["Two"],
            expected: vec!["BetaTwo9"],
        },
        FilterCase {
            case: "NAME, a value no estimate carries",
            name: "NAME",
            match_option: Some("EQUALS"),
            values: vec!["no-such-estimate"],
            expected: vec![],
        },
        FilterCase {
            case: "STATUS, the status every fresh estimate starts in",
            name: "STATUS",
            match_option: Some("EQUALS"),
            values: vec![state::WORKLOAD_ESTIMATE_STATUS_UPDATING],
            expected: vec!["alpha", "alpha-one", "alpha-two", "BetaTwo9"],
        },
        FilterCase {
            case: "STATUS, the status only the settled estimate reached",
            name: "STATUS",
            match_option: Some("EQUALS"),
            values: vec![state::WORKLOAD_ESTIMATE_STATUS_VALID],
            expected: vec!["gamma-delta-epsilon"],
        },
        FilterCase {
            case: "STATUS, both values are OR-ed",
            name: "STATUS",
            match_option: Some("EQUALS"),
            values: vec![
                state::WORKLOAD_ESTIMATE_STATUS_UPDATING,
                state::WORKLOAD_ESTIMATE_STATUS_VALID,
            ],
            expected: vec![
                "alpha",
                "alpha-one",
                "alpha-two",
                "BetaTwo9",
                "gamma-delta-epsilon",
            ],
        },
    ];

    let emulator = Emulator::new();
    let ids = seed(&emulator, &NAMES).await;
    // The clock owes the LAST estimate its move out of the transient status: its
    // recomputation window is backdated so the settlement is already due, which is what
    // a caller who waited would have seen. Nothing else is touched, so the other four
    // are still `UPDATING` and the STATUS rows above are deterministic.
    {
        let store = emulator.engine.state::<ServiceState>();
        let mut guard = store.lock();
        let settled = ids.last().expect("the store was seeded");
        let estimate = guard
            .workload_estimate_mut(settled)
            .unwrap_or_else(|| panic!("{settled} was not stored"));
        estimate.status_due_at_millis = state::now_millis() - 1;
    }

    for case in &cases {
        let mut filter = json!({ "name": case.name, "values": case.values });
        if let Some(match_option) = case.match_option {
            filter["matchOption"] = json!(match_option);
        }
        let (status, body) = emulator
            .call("ListWorkloadEstimates", json!({ "filters": [filter] }))
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {body}",
            case.case
        );
        assert_eq!(
            names_of(&body),
            case.expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {}: expected {:?}, got {body}",
            case.case,
            case.expected
        );
    }

    // Two filters are AND-ed: the name row and the status row select one estimate
    // between them, and a pair that cannot both hold selects none.
    for (case, status_value, expected) in [
        (
            "NAME and STATUS agreeing",
            state::WORKLOAD_ESTIMATE_STATUS_VALID,
            vec!["gamma-delta-epsilon"],
        ),
        (
            "NAME and STATUS disagreeing",
            state::WORKLOAD_ESTIMATE_STATUS_UPDATING,
            vec![],
        ),
    ] {
        let (status, body) = emulator
            .call(
                "ListWorkloadEstimates",
                json!({
                    "filters": [
                        { "name": "NAME", "values": ["gamma-delta-epsilon"] },
                        { "name": "STATUS", "values": [status_value] },
                    ],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            names_of(&body),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case}: expected {expected:?}, got {body}"
        );
    }
}

/// RULE: a timestamp window keeps only the estimates whose timestamp falls inside it,
/// inclusively at a bound the caller set and openly at one it left unset.
///
/// The table spans which window is filtered, which bound is set, and whether the bound
/// is the estimate's own instant (the inclusive case) or one second off it.
#[tokio::test]
async fn a_timestamp_window_keeps_the_estimates_inside_it() {
    let emulator = Emulator::new();
    seed(&emulator, &["windowed"]).await;

    let (status, listed) = emulator.call("ListWorkloadEstimates", json!({})).await;
    assert_eq!(
        status, 200,
        "seeding the window: expected 200, got {status} with {listed}"
    );
    let created_at = items(&listed)[0]["createdAt"]
        .as_f64()
        .expect("createdAt is a numeric epoch timestamp") as i64;
    let expires_at = items(&listed)[0]["expiresAt"]
        .as_f64()
        .expect("expiresAt is a numeric epoch timestamp") as i64;

    let cases: [(&str, Value, bool); 8] = [
        (
            "createdAt, afterTimestamp at the instant itself",
            json!({ "createdAtFilter": { "afterTimestamp": created_at } }),
            true,
        ),
        (
            "createdAt, afterTimestamp one second later",
            json!({ "createdAtFilter": { "afterTimestamp": created_at + 1 } }),
            false,
        ),
        (
            "createdAt, beforeTimestamp at the instant itself",
            json!({ "createdAtFilter": { "beforeTimestamp": created_at } }),
            true,
        ),
        (
            "createdAt, beforeTimestamp one second earlier",
            json!({ "createdAtFilter": { "beforeTimestamp": created_at - 1 } }),
            false,
        ),
        (
            "createdAt, both bounds around the instant",
            json!({
                "createdAtFilter": {
                    "afterTimestamp": created_at - 60,
                    "beforeTimestamp": created_at + 60,
                },
            }),
            true,
        ),
        (
            "expiresAt, afterTimestamp at the instant itself",
            json!({ "expiresAtFilter": { "afterTimestamp": expires_at } }),
            true,
        ),
        (
            "expiresAt, beforeTimestamp one second earlier",
            json!({ "expiresAtFilter": { "beforeTimestamp": expires_at - 1 } }),
            false,
        ),
        (
            "both windows, each around its own instant",
            json!({
                "createdAtFilter": { "afterTimestamp": created_at - 1 },
                "expiresAtFilter": { "beforeTimestamp": expires_at + 1 },
            }),
            true,
        ),
    ];

    for (case, request, expected_selected) in cases {
        let (status, body) = emulator
            .call("ListWorkloadEstimates", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let selected = names_of(&body);
        let expected: Vec<String> = if expected_selected {
            vec!["windowed".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            selected, expected,
            "case {case} ({request}): expected {expected:?}, got {body}"
        );
    }
}

/// RULE: a `nextToken` is bound to the selection it was measured against, so an offset
/// is never applied to a differently selected list: a token from another request, or one
/// this service never issued, is refused as a `ValidationException` (HTTP 400) rather
/// than silently skipping or repeating items.
///
/// The table spans a token this service never issued, a real token replayed against a
/// different filter set and a different page size, and a token minted by another
/// operation's list.
#[tokio::test]
async fn a_token_measured_against_another_request_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator, &NAMES).await;

    let (status, page) = emulator
        .call("ListWorkloadEstimates", json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a real token: expected 200, got {status} with {page}"
    );
    let real = page["nextToken"]
        .as_str()
        .expect("a first page of five leaves a nextToken")
        .to_string();

    // A token from the OTHER paginated list, measured against a different operation.
    let (status, scenarios) = emulator
        .call("CreateBillScenario", json!({ "name": "scenario-one" }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a bill scenario: expected 200, got {status} with {scenarios}"
    );
    let (status, scenarios) = emulator
        .call("CreateBillScenario", json!({ "name": "scenario-two" }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a second bill scenario: expected 200, got {status} with {scenarios}"
    );
    let (status, scenario_page) = emulator
        .call("ListBillScenarios", json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a foreign token: expected 200, got {status} with {scenario_page}"
    );
    let foreign_operation_token = scenario_page["nextToken"]
        .as_str()
        .expect("a first page of two leaves a nextToken")
        .to_string();

    let cases: [(&str, Value); 5] = [
        (
            "a token this service never issued",
            json!({ "nextToken": "not-a-token" }),
        ),
        (
            "a hex token without this service's prefix",
            json!({ "nextToken": "6f66667365743a30" }),
        ),
        (
            "a real token under a filter it was not measured against",
            json!({
                "maxResults": 1,
                "nextToken": real,
                "filters": [{ "name": "NAME", "values": ["alpha"] }],
            }),
        ),
        (
            "a real token under a window it was not measured against",
            json!({
                "maxResults": 1,
                "nextToken": real,
                "createdAtFilter": { "afterTimestamp": 1 },
            }),
        ),
        (
            "a token minted by another operation's list",
            json!({ "maxResults": 1, "nextToken": foreign_operation_token }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = emulator
            .call("ListWorkloadEstimates", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case} ({request}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({request}): expected ValidationException, got {body}"
        );
    }

    // The same real token still works for the request it WAS measured against.
    let (status, body) = emulator
        .call(
            "ListWorkloadEstimates",
            json!({ "maxResults": 1, "nextToken": real }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the token's own request: expected 200, got {status} with {body}"
    );
    assert_eq!(
        names_of(&body),
        vec![NAMES[1].to_string()],
        "the token's own request: expected the second page, got {body}"
    );
}
