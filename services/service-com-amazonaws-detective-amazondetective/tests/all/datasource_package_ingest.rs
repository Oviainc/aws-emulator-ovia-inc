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
//!    other package the graph holds `STOPPED` — whatever set is named, in whatever
//!    order, however often a value is repeated. No key is ever removed, so a package
//!    that is not ingesting is reported stopped rather than dropped.
//! 2. `ListDatasourcePackages` reports, for every package the graph holds, that stored
//!    ingest state and — under `LastIngestStateChange`, keyed by ingest state — the
//!    instant the package entered each state it has been in. A package an update leaves
//!    in the state it was already in keeps the instant it got there, so the report says
//!    when ingest last CHANGED rather than when it was last confirmed.
//! 3. The report answers every package the graph holds in ONE response, carrying no
//!    `NextToken` and refusing any `MaxResults` the request names — it is not a
//!    paginated report — and it never changes what a later caller sees.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    DATASOURCE_PACKAGE_STATE_STARTED, DATASOURCE_PACKAGE_STATE_STOPPED,
    MANDATORY_DATASOURCE_PACKAGE, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    assert_iso8601, call, create_graph_ok, emulator, error_code, error_message, millis_of_iso8601,
    REGION,
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

/// Every package `ListDatasourcePackages` reports for `graph_arn`, keyed by package.
///
/// Asserts rule 3's response invariant on the way: the report is NOT paginated, so one
/// call answers everything the graph holds and carries no `NextToken` to follow.
async fn all_packages(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
) -> BTreeMap<String, Value> {
    let (status, answered) = list_page(router, graph_arn, None, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {answered:?}"
    );
    assert_eq!(
        answered.get("NextToken"),
        None,
        "case {case:?}: an unpaginated report issues no NextToken, got {answered:?}"
    );
    answered
        .get("DatasourcePackages")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected DatasourcePackages, got {answered:?}")
        })
        .iter()
        .map(|(package, detail)| (package.clone(), detail.clone()))
        .collect()
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
    Some(millis_of_iso8601(case, &rendered))
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
    // updates is the next test. A graph holds a record for every modelled package from
    // creation, so every row expects all three keys: the ones it names started, the
    // rest stopped.
    let started = DATASOURCE_PACKAGE_STATE_STARTED;
    let stopped = DATASOURCE_PACKAGE_STATE_STOPPED;
    let cases = vec![
        UpdateCase {
            case: "mandatory-only",
            requested: vec![MANDATORY_DATASOURCE_PACKAGE],
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", stopped),
                ("DETECTIVE_CORE", started),
                ("EKS_AUDIT", stopped),
            ],
        },
        UpdateCase {
            case: "mandatory-plus-eks",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT"],
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", stopped),
                ("DETECTIVE_CORE", started),
                ("EKS_AUDIT", started),
            ],
        },
        UpdateCase {
            // The one package the issue's recording never cited.
            case: "mandatory-plus-asff",
            requested: vec!["DETECTIVE_CORE", "ASFF_SECURITYHUB_FINDING"],
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", started),
                ("DETECTIVE_CORE", started),
                ("EKS_AUDIT", stopped),
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
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", stopped),
                ("DETECTIVE_CORE", started),
                ("EKS_AUDIT", started),
            ],
        },
        UpdateCase {
            // A set, so a repeated value asks for nothing more than the value does.
            case: "repeated-value",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT", "EKS_AUDIT"],
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", stopped),
                ("DETECTIVE_CORE", started),
                ("EKS_AUDIT", started),
            ],
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
        let reported = all_packages(&router, case, &graph_arn).await;
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
        let again = all_packages(&router, case, &graph_arn).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second report differed from the first"
        );
        // And the stopped control: a package the row did not name is reported stopped,
        // never started and never dropped.
        for package in PACKAGES {
            if requested.contains(&package) {
                continue;
            }
            let detail = reported.get(package).unwrap_or_else(|| {
                panic!("case {case:?}: {package} was not named, so it must still be reported")
            });
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

#[tokio::test]
async fn last_ingest_state_change_records_when_each_state_was_entered_and_nothing_else() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // A new graph holds a record for every modelled package, each one STARTED, and the
    // instant it entered STARTED is the only one it has been in.
    let case = "created";
    let created = all_packages(&router, case, &graph_arn).await;
    assert_eq!(
        reported_states(&created),
        PACKAGES
            .iter()
            .map(|package| (
                (*package).to_string(),
                DATASOURCE_PACKAGE_STATE_STARTED.to_string()
            ))
            .collect::<Vec<(String, String)>>(),
        "case {case:?}: a new graph ingests every modelled package, got {created:?}"
    );
    let mut started_at: BTreeMap<&str, i64> = BTreeMap::new();
    for package in PACKAGES {
        let detail = detail_of(case, &created, package);
        started_at.insert(
            package,
            require_state_change_millis(case, detail, DATASOURCE_PACKAGE_STATE_STARTED),
        );
        assert_eq!(
            state_change_millis(case, detail, DATASOURCE_PACKAGE_STATE_STOPPED),
            None,
            "case {case:?}: {package} was never stopped, so it reports no STOPPED instant"
        );
    }
    let core_started = started_at[MANDATORY_DATASOURCE_PACKAGE];

    // An update that confirms the set already ingested changes no state, so no instant
    // moves.
    tokio::time::sleep(TICK).await;
    let case = "confirmed-same-set";
    update_ok(&router, case, &graph_arn, &PACKAGES).await;
    let confirmed = all_packages(&router, case, &graph_arn).await;
    assert_eq!(
        confirmed, created,
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
    let narrowed = all_packages(&router, case, &graph_arn).await;
    for (package, entered) in started_at
        .iter()
        .filter(|(package, _)| **package != MANDATORY_DATASOURCE_PACKAGE)
    {
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
    // The package the graph may never switch off kept its state, and so its instant.
    assert_eq!(
        require_state_change_millis(
            case,
            detail_of(case, &narrowed, MANDATORY_DATASOURCE_PACKAGE),
            DATASOURCE_PACKAGE_STATE_STARTED
        ),
        core_started,
        "case {case:?}: the mandatory package was named again, so its instant must not move"
    );

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
    let restarted = all_packages(&router, case, &graph_arn).await;
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
async fn the_report_answers_every_held_package_in_one_response() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    update_ok(&router, "seed", &graph_arn, &PACKAGES).await;
    let expected: BTreeSet<String> = PACKAGES.iter().map(|p| (*p).to_string()).collect();

    let reported = all_packages(&router, "one response", &graph_arn).await;
    let keys: BTreeSet<String> = reported.keys().cloned().collect();
    assert_eq!(
        keys, expected,
        "expected every held package exactly once, got {keys:?}"
    );

    // Any `MaxResults` at all is refused: the rows differ along where the value sits
    // relative to the modelled `MemberResultsLimit` range (1..200) — each published
    // bound, a value inside it, and one past each bound, which the modelled range
    // itself refuses first. Every row is the modelled `ValidationException` at 400.
    for max_results in [1, 2, MAX_PAGE_SIZE as i32, 0, -1, MAX_PAGE_SIZE as i32 + 1] {
        let case = format!("max-results-{max_results}");
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

    // The rows differ along HOW the token is wrong — and since this report issues no
    // token at all, every one of them is a token it did not issue: not this service's
    // encoding at all, this encoding wrapping something that is not a package, a token
    // for a different list operation's key space (a graph ARN), and this encoding
    // wrapping a package name in the wrong case.
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

    // The control: this report issues no token at all, so there is none to accept.
    let (status, first) = list_page(&router, &graph_arn, None, None).await;
    assert_eq!(status, 200, "the unpaginated report: got {status} {first:?}");
    assert_eq!(
        first.get("NextToken"),
        None,
        "the report must issue no NextToken, got {first:?}"
    );
    let keys: BTreeSet<String> = first
        .get("DatasourcePackages")
        .and_then(Value::as_object)
        .expect("DatasourcePackages")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        PACKAGES.iter().map(|p| (*p).to_string()).collect::<BTreeSet<String>>(),
        "one response must carry every held package, got {keys:?}"
    );
}
