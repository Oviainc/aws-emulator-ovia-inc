// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetCoverageStatistics`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handler are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A `detectorId` this account/Region does not hold is refused with the modelled
//!    `BadRequestException` envelope, never answered with a hollow 200.
//! 2. The answer carries ONE map per `CoverageStatisticsType` the request names, and no
//!    map it does not name: both members of `CoverageStatistics` are optional, so a
//!    statistic nobody asked for is absent rather than reported.
//! 3. Each map counts the covered resources the request's `filterCriteria` admit, with
//!    one entry per group at least one of them falls into — so where no covered
//!    resource is reachable the map is present and EMPTY rather than carrying invented
//!    or zero-filled keys.
//! 4. A `statisticsType` or `criterionKey` this service's model does not carry is
//!    REFUSED, naming the member at fault: silently ignoring one would answer with a
//!    count over resources the caller asked to exclude, or with a missing map a client
//!    would read as "no resources".
//!
//! The counted ROWS themselves are the account's own compute resources, which belong to
//! the services that own them (EC2's instances, Amazon EKS's clusters and the GuardDuty
//! agent add-on on each): a router carrying only this service has nobody to ask, so the
//! counts here are over the EMPTY inventory. That the counts are computed from that
//! inventory — and that they agree, group for group, with the rows `ListCoverage`
//! reports — is asserted on a router carrying every service, in
//! `core/router/tests/all/guardduty_runtime_coverage.rs`.

use serde_json::{json, Value};

use super::detector_lifecycle::{
    call, call_in, create_detector, enabled_detector, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The only client error GuardDuty's model binds to an operation, and the error shape
/// restJson1 resolves a refusal to (the `x-amzn-errortype` header).
const MODELED_CLIENT_ERROR: &str = "BadRequestException";

/// Every `CoverageStatisticsType` member
/// (`smithy_query(shape="CoverageStatisticsType")`), with the `CoverageStatistics`
/// member each one fills (`smithy_query(shape="CoverageStatistics")`).
const STATISTICS: [(&str, &str); 2] = [
    ("COUNT_BY_RESOURCE_TYPE", "countByResourceType"),
    ("COUNT_BY_COVERAGE_STATUS", "countByCoverageStatus"),
];

/// Every `CoverageFilterCriterionKey` member
/// (`smithy_query(shape="CoverageFilterCriterionKey")`).
const CRITERION_KEYS: [&str; 11] = [
    "ACCOUNT_ID",
    "RESOURCE_TYPE",
    "COVERAGE_STATUS",
    "ADDON_VERSION",
    "CLUSTER_NAME",
    "ECS_CLUSTER_NAME",
    "MANAGEMENT_TYPE",
    "EKS_CLUSTER_NAME",
    "AGENT_VERSION",
    "INSTANCE_ID",
    "CLUSTER_ARN",
];

/// Call `GetCoverageStatistics`.
async fn statistics(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/coverage/statistics"),
        Some(body),
    )
    .await
}

/// The `coverageStatistics` object an answer carries, failing the test unless the call
/// succeeded and reported one.
async fn statistics_ok(router: &Router, detector_id: &str, body: Value, case: &str) -> Value {
    let (status, response) = statistics(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-coverage-statistics expected 200, got {status} {response:?}"
    );
    let reported = response["coverageStatistics"].clone();
    assert!(
        reported.is_object(),
        "case {case:?}: expected a coverageStatistics object, got {response:?}"
    );
    reported
}

/// Assert one refusal: HTTP 400 under the modelled envelope, with a message naming the
/// member at fault.
fn assert_refusal(case: &str, status: u16, response: &Value, member: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {response:?}"
    );
    assert_eq!(
        error_shape(response),
        MODELED_CLIENT_ERROR,
        "case {case:?}: expected the modelled error shape, got {response:?}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(member),
        "case {case:?}: expected the message to name {member:?}, got {message:?}"
    );
}

/// Assert that `reported` carries exactly the maps `named` asks for, each of them
/// present and counting nothing (this router can reach no inventory), and that every
/// other map is absent.
fn assert_maps(case: &str, reported: &Value, named: &[&str]) {
    for (kind, member) in STATISTICS {
        if named.contains(&kind) {
            let map = reported[member].as_object().unwrap_or_else(|| {
                panic!("case {case:?}: expected the map {member} to be present, got {reported:?}")
            });
            assert!(
                map.is_empty(),
                "case {case:?}: no covered resource is reachable, so {member} counts none, got {reported:?}"
            );
        } else {
            assert_eq!(
                reported[member],
                Value::Null,
                "case {case:?}: expected {member} to be absent, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1: a detector this account/Region does not hold.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn statistics_for_a_detector_this_account_does_not_hold_are_refused() {
    let router = new_router();
    let held = enabled_detector(&router).await;

    // Rows span what is wrong with the id: a well-formed id nothing minted, values no
    // detector id can be (too short, not hex), and a detector the account DOES hold
    // but in another Region's endpoint.
    let rows: Vec<(String, String, String)> = vec![
        (
            "a well-formed detector id nothing minted".to_string(),
            ABSENT_DETECTOR_ID.to_string(),
            "us-east-1".to_string(),
        ),
        (
            "a value no detector id can be: too short".to_string(),
            "abc".to_string(),
            "us-east-1".to_string(),
        ),
        (
            "a value no detector id can be: not hexadecimal".to_string(),
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string(),
            "us-east-1".to_string(),
        ),
        (
            "a detector this account holds, asked for in another Region".to_string(),
            held.clone(),
            "eu-west-1".to_string(),
        ),
    ];

    for (case, detector_id, region) in rows {
        let (status, response) = call_in(
            &router,
            &region,
            "POST",
            &format!("/detector/{detector_id}/coverage/statistics"),
            Some(json!({"statisticsType": ["COUNT_BY_RESOURCE_TYPE"]})),
        )
        .await;
        assert_refusal(&case, status, &response, "detectorId");
    }
}

// ---------------------------------------------------------------------------
// Rules 2 + 3: the answer carries exactly the statistics the request named.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_answer_carries_one_map_per_named_statistics_type_and_no_other() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Rows span which statistics the request names and how: each one alone, both in
    // either order, one named twice, and none at all.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("neither statistic", vec![]),
        ("the resource-type count", vec!["COUNT_BY_RESOURCE_TYPE"]),
        ("the coverage-status count", vec!["COUNT_BY_COVERAGE_STATUS"]),
        (
            "both, resource type first",
            vec!["COUNT_BY_RESOURCE_TYPE", "COUNT_BY_COVERAGE_STATUS"],
        ),
        (
            "both, coverage status first",
            vec!["COUNT_BY_COVERAGE_STATUS", "COUNT_BY_RESOURCE_TYPE"],
        ),
        (
            "one of them named twice",
            vec!["COUNT_BY_RESOURCE_TYPE", "COUNT_BY_RESOURCE_TYPE"],
        ),
    ];

    for (case, named) in rows {
        let reported = statistics_ok(
            &router,
            &detector_id,
            json!({"statisticsType": named}),
            case,
        )
        .await;
        assert_maps(case, &reported, &named);
    }
}

/// The statistics are reported whatever the detector's Runtime Monitoring configuration
/// is: a configuration that covers no resource type counts nothing, which is the empty
/// map and not a refusal.
#[tokio::test]
async fn every_runtime_monitoring_configuration_is_counted() {
    // Rows span the Runtime Monitoring configuration the detector carries: the feature
    // that covers every compute resource type, the older one that covers only the
    // Amazon EKS resources, Runtime Monitoring with GuardDuty's own agent management
    // switched on, and a detector with neither feature enabled.
    let rows: Vec<(&str, Value)> = vec![
        ("no features named at all", json!({"enable": true})),
        (
            "RUNTIME_MONITORING enabled",
            json!({"enable": true, "features": [{"name": "RUNTIME_MONITORING", "status": "ENABLED"}]}),
        ),
        (
            "RUNTIME_MONITORING enabled with GuardDuty-managed agents",
            json!({"enable": true, "features": [{
                "name": "RUNTIME_MONITORING",
                "status": "ENABLED",
                "additionalConfiguration": [
                    {"name": "EC2_AGENT_MANAGEMENT", "status": "ENABLED"},
                    {"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"},
                ],
            }]}),
        ),
        (
            "EKS_RUNTIME_MONITORING enabled",
            json!({"enable": true, "features": [{"name": "EKS_RUNTIME_MONITORING", "status": "ENABLED"}]}),
        ),
        (
            "RUNTIME_MONITORING disabled",
            json!({"enable": true, "features": [{"name": "RUNTIME_MONITORING", "status": "DISABLED"}]}),
        ),
    ];

    let both: Vec<&str> = STATISTICS.iter().map(|(kind, _)| *kind).collect();
    for (case, create_body) in rows {
        // One detector per account per Region, so each row gets a router of its own.
        let router = new_router();
        let (status, created) = create_detector(&router, create_body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-detector expected 200, got {status} {created:?}"
        );
        let detector_id = created["detectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {created:?}"))
            .to_string();

        let reported = statistics_ok(
            &router,
            &detector_id,
            json!({"statisticsType": both}),
            case,
        )
        .await;
        assert_maps(case, &reported, &both);
    }
}

/// Every criterion key the model carries is accepted, on both halves of a
/// `CoverageFilterCondition` and beside either statistic.
#[tokio::test]
async fn every_modeled_criterion_key_is_accepted() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for key in CRITERION_KEYS {
        for condition in [
            json!({"equals": ["a-value"]}),
            json!({"notEquals": ["a-value"]}),
            json!({"equals": ["a-value", "another-value"], "notEquals": ["a-third"]}),
            // A condition that lists no value restricts nothing.
            json!({}),
        ] {
            for (kind, _) in STATISTICS {
                let case = format!("criterionKey {key} with condition {condition} under {kind}");
                let reported = statistics_ok(
                    &router,
                    &detector_id,
                    json!({
                        "statisticsType": [kind],
                        "filterCriteria": {"filterCriterion": [
                            {"criterionKey": key, "filterCondition": condition},
                        ]},
                    }),
                    &case,
                )
                .await;
                assert_maps(&case, &reported, &[kind]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: what the statistics refuse.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_statistics_type_or_criterion_this_service_cannot_evaluate_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Rows span WHICH member is wrong and HOW: a statistics type outside the enum, in
    // the wrong case, empty, one beside a valid one, the member left out altogether, a
    // criterion key outside the enum or empty, and a criterion naming no key at all.
    // Each row names the member a client must fix.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a statisticsType outside the enum",
            json!({"statisticsType": ["COUNT_BY_ACCOUNT"]}),
            "statisticsType",
        ),
        (
            "a statisticsType in the wrong case",
            json!({"statisticsType": ["count_by_resource_type"]}),
            "statisticsType",
        ),
        (
            "an empty statisticsType value",
            json!({"statisticsType": [""]}),
            "statisticsType",
        ),
        (
            "an unknown statisticsType beside a known one",
            json!({"statisticsType": ["COUNT_BY_RESOURCE_TYPE", "COUNT_BY_NODE"]}),
            "statisticsType",
        ),
        (
            "no statisticsType member at all, which the model requires",
            json!({}),
            "statisticsType",
        ),
        (
            "a criterionKey outside the enum",
            json!({"statisticsType": ["COUNT_BY_RESOURCE_TYPE"], "filterCriteria": {"filterCriterion": [
                {"criterionKey": "NODE_NAME", "filterCondition": {"equals": ["a"]}},
            ]}}),
            "criterionKey",
        ),
        (
            "a criterionKey in the wrong case",
            json!({"statisticsType": ["COUNT_BY_RESOURCE_TYPE"], "filterCriteria": {"filterCriterion": [
                {"criterionKey": "account_id", "filterCondition": {"equals": ["a"]}},
            ]}}),
            "criterionKey",
        ),
        (
            "an empty criterionKey",
            json!({"statisticsType": ["COUNT_BY_RESOURCE_TYPE"], "filterCriteria": {"filterCriterion": [
                {"criterionKey": "", "filterCondition": {"equals": ["a"]}},
            ]}}),
            "criterionKey",
        ),
        (
            "a criterion that names no key at all",
            json!({"statisticsType": ["COUNT_BY_RESOURCE_TYPE"], "filterCriteria": {"filterCriterion": [
                {"filterCondition": {"equals": ["a"]}},
            ]}}),
            "criterionKey",
        ),
        (
            // A sort key is not a criterion key: `ISSUE` orders the list and selects
            // nothing.
            "a criterionKey that is a sort key but not a criterion key",
            json!({"statisticsType": ["COUNT_BY_RESOURCE_TYPE"], "filterCriteria": {"filterCriterion": [
                {"criterionKey": "ISSUE", "filterCondition": {"equals": ["a"]}},
            ]}}),
            "criterionKey",
        ),
    ];

    for (case, body, member) in rows {
        let (status, response) = statistics(&router, &detector_id, body).await;
        assert_refusal(case, status, &response, member);
    }
}
