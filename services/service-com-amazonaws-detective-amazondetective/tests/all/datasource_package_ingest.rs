// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the data source package ingest pair:
//! `UpdateDatasourcePackages` (the writer) and `ListDatasourcePackages` (the reader
//! that reads back what it wrote).
//!
//! The rules under test, each asserted over a SET of inputs rather than the one input a
//! recording happened to carry:
//!
//! 1. An accepted `UpdateDatasourcePackages` leaves the behavior graph ingesting
//!    exactly the packages the request names: every named package `STARTED`, and every
//!    package the graph already held that the request does not name `STOPPED` — whatever
//!    set is named, in whatever order, however often a value is repeated.
//! 2. `ListDatasourcePackages` reports, for every package the graph holds, that stored
//!    ingest state and — under `LastIngestStateChange`, keyed by ingest state — the
//!    instant the package entered each state it has been in. A package an update leaves
//!    in the state it was already in keeps the instant it got there, so the report says
//!    when ingest last CHANGED rather than when it was last confirmed.
//! 3. The report pages through every held package exactly once in one stable order,
//!    emitting a `NextToken` only while packages remain, and refuses a token it did not
//!    issue with the modelled `ValidationException`; `MaxResults` outside its modelled
//!    `1..200` range is refused, and the report never changes what a later caller sees.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    DATASOURCE_PACKAGE_STATE_STARTED, DATASOURCE_PACKAGE_STATE_STOPPED,
    MANDATORY_DATASOURCE_PACKAGE, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    assert_iso8601, call, create_graph_ok, emulator, error_code, error_message, REGION,
};

/// Every value the modelled `DatasourcePackage` enum accepts, from
/// `smithy_query(query="shape",shape="DatasourcePackage")`:
/// `["DETECTIVE_CORE", "EKS_AUDIT", "ASFF_SECURITYHUB_FINDING"]`. Listed here in the
/// order a package name sorts, which is the order the report walks them in.
const PACKAGES: [&str; 3] = ["ASFF_SECURITYHUB_FINDING", "DETECTIVE_CORE", "EKS_AUDIT"];

/// `UpdateDatasourcePackages`' URI, from
/// `smithy_query(query="operation",operation="UpdateDatasourcePackages")`.
const UPDATE_URI: &str = "/graph/datasources/update";

/// `ListDatasourcePackages`' URI, from the same model.
const LIST_URI: &str = "/graph/datasources/list";

/// Long enough that two calls around it land on different milliseconds, which is the
/// resolution the modelled `date-time` timestamps carry.
const TICK: Duration = Duration::from_millis(5);

/// `UpdateDatasourcePackages` naming `packages`, which must be accepted.
async fn update_ok(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    packages: &[&str],
) {
    let (status, answered) = call(
        router,
        UPDATE_URI,
        &json!({ "GraphArn": graph_arn, "DatasourcePackages": packages }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 for {packages:?}, got {status} {answered:?}"
    );
}

/// One page of `ListDatasourcePackages`.
async fn list_page(
    router: &MultiServiceRouter,
    graph_arn: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "GraphArn": graph_arn });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    call(router, LIST_URI, &body).await
}

/// Every package `ListDatasourcePackages` reports for `graph_arn`, keyed by package, by
/// following `NextToken` to the end at a page size of `max_results`.
///
/// Asserts rule 3's page invariants on the way: no page is larger than the size asked
/// for, no package is reported twice across the pages, and only a page that leaves
/// packages behind carries a token.
async fn all_packages(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    max_results: Option<i32>,
) -> BTreeMap<String, Value> {
    let mut collected: BTreeMap<String, Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    for page in 0..=PACKAGES.len() + 1 {
        let (status, answered) =
            list_page(router, graph_arn, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page}: expected 200, got {status} {answered:?}"
        );
        let reported = answered
            .get("DatasourcePackages")
            .and_then(Value::as_object)
            .unwrap_or_else(|| {
                panic!("case {case:?}: page {page}: expected DatasourcePackages, got {answered:?}")
            });
        if let Some(limit) = max_results {
            assert!(
                reported.len() <= limit as usize,
                "case {case:?}: page {page}: expected at most {limit} entries, got {}",
                reported.len()
            );
        }
        for (package, detail) in reported {
            let already = collected.insert(package.clone(), detail.clone());
            assert!(
                already.is_none(),
                "case {case:?}: page {page}: {package} was reported twice, second time {detail:?}"
            );
        }
        token = answered
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(ref issued) = token else {
            return collected;
        };
        assert!(
            !reported.is_empty(),
            "case {case:?}: page {page}: an empty page carried the token {issued:?}"
        );
    }
    panic!("case {case:?}: the report never reached a last page, collected {collected:?}");
}

/// The ingest state reported for each held package, in package order.
fn reported_states(reported: &BTreeMap<String, Value>) -> Vec<(String, String)> {
    reported
        .iter()
        .map(|(package, detail)| {
            let state = detail
                .get("DatasourcePackageIngestState")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("{package} carried no DatasourcePackageIngestState: {detail:?}")
                });
            (package.clone(), state.to_string())
        })
        .collect()
}

/// The epoch-millisecond instant `LastIngestStateChange` reports for `state`, or `None`
/// when the package has never been in that state.
fn state_change_millis(case: &str, detail: &Value, state: &str) -> Option<i64> {
    let entry = detail
        .get("LastIngestStateChange")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: expected LastIngestStateChange, got {detail:?}"))
        .get(state)?;
    let rendered = assert_iso8601(
        &format!("{case}: {state} timestamp"),
        entry.get("Timestamp"),
    );
    let parsed = DateTime::from_str(&rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {rendered:?} is not a date-time timestamp: {error}")
    });
    Some(parsed.to_millis().unwrap_or_else(|error| {
        panic!("case {case:?}: {rendered:?} does not convert to millis: {error}")
    }))
}

/// As [`state_change_millis`], for a state the package must have been in.
fn require_state_change_millis(case: &str, detail: &Value, state: &str) -> i64 {
    state_change_millis(case, detail, state).unwrap_or_else(|| {
        panic!("case {case:?}: expected a {state} instant in LastIngestStateChange, got {detail:?}")
    })
}

/// The detail reported for `package`, which the graph must hold.
fn detail_of<'a>(case: &str, reported: &'a BTreeMap<String, Value>, package: &str) -> &'a Value {
    reported.get(package).unwrap_or_else(|| {
        panic!("case {case:?}: expected the graph to hold {package}, got {reported:?}")
    })
}

/// One row of the update table: the packages a request names, and the ingest state the
/// rule says the graph then holds for each package, in package order.
struct UpdateCase {
    case: &'static str,
    requested: Vec<&'static str>,
    expected: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn an_accepted_update_starts_exactly_the_packages_it_names_and_stops_the_rest() {
    // Rule 1 over a table whose rows differ along WHICH packages are named, HOW MANY of
    // them, the ORDER they are listed in and whether a value repeats — the whole class
    // the rule generalises over. Each row gets its own emulator, and so its own graph:
    // an account administers one behavior graph per Region, so a row's expectation is
    // about its own request and nothing a row before it did; the sequence across
    // updates is the next test. A fresh graph ingests only the mandatory package, so
    // every state the row does not name is the stored STARTED/STOPPED the rule derives.
    let started = DATASOURCE_PACKAGE_STATE_STARTED;
    let stopped = DATASOURCE_PACKAGE_STATE_STOPPED;
    let cases = vec![
        UpdateCase {
            case: "mandatory-only",
            requested: vec![MANDATORY_DATASOURCE_PACKAGE],
            expected: vec![("DETECTIVE_CORE", started)],
        },
        UpdateCase {
            case: "mandatory-plus-eks",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT"],
            expected: vec![("DETECTIVE_CORE", started), ("EKS_AUDIT", started)],
        },
        UpdateCase {
            // The one package the issue's recording never cited.
            case: "mandatory-plus-asff",
            requested: vec!["DETECTIVE_CORE", "ASFF_SECURITYHUB_FINDING"],
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", started),
                ("DETECTIVE_CORE", started),
            ],
        },
        UpdateCase {
            case: "every-modelled-package",
            requested: PACKAGES.to_vec(),
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", started),
                ("DETECTIVE_CORE", started),
                ("EKS_AUDIT", started),
            ],
        },
        UpdateCase {
            // Order is not part of the rule: this is the second row's set, listed the
            // other way round.
            case: "reversed-order",
            requested: vec!["EKS_AUDIT", "DETECTIVE_CORE"],
            expected: vec![("DETECTIVE_CORE", started), ("EKS_AUDIT", started)],
        },
        UpdateCase {
            // A set, so a repeated value asks for nothing more than the value does.
            case: "repeated-value",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT", "EKS_AUDIT"],
            expected: vec![("DETECTIVE_CORE", started), ("EKS_AUDIT", started)],
        },
    ];

    for UpdateCase {
        case,
        requested,
        expected,
    } in cases
    {
        let router = emulator();
        let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
        update_ok(&router, case, &graph_arn, &requested).await;
        let reported = all_packages(&router, case, &graph_arn, None).await;
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(package, state)| (package.to_string(), state.to_string()))
            .collect();
        let actual = reported_states(&reported);
        assert_eq!(
            actual, expected,
            "case {case:?}: requested {requested:?}, expected {expected:?}, got {actual:?}"
        );
        // Nothing outside the modelled value set is ever reported.
        for (package, _) in &actual {
            assert!(
                PACKAGES.contains(&package.as_str()),
                "case {case:?}: {package} is not a modelled DatasourcePackage"
            );
        }
        // The report left the graph as it found it, so the next caller sees the same.
        let again = all_packages(&router, case, &graph_arn, None).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second report differed from the first"
        );
        // And the stopped control: a package the row did not name is either absent or
        // stopped, never started.
        for package in PACKAGES {
            if requested.contains(&package) {
                continue;
            }
            if let Some(detail) = reported.get(package) {
                let state = detail
                    .get("DatasourcePackageIngestState")
                    .and_then(Value::as_str);
                assert_eq!(
                    state,
                    Some(stopped),
                    "case {case:?}: {package} was not named, so it must not be started"
                );
            }
        }
    }
}

#[tokio::test]
async fn last_ingest_state_change_records_when_each_state_was_entered_and_nothing_else() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // A new graph ingests the mandatory package, and the instant it entered STARTED is
    // the only one it has been in.
    let case = "created";
    let created = all_packages(&router, case, &graph_arn, None).await;
    assert_eq!(
        reported_states(&created),
        vec![(
            MANDATORY_DATASOURCE_PACKAGE.to_string(),
            DATASOURCE_PACKAGE_STATE_STARTED.to_string()
        )],
        "case {case:?}: a new graph ingests only the mandatory package, got {created:?}"
    );
    let core_started = require_state_change_millis(
        case,
        detail_of(case, &created, MANDATORY_DATASOURCE_PACKAGE),
        DATASOURCE_PACKAGE_STATE_STARTED,
    );
    assert_eq!(
        state_change_millis(
            case,
            detail_of(case, &created, MANDATORY_DATASOURCE_PACKAGE),
            DATASOURCE_PACKAGE_STATE_STOPPED
        ),
        None,
        "case {case:?}: a package that was never stopped reports no STOPPED instant"
    );

    // Starting the two optional packages records when each entered STARTED, and leaves
    // the mandatory package's own instant alone: its state did not change.
    tokio::time::sleep(TICK).await;
    let case = "started-all";
    update_ok(&router, case, &graph_arn, &PACKAGES).await;
    let started_all = all_packages(&router, case, &graph_arn, None).await;
    let mut optional_started: BTreeMap<&str, i64> = BTreeMap::new();
    for package in PACKAGES {
        let detail = detail_of(case, &started_all, package);
        let entered =
            require_state_change_millis(case, detail, DATASOURCE_PACKAGE_STATE_STARTED);
        if package == MANDATORY_DATASOURCE_PACKAGE {
            assert_eq!(
                entered, core_started,
                "case {case:?}: {package} was already started, so its instant must not move"
            );
        } else {
            assert!(
                entered > core_started,
                "case {case:?}: {package} started after the graph was created, \
                 got {entered} against {core_started}"
            );
            optional_started.insert(package, entered);
        }
    }

    // An update that confirms the same set changes no state, so no instant moves.
    tokio::time::sleep(TICK).await;
    let case = "confirmed-same-set";
    update_ok(&router, case, &graph_arn, &PACKAGES).await;
    let confirmed = all_packages(&router, case, &graph_arn, None).await;
    assert_eq!(
        confirmed, started_all,
        "case {case:?}: confirming the set already ingested changed the report"
    );

    // Narrowing the set stops the packages it leaves out: each one keeps the instant it
    // entered STARTED and gains the instant it entered STOPPED.
    tokio::time::sleep(TICK).await;
    let case = "narrowed-to-mandatory";
    update_ok(
        &router,
        case,
        &graph_arn,
        &[MANDATORY_DATASOURCE_PACKAGE],
    )
    .await;
    let narrowed = all_packages(&router, case, &graph_arn, None).await;
    for (package, entered) in &optional_started {
        let detail = detail_of(case, &narrowed, package);
        assert_eq!(
            detail
                .get("DatasourcePackageIngestState")
                .and_then(Value::as_str),
            Some(DATASOURCE_PACKAGE_STATE_STOPPED),
            "case {case:?}: {package} was left out, so it must report STOPPED, got {detail:?}"
        );
        assert_eq!(
            require_state_change_millis(case, detail, DATASOURCE_PACKAGE_STATE_STARTED),
            *entered,
            "case {case:?}: {package} must keep the instant it entered STARTED"
        );
        let stopped_at =
            require_state_change_millis(case, detail, DATASOURCE_PACKAGE_STATE_STOPPED);
        assert!(
            stopped_at > *entered,
            "case {case:?}: {package} stopped after it started, got {stopped_at} against {entered}"
        );
    }

    // Starting a stopped package again moves only its STARTED instant forward.
    tokio::time::sleep(TICK).await;
    let case = "restarted-eks";
    let restarted_from = require_state_change_millis(
        case,
        detail_of(case, &narrowed, "EKS_AUDIT"),
        DATASOURCE_PACKAGE_STATE_STOPPED,
    );
    update_ok(
        &router,
        case,
        &graph_arn,
        &[MANDATORY_DATASOURCE_PACKAGE, "EKS_AUDIT"],
    )
    .await;
    let restarted = all_packages(&router, case, &graph_arn, None).await;
    let detail = detail_of(case, &restarted, "EKS_AUDIT");
    assert_eq!(
        detail
            .get("DatasourcePackageIngestState")
            .and_then(Value::as_str),
        Some(DATASOURCE_PACKAGE_STATE_STARTED),
        "case {case:?}: EKS_AUDIT was named again, so it must report STARTED, got {detail:?}"
    );
    let started_again =
        require_state_change_millis(case, detail, DATASOURCE_PACKAGE_STATE_STARTED);
    assert!(
        started_again > restarted_from,
        "case {case:?}: EKS_AUDIT re-entered STARTED after it stopped, \
         got {started_again} against {restarted_from}"
    );
    assert_eq!(
        require_state_change_millis(case, detail, DATASOURCE_PACKAGE_STATE_STOPPED),
        restarted_from,
        "case {case:?}: the instant EKS_AUDIT last stopped must not move"
    );
}

#[tokio::test]
async fn the_report_pages_through_every_held_package_at_every_page_size() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    update_ok(&router, "seed", &graph_arn, &PACKAGES).await;
    let expected: BTreeSet<String> = PACKAGES.iter().map(|p| (*p).to_string()).collect();

    // The rows differ along the page size: each published bound of the modelled
    // `MemberResultsLimit` range (1..200), a size that splits the held packages, one
    // that exactly covers them, and the request that names none at all.
    for max_results in [None, Some(1), Some(2), Some(3), Some(MAX_PAGE_SIZE as i32)] {
        let case = format!("max-results-{max_results:?}");
        let reported = all_packages(&router, &case, &graph_arn, max_results).await;
        let keys: BTreeSet<String> = reported.keys().cloned().collect();
        assert_eq!(
            keys, expected,
            "case {case:?}: expected every held package exactly once, got {keys:?}"
        );
    }

    // One past each published bound is refused by the modelled range.
    for max_results in [0, -1, MAX_PAGE_SIZE as i32 + 1] {
        let case = format!("out-of-range-{max_results}");
        let (status, answered) = list_page(&router, &graph_arn, Some(max_results), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_pagination_token_the_report_did_not_issue_is_refused() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    update_ok(&router, "seed", &graph_arn, &PACKAGES).await;

    // The rows differ along HOW the token is wrong: not this service's encoding at all,
    // this encoding wrapping something that is not a package, and a token for a
    // different list operation's key space (a graph ARN).
    let cases = [
        ("not-this-encoding", "not-a-token".to_string()),
        ("encodes-a-non-package", hex::encode("NOT_A_PACKAGE")),
        ("encodes-a-graph-arn", hex::encode(graph_arn.as_bytes())),
        (
            "encodes-a-lowercase-package",
            hex::encode("detective_core"),
        ),
    ];
    for (case, token) in cases {
        let (status, answered) = list_page(&router, &graph_arn, None, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        let message = error_message(&answered);
        assert!(
            !message.contains(&token),
            "case {case:?}: the refusal must not echo the token, got {message:?}"
        );
    }

    // The control: a token the report DID issue is accepted and names the rest.
    let (status, first) = list_page(&router, &graph_arn, Some(1), None).await;
    assert_eq!(status, 200, "first page: got {status} {first:?}");
    let issued = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the first of three pages carried no NextToken: {first:?}"));
    let (status, second) = list_page(&router, &graph_arn, Some(2), Some(issued)).await;
    assert_eq!(status, 200, "second page: got {status} {second:?}");
    let keys: BTreeSet<String> = second
        .get("DatasourcePackages")
        .and_then(Value::as_object)
        .expect("DatasourcePackages")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["DETECTIVE_CORE".to_string(), "EKS_AUDIT".to_string()]),
        "the token must name the packages the first page did not report, got {keys:?}"
    );
}
