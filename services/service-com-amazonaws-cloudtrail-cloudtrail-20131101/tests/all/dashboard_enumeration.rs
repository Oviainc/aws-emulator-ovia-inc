// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListDashboards`.
//!
//! Every case goes over the wire through the shared harness
//! (`tests/all/emulator.rs`): an `awsJson1_1` `POST /` with this service's published
//! `X-Amz-Target` prefix, dispatched through the same `MultiServiceRouter` the
//! production router serves, so what is asserted here is what a client sees —
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * `ListDashboards` reports every dashboard the registry holds for the Region the
//!   request is served in, each as its ARN and its type, and no dashboard homed in
//!   another Region; a Region holding none enumerates as a present, empty list;
//! * `NamePrefix` keeps the dashboards whose name STARTS with it, and `Type` the ones
//!   of that type — each published `DashboardType` is a row;
//! * a page holds at most `MaxResults` dashboards and, when more match, a `NextToken`
//!   the next page resumes at, so paging through reports every matching dashboard
//!   exactly once in one stable order, with the filters still applied;
//! * `MaxResults` and `NamePrefix` are modelled constraints, refused at the boundary;
//! * enumerating is a read: it reports the same dashboards twice over and changes
//!   nothing a later `GetDashboard` sees.

use serde_json::{json, Value};
use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::DASHBOARD_TYPE_VALUES;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, Reply, HOME_REGION};

/// A second Region, for the cases that pin the enumeration to one Region.
const OTHER_REGION: &str = "eu-west-1";

/// A third Region, which no case creates a dashboard in.
const EMPTY_REGION: &str = "ap-south-1";

/// A margin on top of [`state::DASHBOARD_SETTLE_AFTER`], so a case that waits for the
/// clock does not race the instant a handler reads.
fn past_settle() -> Duration {
    state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)
}

/// The ARN a dashboard called `name` in `region` carries.
fn arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:dashboard/{name}")
}

/// The type a dashboard called `name` is created as: the one reserved name names the
/// dashboard CloudTrail manages, every other name a custom one.
fn dashboard_type(name: &str) -> &'static str {
    if name == state::HIGHLIGHTS_DASHBOARD_NAME {
        state::DASHBOARD_TYPE_MANAGED
    } else {
        state::DASHBOARD_TYPE_CUSTOM
    }
}

/// Create the dashboard called `name` in `region`, with the refresh schedule the
/// reserved name requires (ENABLED, 6 HOURS) when that is the name being created.
async fn seed(emulator: &Emulator, region: &str, name: &str) {
    let mut request = json!({"Name": name});
    if name == state::HIGHLIGHTS_DASHBOARD_NAME {
        request["RefreshSchedule"] = json!({
            "Frequency": {
                "Unit": state::REFRESH_FREQUENCY_UNIT_HOURS,
                "Value": state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
            },
            "Status": state::REFRESH_SCHEDULE_ENABLED,
        });
    }
    emulator
        .call_in("CreateDashboard", region, &request)
        .await
        .expect_ok(&format!("seeding {name} in {region}"));
}

/// Enumerate the dashboards of `region` with `request` as the `ListDashboards` body.
async fn list(emulator: &Emulator, region: &str, request: Value) -> Reply {
    emulator.call_in("ListDashboards", region, &request).await
}

/// `names`, in the order the registry holds them: a stable total order by name, which
/// is the order a page walks and a page token resumes at.
fn in_registry_order(names: &[&str]) -> Vec<String> {
    let mut ordered: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    ordered.sort();
    ordered
}

/// The `Dashboards` member a page of `names` (in the order given) must report: each
/// dashboard's ARN and its type, which is all `DashboardDetail` models.
fn expected_details(region: &str, names: &[String]) -> Value {
    names
        .iter()
        .map(|name| json!({"DashboardArn": arn(region, name), "Type": dashboard_type(name)}))
        .collect()
}

/// The dashboard names a reply's page reports, read back out of the ARNs.
fn listed_names(body: &Value) -> Vec<String> {
    body["Dashboards"]
        .as_array()
        .map(|page| {
            page.iter()
                .map(|detail| {
                    detail["DashboardArn"]
                        .as_str()
                        .and_then(|value| value.rsplit('/').next())
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Page through `ListDashboards` from `region` with `request` as the first page's body,
/// handing back every page's names in order plus the page sizes seen.
async fn page_through(
    emulator: &Emulator,
    region: &str,
    request: &Value,
    case: &str,
) -> (Vec<String>, Vec<usize>) {
    let mut names: Vec<String> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut body = request.clone();
    // A page always holds at least one dashboard, so this bound cannot cut a correct
    // enumeration short — it only stops a token that never finishes.
    for page in 0..=state::DASHBOARD_QUOTA {
        let reply = list(emulator, region, body.clone()).await;
        let answered = reply.expect_ok(&format!("{case}: page {page}")).clone();
        let page_names = listed_names(&answered);
        sizes.push(page_names.len());
        names.extend(page_names);
        match answered.get("NextToken").and_then(Value::as_str) {
            Some(token) => body["NextToken"] = json!(token),
            None => return (names, sizes),
        }
    }
    panic!("case {case:?}: the enumeration never reported a last page");
}

/// RULE: `ListDashboards` reports every dashboard the registry holds for the Region the
/// request is served in — each as its ARN and its type — and none homed elsewhere; a
/// Region holding no dashboard enumerates as a present, empty list.
#[tokio::test]
async fn every_dashboard_homed_in_the_region_is_reported() {
    let emulator = Emulator::new();
    // Names differing in case, in digits and in separator, including the reserved one,
    // and a name taken in BOTH Regions: two dashboards, one per Region.
    let home: [&str; 5] = [
        "alpha-board",
        "beta_board",
        "123-board",
        "zzz-board",
        state::HIGHLIGHTS_DASHBOARD_NAME,
    ];
    let elsewhere: [&str; 2] = ["alpha-board", "west-only"];
    for name in home {
        seed(&emulator, HOME_REGION, name).await;
    }
    for name in elsewhere {
        seed(&emulator, OTHER_REGION, name).await;
    }

    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "the Region most dashboards were created in",
            HOME_REGION,
            in_registry_order(&home),
        ),
        (
            "another Region with its own dashboards",
            OTHER_REGION,
            in_registry_order(&elsewhere),
        ),
        ("a Region holding no dashboard", EMPTY_REGION, Vec::new()),
    ];
    for (case, region, expected) in rows {
        let reply = list(&emulator, region, json!({})).await;
        let body = reply.expect_ok(case).clone();
        assert_eq!(
            body["Dashboards"],
            expected_details(region, &expected),
            "case {case:?}: expected the dashboards of {region} ({expected:?}), got {body:?}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case:?}: every matching dashboard fit in the page, so no token may be \
             reported, got {body:?}"
        );
    }
}

/// RULE: `NamePrefix` keeps the dashboards whose name STARTS with it — nothing else —
/// and a prefix the model cannot read as a dashboard name is refused at the boundary.
#[tokio::test]
async fn a_name_prefix_keeps_the_dashboards_whose_name_starts_with_it() {
    let emulator = Emulator::new();
    let names: [&str; 5] = [
        "shared-one",
        "shared-two",
        "solo-board",
        "123-board",
        state::HIGHLIGHTS_DASHBOARD_NAME,
    ];
    for name in names {
        seed(&emulator, HOME_REGION, name).await;
    }

    // Prefixes shared by two dashboards, by one, by none, a whole name, a name plus one
    // character, and a string that is INSIDE a name without starting it.
    let prefixes = [
        "sha",
        "shared",
        "shared-",
        "shared-one",
        "shared-onex",
        "sol",
        "123",
        "AWSCloudTrail",
        state::HIGHLIGHTS_DASHBOARD_NAME,
        "zzz",
        "one",
        "board",
    ];
    for prefix in prefixes {
        let expected: Vec<String> = in_registry_order(&names)
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .collect();
        let reply = list(&emulator, HOME_REGION, json!({"NamePrefix": prefix})).await;
        let body = reply.expect_ok(prefix).clone();
        assert_eq!(
            body["Dashboards"],
            expected_details(HOME_REGION, &expected),
            "case {prefix:?}: expected the dashboards whose name starts with it \
             ({expected:?}), got {body:?}"
        );
    }

    // `NamePrefix` is a `DashboardName` (len 3..128, `^[a-zA-Z0-9_\-]+$`), so a value
    // outside those constraints is refused before the handler sees it.
    let refused = [
        ("empty", String::new()),
        ("one character below the minimum length", "ab".to_string()),
        (
            "one character above the maximum length",
            "a".repeat(state::DASHBOARD_NAME_MAX_LEN + 1),
        ),
        ("a character the pattern excludes", "shared one".to_string()),
    ];
    for (case, prefix) in refused {
        let reply = list(&emulator, HOME_REGION, json!({"NamePrefix": prefix})).await;
        reply.expect_error(case, 400, "ValidationException");
    }

    // The maximum length itself is a name the model admits, so it enumerates — to
    // nothing, since no dashboard has that name.
    let longest = "a".repeat(state::DASHBOARD_NAME_MAX_LEN);
    let reply = list(&emulator, HOME_REGION, json!({"NamePrefix": longest})).await;
    assert_eq!(
        reply.expect_ok("a prefix at the maximum name length")["Dashboards"],
        json!([]),
        "case a prefix at the maximum name length: no dashboard has that name, got {reply:?}"
    );
}

/// RULE: `Type` keeps the dashboards of that type, and the types are the model's own
/// `DashboardType` values — the catalog is `ops::types::DASHBOARD_TYPE_VALUES`, so a
/// value added to the model moves this table rather than passing silently.
#[tokio::test]
async fn a_type_filter_keeps_the_dashboards_of_that_type() {
    let emulator = Emulator::new();
    let names: [&str; 3] = [
        "custom-one",
        "custom-two",
        state::HIGHLIGHTS_DASHBOARD_NAME,
    ];
    for name in names {
        seed(&emulator, HOME_REGION, name).await;
    }

    for wanted in DASHBOARD_TYPE_VALUES {
        let expected: Vec<String> = in_registry_order(&names)
            .into_iter()
            .filter(|name| dashboard_type(name) == *wanted)
            .collect();
        assert!(
            !expected.is_empty(),
            "case {wanted:?}: this table needs a dashboard of every published type"
        );
        let reply = list(&emulator, HOME_REGION, json!({"Type": wanted})).await;
        let body = reply.expect_ok(wanted).clone();
        assert_eq!(
            body["Dashboards"],
            expected_details(HOME_REGION, &expected),
            "case {wanted:?}: expected the {wanted} dashboards ({expected:?}), got {body:?}"
        );
    }

    // `Type` is a modelled enum, so a value outside it is refused at the boundary.
    for case in ["custom", "MANAGED ", "HIGHLIGHTS", ""] {
        let reply = list(&emulator, HOME_REGION, json!({"Type": case})).await;
        reply.expect_error(
            &format!("a Type outside {DASHBOARD_TYPE_VALUES:?}: {case:?}"),
            400,
            "ValidationException",
        );
    }

    // A filter that matches nothing is an empty page, not a failure: the type is a
    // published one, but every dashboard here is of the other type.
    let only_managed = Emulator::new();
    seed(&only_managed, HOME_REGION, state::HIGHLIGHTS_DASHBOARD_NAME).await;
    let reply = list(
        &only_managed,
        HOME_REGION,
        json!({"Type": state::DASHBOARD_TYPE_CUSTOM}),
    )
    .await;
    assert_eq!(
        reply.expect_ok("a published type no dashboard has")["Dashboards"],
        json!([]),
        "case a published type no dashboard has: expected an empty page, got {reply:?}"
    );
}

/// RULE: a page holds at most `MaxResults` dashboards and, when more match, a token the
/// next page resumes at — so paging through reports every matching dashboard exactly
/// once, in one order, with the request's filters still applied.
#[tokio::test]
async fn paging_reports_every_matching_dashboard_exactly_once() {
    let emulator = Emulator::new();
    // Created out of order, so the enumeration's order cannot be the insertion order.
    let names: [&str; 7] = [
        "page-4", "page-1", "page-7", "page-2", "page-6", "page-3", "page-5",
    ];
    for name in names {
        seed(&emulator, HOME_REGION, name).await;
    }
    let all = in_registry_order(&names);

    // Page sizes below, at and above the number of matching dashboards, plus both
    // published bounds of `MaxResults` (range 1..1000).
    for size in [1usize, 2, 3, all.len() - 1, all.len(), all.len() + 1, 1000] {
        let case = format!("MaxResults {size}");
        let (paged, page_sizes) =
            page_through(&emulator, HOME_REGION, &json!({"MaxResults": size}), &case).await;
        assert_eq!(
            paged, all,
            "case {case:?}: paging through must report every dashboard exactly once in \
             registry order, got {paged:?}"
        );
        assert!(
            page_sizes.iter().all(|held| *held <= size),
            "case {case:?}: no page may hold more than MaxResults, got {page_sizes:?}"
        );
        // Every page but the last is full: a short page means the enumeration was cut.
        assert!(
            page_sizes
                .iter()
                .take(page_sizes.len().saturating_sub(1))
                .all(|held| *held == size),
            "case {case:?}: only the last page may be short, got {page_sizes:?}"
        );
        assert_eq!(
            page_sizes.len(),
            all.len().div_ceil(size),
            "case {case:?}: wrong number of pages for {} dashboards, got {page_sizes:?}",
            all.len()
        );
    }

    // A token carries on under the same filters: a one-at-a-time walk of a prefix
    // reports that prefix's dashboards and nothing else.
    seed(&emulator, HOME_REGION, "other-board").await;
    let (paged, _) = page_through(
        &emulator,
        HOME_REGION,
        &json!({"MaxResults": 1, "NamePrefix": "page-"}),
        "a filtered one-at-a-time walk",
    )
    .await;
    assert_eq!(
        paged, all,
        "case a filtered one-at-a-time walk: the filter must hold across pages, got \
         {paged:?}"
    );

    // `MaxResults` is a modelled range, so a value outside it is refused at the
    // boundary rather than clamped.
    for size in [0, 1001] {
        let reply = list(&emulator, HOME_REGION, json!({"MaxResults": size})).await;
        reply.expect_error(
            &format!("MaxResults {size}, outside the published range"),
            400,
            "ValidationException",
        );
    }
}

/// RULE: enumerating is a read — it reports the same dashboards however often it is
/// called, and changes nothing a later `GetDashboard` sees.
#[tokio::test]
async fn enumerating_changes_nothing_a_later_read_sees() {
    let emulator = Emulator::new();
    for name in ["read-one", "read-two"] {
        seed(&emulator, HOME_REGION, name).await;
    }
    // Past the settle span, so the dashboard the reads compare is in its terminal
    // status and the comparison is not racing the clock.
    sleep(past_settle());

    let before = emulator
        .call("GetDashboard", &json!({"DashboardId": "read-one"}))
        .await;
    let before = before.expect_ok("the dashboard before any enumeration").clone();

    let first = list(&emulator, HOME_REGION, json!({})).await;
    let first = first.expect_ok("the first enumeration").clone();
    for attempt in 1..4 {
        let again = list(&emulator, HOME_REGION, json!({})).await;
        assert_eq!(
            again.expect_ok("a repeated enumeration"),
            &first,
            "attempt {attempt}: enumerating must report the same dashboards every time, \
             got {again:?}"
        );
    }

    let after = emulator
        .call("GetDashboard", &json!({"DashboardId": "read-one"}))
        .await;
    assert_eq!(
        after.expect_ok("the dashboard after the enumerations"),
        &before,
        "enumerating must not change what a later read sees, got {after:?}"
    );
}
