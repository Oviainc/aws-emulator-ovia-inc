// SPDX-License-Identifier: CC0-1.0

//! Wire tests for `ListScheduleGroups`.
//!
//! The rule under test: the operation REPORTS the schedule groups the account
//! actually holds — every stored group plus the implicit `default` group AWS
//! creates with the account — as `ScheduleGroupSummary` in
//! `ScheduleGroupName` order, filtered by `NamePrefix`, cut into pages of at
//! most `MaxResults`, carrying a `NextToken` exactly when a further page is
//! left. Every request goes through the real restJson1 front door
//! (`MultiServiceRouter::dispatch`), so routing, query binding, constraint
//! validation, the handler and response serialization are all exercised.

use smithy_server_runtime::MultiServiceRouter;

use crate::wire_harness::{account_id, call, call_in, error_code, scheduler_router, REGION};

/// Group names spanning the class `ScheduleGroupName` accepts — modelled
/// `length 1..64`, `pattern ^[0-9a-zA-Z-_.]+$` (`smithy_query(query="shape",
/// shape="ScheduleGroupName")`): a leading digit, upper case, lower case, and
/// each punctuation mark the pattern permits (`.`, `_`, `-`).
const CREATED_GROUPS: [&str; 7] = [
    "alpha",
    "alpha.one",
    "alpha_two",
    "beta-1",
    "9lives",
    "Zulu",
    "d.e.f",
];

/// `CREATED_GROUPS` plus the implicit `default` group, in the byte order of
/// their names: digits before upper case before lower case, and `.` before
/// `_` — so `d.e.f` precedes `default`.
const EXPECTED_ORDER: [&str; 8] = [
    "9lives",
    "Zulu",
    "alpha",
    "alpha.one",
    "alpha_two",
    "beta-1",
    "d.e.f",
    "default",
];

/// Create every group in [`CREATED_GROUPS`], in an order that is NOT the order
/// the listing must report, so a handler that echoed insertion order would be
/// visible.
async fn create_groups(router: &MultiServiceRouter) {
    for name in CREATED_GROUPS {
        let (status, body) = call(router, "POST", &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateScheduleGroup got {status} {body}"
        );
    }
}

/// The `ScheduleGroups` member of a `ListScheduleGroups` response.
fn summaries(body: &serde_json::Value) -> &Vec<serde_json::Value> {
    body.get("ScheduleGroups")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("ScheduleGroups is a required output member, got {body}"))
}

/// Just the names, in the order the response listed them.
fn listed_names(body: &serde_json::Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary
                .get("Name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("a summary must carry its Name, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Assert one summary is built from the stored record: the modelled ARN for its
/// name in `region`, an `ACTIVE` `ScheduleGroupState`, and server-set dates.
fn assert_summary_shape(case: &str, summary: &serde_json::Value, region: &str) {
    let name = summary
        .get("Name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a summary must carry its Name, got {summary}"));
    let expected_arn = format!(
        "arn:aws:scheduler:{region}:{}:schedule-group/{name}",
        account_id()
    );
    assert_eq!(
        summary.get("Arn").and_then(serde_json::Value::as_str),
        Some(expected_arn.as_str()),
        "case {case:?}: expected Arn {expected_arn}, got {summary}"
    );
    assert_eq!(
        summary.get("State").and_then(serde_json::Value::as_str),
        Some("ACTIVE"),
        "case {case:?}: expected State ACTIVE, got {summary}"
    );
    let creation = summary
        .get("CreationDate")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: CreationDate absent in {summary}"));
    let modified = summary
        .get("LastModificationDate")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: LastModificationDate absent in {summary}"));
    assert!(
        creation > 0.0 && modified >= creation,
        "case {case:?}: expected a server-set CreationDate <= LastModificationDate, \
         got {creation} and {modified}"
    );
}

#[tokio::test]
async fn list_schedule_groups_reports_persisted_groups_in_name_order() {
    let router = scheduler_router();

    // Before any group is created the account still holds the implicit
    // `default` group, so the listing is never empty.
    let (status, body) = call(&router, "GET", "/schedule-groups", None).await;
    assert_eq!(status, 200, "ListScheduleGroups got {status} {body}");
    assert_eq!(
        listed_names(&body),
        vec!["default".to_string()],
        "case \"no groups created\": expected just the implicit default group, got {body}"
    );
    assert_summary_shape("no groups created", &summaries(&body)[0], REGION);
    assert_eq!(
        body.get("NextToken"),
        None,
        "case \"no groups created\": a complete listing must not carry a NextToken, got {body}"
    );

    create_groups(&router).await;

    let (status, body) = call(&router, "GET", "/schedule-groups", None).await;
    assert_eq!(status, 200, "ListScheduleGroups got {status} {body}");
    assert_eq!(
        listed_names(&body),
        EXPECTED_ORDER
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>(),
        "case \"every group\": expected {EXPECTED_ORDER:?} in name order, got {body}"
    );
    for summary in summaries(&body) {
        assert_summary_shape("every group", summary, REGION);
    }
    assert_eq!(
        body.get("NextToken"),
        None,
        "case \"every group\": 8 groups fit one page, so no NextToken, got {body}"
    );

    // A read reports; it must not change what the next reader sees.
    let (status, again) = call(&router, "GET", "/schedule-groups", None).await;
    assert_eq!(status, 200, "the second ListScheduleGroups got {status}");
    assert_eq!(
        again, body,
        "case \"read twice\": a reporting operation must answer identically, got {again}"
    );
}

#[tokio::test]
async fn list_schedule_groups_filters_by_name_prefix() {
    let router = scheduler_router();
    create_groups(&router).await;

    // One row per class of prefix: a shared stem, a stem plus punctuation, a
    // prefix of the implicit group's name, an exact name, a single character of
    // each case, and a prefix nothing starts with.
    let cases: [(&str, &[&str]); 8] = [
        ("alpha", &["alpha", "alpha.one", "alpha_two"]),
        ("alpha.", &["alpha.one"]),
        ("alpha_", &["alpha_two"]),
        ("d", &["d.e.f", "default"]),
        ("default", &["default"]),
        ("Z", &["Zulu"]),
        ("9", &["9lives"]),
        ("nothing-starts-with-this", &[]),
    ];
    for (prefix, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/schedule-groups?NamePrefix={prefix}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {prefix:?}: got {status} {body}");
        let names = listed_names(&body);
        assert_eq!(
            names,
            expected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {prefix:?}: expected {expected:?}, got {names:?}"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {prefix:?}: a complete page must not carry a NextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn following_next_token_enumerates_every_group_exactly_once() {
    let router = scheduler_router();
    create_groups(&router).await;

    let all: Vec<String> = EXPECTED_ORDER.iter().map(ToString::to_string).collect();
    // Every page size from one group per page up to the modelled maximum
    // (`MaxResults` is `range 1..100`), plus the unparameterised call.
    for max_results in [None, Some(1usize), Some(2), Some(3), Some(5), Some(8), Some(100)] {
        let case = format!("MaxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(max) = max_results {
                query.push(format!("MaxResults={max}"));
            }
            if let Some(token) = &token {
                query.push(format!("NextToken={token}"));
            }
            let uri = if query.is_empty() {
                "/schedule-groups".to_string()
            } else {
                format!("/schedule-groups?{}", query.join("&"))
            };
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(status, 200, "case {case:?}: {uri} got {status} {body}");
            let page = listed_names(&body);
            assert!(
                !page.is_empty() && page.len() <= max_results.unwrap_or(all.len()),
                "case {case:?}: a page must hold between 1 and MaxResults entries, got {page:?}"
            );
            seen.extend(page);
            token = body
                .get("NextToken")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: following NextToken must terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: paging must yield every group once, in name order, got {seen:?}"
        );
    }

    // A token this service did not mint is a bad request, not an empty page.
    for forged in ["zzzz", "00", "6e6f7461746f6b656e"] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/schedule-groups?NextToken={forged}"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {forged:?}: a forged NextToken must be refused, got {status} {body}"
        );
    }

    // A groups token and a schedules token are not interchangeable: each
    // listing refuses the other's position.
    let (_, groups) = call(&router, "GET", "/schedule-groups?MaxResults=1", None).await;
    let groups_token = groups
        .get("NextToken")
        .and_then(serde_json::Value::as_str)
        .expect("a first page of one carries a NextToken")
        .to_string();
    let (status, body) = call(
        &router,
        "GET",
        &format!("/schedules?NextToken={groups_token}"),
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"groups token to ListSchedules\": expected a refusal, got {status} {body}"
    );
}

#[tokio::test]
async fn max_results_is_honoured_at_its_published_bounds() {
    let router = scheduler_router();
    create_groups(&router).await;

    // `ListScheduleGroupsInput$MaxResults` is modelled `range 1..100`: both
    // bounds are accepted and one past each is refused.
    let cases: [(i64, u16); 4] = [(0, 400), (1, 200), (100, 200), (101, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/schedule-groups?MaxResults={max_results}"),
            None,
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected {expected_status}, got {status} {body}"
        );
        if expected_status == 200 {
            let page = listed_names(&body);
            let expected_len = (max_results as usize).min(EXPECTED_ORDER.len());
            assert_eq!(
                page.len(),
                expected_len,
                "case MaxResults={max_results}: expected {expected_len} entries, got {page:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case MaxResults={max_results}: expected a ValidationException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_implicit_default_groups_arn_follows_the_signed_region() {
    let router = scheduler_router();

    // The implicit group belongs to every Region of the account, so its ARN
    // carries the region the reading request was signed for — not a constant.
    for region in ["us-east-1", "eu-west-2", "ap-south-1", "us-gov-west-1"] {
        let (status, body) = call_in(&router, "GET", "/schedule-groups", region, None).await;
        assert_eq!(status, 200, "case {region:?}: got {status} {body}");
        let summary = &summaries(&body)[0];
        assert_summary_shape(region, summary, region);
    }

    // And the date it reports is the account's, stable across reads and
    // Regions — not a fresh clock reading per call.
    let (_, first) = call_in(&router, "GET", "/schedule-groups", "us-east-1", None).await;
    let (_, second) = call_in(&router, "GET", "/schedule-groups", "eu-west-2", None).await;
    assert_eq!(
        summaries(&first)[0].get("CreationDate"),
        summaries(&second)[0].get("CreationDate"),
        "case \"default group CreationDate\": expected one stable date, got {first} and {second}"
    );
}
