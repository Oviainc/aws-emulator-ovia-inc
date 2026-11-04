// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListCoverage`.
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
//! 2. A detector the account DOES hold is answered 200 with the rows it can reach and
//!    no `nextToken`, for every criterion key, sort key and order the model carries,
//!    and whatever the detector's Runtime Monitoring configuration is. A page that
//!    matches no coverage row carries NO `resources` member at all, so a client can
//!    tell "nothing matched" from "coverage has rows".
//! 3. A `criterionKey`, `attributeName` or `orderBy` the model does not carry is
//!    REFUSED, naming the member at fault: silently ignoring one would answer with
//!    resources the caller asked to exclude. So is a criterion that names no key at
//!    all.
//! 4. `maxResults` outside the modelled range, and a `nextToken` this service did not
//!    issue, are refused.
//!
//! The coverage ROWS themselves are the account's own compute resources, which belong
//! to the services that own them (EC2's instances, Amazon EKS's clusters and the
//! GuardDuty agent add-on on each): a router carrying only this service has nobody to
//! ask, so the answer here is the EMPTY report. That the rows are computed from that
//! inventory — their status, details, filtering, ordering and paging — is asserted on a
//! router carrying every service, in
//! `core/router/tests/all/guardduty_runtime_coverage.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state;

use super::client_rejections::UNUSABLE_INPUT_VALUE;
use super::detector_lifecycle::{
    call, call_in, create_detector, enabled_detector, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The only client error GuardDuty's model binds to an operation, and the error shape
/// restJson1 resolves a refusal to (the `x-amzn-errortype` header).
const MODELED_CLIENT_ERROR: &str = "BadRequestException";

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

/// Every `CoverageSortKey` member (`smithy_query(shape="CoverageSortKey")`).
const SORT_KEYS: [&str; 9] = [
    "ACCOUNT_ID",
    "COVERAGE_STATUS",
    "ISSUE",
    "ADDON_VERSION",
    "UPDATED_AT",
    "CLUSTER_NAME",
    "EKS_CLUSTER_NAME",
    "ECS_CLUSTER_NAME",
    "INSTANCE_ID",
];

/// Every `OrderBy` member (`smithy_query(shape="OrderBy")`).
const ORDERS: [&str; 2] = ["ASC", "DESC"];

/// Call `ListCoverage`.
async fn coverage(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/coverage"),
        Some(body),
    )
    .await
}

/// [`coverage`], failing the test unless the call succeeded.
async fn coverage_ok(router: &Router, detector_id: &str, body: Value, case: &str) -> Value {
    let (status, response) = coverage(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-coverage expected 200, got {status} {response:?}"
    );
    response
}

/// The `resources` rows an answer carries.
///
/// The member is reported only on a page that carries a row: a page matching no
/// coverage row carries no `resources` member at all, so an absent member IS the empty
/// page, and a member that is present must carry at least one row.
fn resources(response: &Value, case: &str) -> Vec<Value> {
    match response.get("resources") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => {
            let rows = value
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a resources list, got {response:?}")
                })
                .clone();
            assert!(
                !rows.is_empty(),
                "case {case:?}: an empty page carries no resources member at all, got {response:?}"
            );
            rows
        }
    }
}

/// Assert one refusal: HTTP 400 under the modelled envelope, with a message stating
/// `expected` — the member at fault where the HANDLER refuses the row, and the whole
/// fixed sentence where the request BOUNDARY does, since a boundary refusal names no
/// member (see `modeled_constraint_refusals.rs`).
fn assert_refusal(case: &str, status: u16, response: &Value, expected: &str) {
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
        message.contains(expected),
        "case {case:?}: expected the message to state {expected:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: a detector this account/Region does not hold.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn coverage_of_a_detector_this_account_does_not_hold_is_refused() {
    let router = new_router();
    let held = enabled_detector(&router).await;

    // Rows span what is wrong with the id: a well-formed id nothing minted, values no
    // detector id can be (too short, not hex, a path-shaped value), and a detector the
    // account DOES hold but in another Region's endpoint.
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
            "us-west-2".to_string(),
        ),
    ];

    for (case, detector_id, region) in rows {
        let (status, response) = call_in(
            &router,
            &region,
            "POST",
            &format!("/detector/{detector_id}/coverage"),
            Some(json!({})),
        )
        .await;
        assert_refusal(&case, status, &response, "detectorId");
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a detector the account holds is answered, with the list present.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn coverage_of_a_held_detector_reports_the_list_it_can_reach() {
    // Rows span the Runtime Monitoring configuration the detector carries: the feature
    // that covers every compute resource type, the older one that covers only the
    // Amazon EKS resources, Runtime Monitoring with GuardDuty's own agent management
    // switched on, and a detector with neither feature enabled. None of them can reach
    // an inventory on a single-service router, so each must answer the empty report
    // rather than refusing or inventing a row.
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

        let response = coverage_ok(&router, &detector_id, json!({}), case).await;
        assert!(
            resources(&response, case).is_empty(),
            "case {case:?}: expected no covered resource to be reachable, got {response:?}"
        );
        assert!(
            response.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a complete report carries no nextToken, got {response:?}"
        );
    }
}

/// Every criterion key, sort key and order the model carries is accepted — on both
/// halves of a `CoverageFilterCondition`, and together with the paging members.
#[tokio::test]
async fn every_modeled_criterion_sort_key_and_order_is_accepted() {
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
            let case = format!("criterionKey {key} with condition {condition}");
            let response = coverage_ok(
                &router,
                &detector_id,
                json!({"filterCriteria": {"filterCriterion": [
                    {"criterionKey": key, "filterCondition": condition},
                ]}}),
                &case,
            )
            .await;
            assert!(
                resources(&response, &case).is_empty(),
                "case {case:?}: expected the empty report, got {response:?}"
            );
        }
    }

    for attribute in SORT_KEYS {
        for order in ORDERS {
            let case = format!("sortCriteria {attribute} {order}");
            let response = coverage_ok(
                &router,
                &detector_id,
                json!({"sortCriteria": {"attributeName": attribute, "orderBy": order}}),
                &case,
            )
            .await;
            assert!(
                resources(&response, &case).is_empty(),
                "case {case:?}: expected the empty report, got {response:?}"
            );
        }
    }

    // Both ends of the modelled `MaxResults` range (1..50), with a criterion and a sort
    // beside them.
    for max_results in [1, 25, 50] {
        let case = format!("maxResults {max_results}");
        let response = coverage_ok(
            &router,
            &detector_id,
            json!({
                "maxResults": max_results,
                "sortCriteria": {"attributeName": "UPDATED_AT", "orderBy": "DESC"},
                "filterCriteria": {"filterCriterion": [
                    {"criterionKey": "RESOURCE_TYPE", "filterCondition": {"equals": ["EC2"]}},
                ]},
            }),
            &case,
        )
        .await;
        assert!(
            resources(&response, &case).is_empty(),
            "case {case:?}: expected the empty report, got {response:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3 + 4: what the report refuses.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_criterion_sort_or_token_this_service_cannot_evaluate_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Rows span WHICH member is wrong and HOW: a key outside the enum, a key in the
    // wrong case, an empty key, a criterion naming no key at all, an order outside the
    // enum, and a token this service never issued. A row the HANDLER refuses names the
    // member a client must fix; a row the request BOUNDARY refuses carries GuardDuty's
    // fixed text for its class, which names none.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a criterionKey outside the enum",
            json!({"filterCriteria": {"filterCriterion": [
                {"criterionKey": "NODE_NAME", "filterCondition": {"equals": ["a"]}},
            ]}}),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE,
        ),
        (
            "a criterionKey in the wrong case",
            json!({"filterCriteria": {"filterCriterion": [
                {"criterionKey": "account_id", "filterCondition": {"equals": ["a"]}},
            ]}}),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE,
        ),
        (
            "an empty criterionKey",
            json!({"filterCriteria": {"filterCriterion": [
                {"criterionKey": "", "filterCondition": {"equals": ["a"]}},
            ]}}),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE,
        ),
        (
            "a criterion that names no key at all",
            json!({"filterCriteria": {"filterCriterion": [
                {"filterCondition": {"equals": ["a"]}},
            ]}}),
            "criterionKey",
        ),
        (
            "an attributeName outside the enum",
            json!({"sortCriteria": {"attributeName": "NODE_NAME"}}),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE,
        ),
        (
            "an attributeName that is a criterion key but not a sort key",
            json!({"sortCriteria": {"attributeName": "RESOURCE_TYPE"}}),
            // `CoverageSortKey` is a different enum from `CoverageFilterCriterionKey`
            // (`smithy_query(shape="CoverageSortKey")`), so a criterion key is
            // outside the sort key's value set and the BOUNDARY refuses it.
            state::UNPROCESSABLE_PAYLOAD_MESSAGE,
        ),
        (
            "an orderBy outside the enum",
            json!({"sortCriteria": {"attributeName": "ACCOUNT_ID", "orderBy": "ASCENDING"}}),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE,
        ),
        (
            "a nextToken that is not one this service issues",
            json!({"nextToken": "not-a-token"}),
            UNUSABLE_INPUT_VALUE,
        ),
        (
            "a nextToken past the end of the report",
            json!({"nextToken": "0"}),
            UNUSABLE_INPUT_VALUE,
        ),
        (
            "maxResults below the modelled minimum of 1",
            json!({"maxResults": 0}),
            state::INVALID_INPUT_PARAMETERS_MESSAGE,
        ),
        (
            "maxResults above the modelled maximum of 50",
            json!({"maxResults": 51}),
            state::INVALID_INPUT_PARAMETERS_MESSAGE,
        ),
    ];

    for (case, body, expected) in rows {
        let (status, response) = coverage(&router, &detector_id, body).await;
        assert_refusal(case, status, &response, expected);
    }
}
