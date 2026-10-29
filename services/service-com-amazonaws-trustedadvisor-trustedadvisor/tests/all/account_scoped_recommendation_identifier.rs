// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT SCOPE of the identifiers the account-level operations take, over
//! HTTP through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rule: **an operation whose input names a Trusted Advisor resource by an
//! ARN whose account segment is an account other than the calling one is
//! answered for that resource first — 403 `AccessDeniedException` naming the
//! refused ARN — whatever the caller's support plan, before the identifier is
//! resolved and before anything is written.** No support plan grants an account
//! access to another account's resources, and the account segment of these
//! identifiers is the caller's to write: `AccountRecommendationIdentifier` is
//! `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$` and
//! `RecommendationResourceArn` is
//! `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation-resource\/[\w-]+\/[\w-]+$`
//! (`smithy_query(query="shape",shape="AccountRecommendationIdentifier,RecommendationResourceArn",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`).
//!
//! So each test below is a TABLE: the rows span every operation that takes one of
//! those identifiers (`GetRecommendation`, `ListRecommendationResources`,
//! `UpdateRecommendationLifecycle`,
//! `BatchUpdateRecommendationResourceExclusion`), several account values and
//! partitions, every support plan AWS offers, and both an identifier the store
//! really holds for that other account and one it holds nothing for. The
//! counterpart tests pin that the caller's OWN identifier still reaches the
//! operation's own behaviour, that a refused write leaves the other account's
//! resource exactly as it was, and that the cross-account view of those same
//! resources — the organization surface, whose identifiers carry no account
//! segment — keeps reporting them.
//!
//! The arbitrary-AWS-resource half of the same rule
//! (`ListRecommendationsForResource`) is pinned in
//! `tests/all/support_level_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::organization_recommendation_arn;
use crate::owning_account_from_arn::{
    enrol_member_account, member_recommendation_arn, member_resource_arn,
};
use crate::recommendation_loop::{call, dispatch, emulator_on_plan, error_code, label};

/// The check whose recommendation the rows below name: its recommendation is
/// `priority`, the one type `UpdateRecommendationLifecycle` can move, so a
/// request that got past the account scope would reach the write. Source: the
/// `SEED` table in `src/state.rs`.
const CHECK: &str = "low-utilization-ec2-instances";

/// A seeded check whose recommendation is `standard`, the type whose resources
/// the exclusion batch applies to. Source: the `SEED` table in `src/state.rs`.
const STANDARD_CHECK: &str = "amazon-ebs-snapshots";

/// The ids of [`CHECK`]'s seeded resources, written out by hand; their source is
/// the `SEED` table in `src/state.rs`. Each enrolled member account holds a copy
/// of every one of them, under its own ARN.
const RESOURCE_IDS: &[&str] = &["res-ec2-0001", "res-ec2-0002", "res-ec2-0003"];

/// One id of [`STANDARD_CHECK`]'s seeded resources, from the same table.
const STANDARD_RESOURCE_ID: &str = "res-ebs-0001";

/// The member account whose copy of [`CHECK`]'s recommendation and resources the
/// store really holds in these tests, and the ones it holds nothing for: the
/// refusal is about the identifier's account segment, so it must not depend on
/// what the identifier resolves to.
const STORED_ACCOUNT: &str = "999988887777";
const UNSTORED_ACCOUNTS: &[&str] = &["111122223333", "000000000012"];

/// The refusal this rule answers with: the status the model binds
/// `AccessDeniedException` to, and the shape's own name, which restJson1 carries
/// both in the body's `__type` and in the `x-amzn-errortype` header.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The path `BatchUpdateRecommendationResourceExclusion` binds.
const BATCH_PATH: &str = "/v1/batch-update-recommendation-resource-exclusion";

/// An `updateReason` the modelled `length 10..4096` admits.
const UPDATE_REASON: &str = "not this caller's account to resolve";

/// A recommendation ARN in `partition` owned by `account`, for one seeded check —
/// the form `AccountRecommendationIdentifier` declares, with the partition left
/// free because the pattern's `[\w-]+` admits any.
fn recommendation_arn_in(partition: &str, account: &str, check_id: &str) -> String {
    format!("arn:{partition}:trustedadvisor::{account}:recommendation/{check_id}-recommendation")
}

/// A recommendation-resource ARN in `partition` owned by `account` — the form
/// `RecommendationResourceArn` declares.
fn resource_arn_in(partition: &str, account: &str, check_id: &str, resource_id: &str) -> String {
    format!(
        "arn:{partition}:trustedadvisor::{account}:recommendation-resource/\
         {check_id}-recommendation/{resource_id}"
    )
}

/// One row of the refusal table: what the row is about, the request to make, and
/// the ARN the refusal must name.
struct Row {
    name: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    refused: String,
}

/// Every operation that takes an account-level Trusted Advisor identifier, at its
/// modelled URI, naming `account`'s copy of the seeded resources in `partition`.
///
/// The last row is a batch that also names the CALLING account's own resource:
/// one entry the caller may not touch refuses the whole request, so nothing beside
/// it is applied either.
fn requests_for(label_of_account: &str, partition: &str, account: &str) -> Vec<Row> {
    let recommendation = recommendation_arn_in(partition, account, CHECK);
    let resource = resource_arn_in(partition, account, CHECK, RESOURCE_IDS[0]);
    let own_resource = resource_arn_in(
        "aws",
        engine::EMULATOR_ACCOUNT_ID,
        STANDARD_CHECK,
        STANDARD_RESOURCE_ID,
    );
    let row = |operation: &str, method, path, body, refused: &String| Row {
        name: format!("{operation} with {label_of_account}"),
        method,
        path,
        body,
        refused: refused.clone(),
    };
    vec![
        row(
            "GetRecommendation",
            "GET",
            format!("/v1/recommendations/{}", label(&recommendation)),
            None,
            &recommendation,
        ),
        row(
            "ListRecommendationResources",
            "GET",
            format!("/v1/recommendations/{}/resources", label(&recommendation)),
            None,
            &recommendation,
        ),
        row(
            "UpdateRecommendationLifecycle",
            "PUT",
            format!("/v1/recommendations/{}/lifecycle", label(&recommendation)),
            Some(json!({ "lifecycleStage": "resolved", "updateReason": UPDATE_REASON })),
            &recommendation,
        ),
        row(
            "BatchUpdateRecommendationResourceExclusion",
            "PUT",
            BATCH_PATH.to_string(),
            Some(json!({
                "recommendationResourceExclusions": [{ "arn": resource, "isExcluded": true }],
            })),
            &resource,
        ),
        row(
            "BatchUpdateRecommendationResourceExclusion beside an own-account entry",
            "PUT",
            BATCH_PATH.to_string(),
            Some(json!({
                "recommendationResourceExclusions": [
                    { "arn": own_resource, "isExcluded": true },
                    { "arn": resource, "isExcluded": true },
                ],
            })),
            &resource,
        ),
    ]
}

/// The account forms the rows span: two unrelated 12-digit ids, one with leading
/// zeros, and one in another partition — every one of them an account other than
/// the caller's.
fn foreign_accounts() -> Vec<(String, &'static str, &'static str)> {
    let mut forms: Vec<(String, &'static str, &'static str)> = vec![(
        format!("the enrolled member account {STORED_ACCOUNT}"),
        "aws",
        STORED_ACCOUNT,
    )];
    for account in UNSTORED_ACCOUNTS {
        forms.push((format!("the account {account}"), "aws", account));
    }
    forms.push((
        format!("the account {STORED_ACCOUNT} in another partition"),
        "aws-us-gov",
        STORED_ACCOUNT,
    ));
    forms
}

/// Assert that one request is refused for the resource it names: the status the
/// model binds `AccessDeniedException` to, the error shape in both places
/// restJson1 carries it, and a message naming the refused ARN.
async fn assert_refused_for_resource(router: &MultiServiceRouter, row: &Row) {
    let Row {
        name,
        method,
        path,
        body,
        refused,
    } = row;
    let (status, errortype, response) = dispatch(router, method, path, body.clone()).await;
    let (expected_status, expected_code) = DENIED;
    assert_eq!(
        status, expected_status,
        "case {name}: expected HTTP {expected_status}, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        expected_code,
        "case {name}: expected __type {expected_code}, got {response}"
    );
    assert_eq!(
        errortype.as_deref(),
        Some(expected_code),
        "case {name}: expected x-amzn-errortype {expected_code}, got {errortype:?}"
    );
    assert_eq!(
        response["message"].as_str(),
        Some(format!("Access denied to resource: {refused}").as_str()),
        "case {name}: expected the refusal to name {refused}, got {response}"
    );
}

/// Read `path`, asserting 200 first, and answer the body.
async fn read(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    body
}

/// The two organization-level views of [`CHECK`]'s member accounts — the
/// cross-account view of the very rows an account-level call may not reach.
async fn organization_views(router: &MultiServiceRouter, case: &str) -> (Value, Value) {
    let identifier = label(&organization_recommendation_arn(CHECK));
    let accounts = read(
        router,
        case,
        &format!("/v1/organization-recommendations/{identifier}/accounts?maxResults=200"),
    )
    .await;
    let resources = read(
        router,
        case,
        &format!("/v1/organization-recommendations/{identifier}/resources?maxResults=200"),
    )
    .await;
    (accounts, resources)
}

#[tokio::test]
async fn an_identifier_in_another_account_is_refused_for_that_resource_on_every_plan() {
    // Every plan AWS offers: the resource refusal does not depend on the support
    // plan, so it answers the unentitled caller and the entitled one alike.
    for plan in SupportPlan::ALL {
        let (router, engine) = emulator_on_plan(*plan);
        // One of the accounts the rows name really is in the store, with its own
        // copy of the recommendation and of every one of its resources; the
        // others are in no store at all.
        enrol_member_account(&engine, CHECK, STORED_ACCOUNT);

        for (label_of_account, partition, account) in foreign_accounts() {
            for row in requests_for(&label_of_account, partition, account) {
                let row = Row {
                    name: format!("{} on {plan:?}", row.name),
                    ..row
                };
                assert_refused_for_resource(&router, &row).await;
            }
        }
    }
}

#[tokio::test]
async fn a_refused_call_leaves_the_other_accounts_resources_as_they_were() {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    enrol_member_account(&engine, CHECK, STORED_ACCOUNT);
    for account in UNSTORED_ACCOUNTS {
        enrol_member_account(&engine, CHECK, account);
    }

    // The cross-account view of every member account's rows, before any refusal.
    let (accounts_before, resources_before) = organization_views(&router, "before").await;

    // Each refused call, made three times: a refusal reads nothing and writes
    // nothing, so it cannot be exhausted and cannot drift the store.
    for attempt in 1..=3 {
        for account in
            std::iter::once(&STORED_ACCOUNT).chain(UNSTORED_ACCOUNTS.iter())
        {
            let member = member_recommendation_arn(account, CHECK);
            let entries: Vec<Value> = RESOURCE_IDS
                .iter()
                .map(|id| json!({ "arn": member_resource_arn(account, CHECK, id), "isExcluded": true }))
                .collect();
            let rows = [
                Row {
                    name: format!("UpdateRecommendationLifecycle for {account}, attempt {attempt}"),
                    method: "PUT",
                    path: format!("/v1/recommendations/{}/lifecycle", label(&member)),
                    body: Some(
                        json!({ "lifecycleStage": "resolved", "updateReason": UPDATE_REASON }),
                    ),
                    refused: member.clone(),
                },
                Row {
                    name: format!("the exclusion batch for {account}, attempt {attempt}"),
                    method: "PUT",
                    path: BATCH_PATH.to_string(),
                    body: Some(json!({ "recommendationResourceExclusions": entries })),
                    refused: member_resource_arn(account, CHECK, RESOURCE_IDS[0]),
                },
            ];
            for row in &rows {
                assert_refused_for_resource(&router, row).await;
            }
        }
    }

    // Not one of the other accounts' rows moved: the same cross-account views
    // report exactly what they reported before, lifecycle members and exclusion
    // statuses included.
    let (accounts_after, resources_after) = organization_views(&router, "after").await;
    assert_eq!(
        accounts_after, accounts_before,
        "case accounts: expected the member accounts' rows to be unchanged by the refused \
         calls, got {accounts_after} where it answered {accounts_before}"
    );
    assert_eq!(
        resources_after, resources_before,
        "case resources: expected the member accounts' resources to be unchanged by the \
         refused calls, got {resources_after} where it answered {resources_before}"
    );

    // And the entry the refused batch named beside the foreign one — the
    // CALLING account's own resource — was not applied either.
    let own = read(
        &router,
        "own resources",
        &format!(
            "/v1/recommendations/{}/resources?maxResults=200",
            label(&recommendation_arn_in(
                "aws",
                engine::EMULATOR_ACCOUNT_ID,
                STANDARD_CHECK
            ))
        ),
    )
    .await;
    let own_resource = resource_arn_in(
        "aws",
        engine::EMULATOR_ACCOUNT_ID,
        STANDARD_CHECK,
        STANDARD_RESOURCE_ID,
    );
    let mixed_batch = json!({
        "recommendationResourceExclusions": [
            { "arn": own_resource, "isExcluded": true },
            {
                "arn": member_resource_arn(STORED_ACCOUNT, CHECK, RESOURCE_IDS[0]),
                "isExcluded": true,
            },
        ],
    });
    assert_refused_for_resource(
        &router,
        &Row {
            name: "a batch naming an own resource beside another account's".to_string(),
            method: "PUT",
            path: BATCH_PATH.to_string(),
            body: Some(mixed_batch),
            refused: member_resource_arn(STORED_ACCOUNT, CHECK, RESOURCE_IDS[0]),
        },
    )
    .await;
    let own_after = read(
        &router,
        "own resources after",
        &format!(
            "/v1/recommendations/{}/resources?maxResults=200",
            label(&recommendation_arn_in(
                "aws",
                engine::EMULATOR_ACCOUNT_ID,
                STANDARD_CHECK
            ))
        ),
    )
    .await;
    assert_eq!(
        own_after, own,
        "case own resources: expected a batch refused for another account's resource to apply \
         none of its entries, got {own_after} where it answered {own}"
    );
}

#[tokio::test]
async fn the_callers_own_identifier_still_reaches_the_operation() {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    // The store also holds other accounts' copies of the same recommendation,
    // so the rows below are answered while foreign rows are present.
    enrol_member_account(&engine, CHECK, STORED_ACCOUNT);

    let own = recommendation_arn_in("aws", engine::EMULATOR_ACCOUNT_ID, CHECK);
    let get_path = format!("/v1/recommendations/{}", label(&own));

    let got = read(&router, "GetRecommendation", &get_path).await;
    assert_eq!(
        got["recommendation"]["arn"].as_str(),
        Some(own.as_str()),
        "case GetRecommendation: expected the caller's own recommendation {own}, got {got}"
    );
    let resources = read(
        &router,
        "ListRecommendationResources",
        &format!("/v1/recommendations/{}/resources?maxResults=200", label(&own)),
    )
    .await;
    assert_eq!(
        resources["recommendationResourceSummaries"]
            .as_array()
            .map(Vec::len),
        Some(RESOURCE_IDS.len()),
        "case ListRecommendationResources: expected the caller's own {} resources, got {resources}",
        RESOURCE_IDS.len()
    );

    // The lifecycle write still applies to the caller's own recommendation, and
    // the next read reports it: rows differ along the stage asked for, including
    // the one that stamps `resolvedAt` and one that clears it again.
    for (stage, resolved) in [
        ("resolved", true),
        ("dismissed", false),
        ("in_progress", false),
    ] {
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/v1/recommendations/{}/lifecycle", label(&own)),
            Some(json!({ "lifecycleStage": stage, "updateReason": UPDATE_REASON })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {stage}: expected the caller's own lifecycle update to be served, got {status} \
             with {body}"
        );
        let after = read(&router, stage, &get_path).await;
        assert_eq!(
            after["recommendation"]["lifecycleStage"].as_str(),
            Some(stage),
            "case {stage}: expected the stage to be read back, got {after}"
        );
        assert_eq!(
            after["recommendation"]["resolvedAt"].is_null(),
            !resolved,
            "case {stage}: expected resolvedAt to be {}, got {after}",
            if resolved { "stamped" } else { "absent" }
        );
    }

    // The exclusion batch still applies to the caller's own resource, read back
    // through `ListRecommendationResources`.
    let own_resource = resource_arn_in(
        "aws",
        engine::EMULATOR_ACCOUNT_ID,
        STANDARD_CHECK,
        STANDARD_RESOURCE_ID,
    );
    let (status, body) = call(
        &router,
        "PUT",
        BATCH_PATH,
        Some(json!({
            "recommendationResourceExclusions": [{ "arn": own_resource, "isExcluded": true }],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the own exclusion batch: expected 200, got {status} with {body}"
    );
    let standard = read(
        &router,
        "own exclusion",
        &format!(
            "/v1/recommendations/{}/resources?maxResults=200",
            label(&recommendation_arn_in(
                "aws",
                engine::EMULATOR_ACCOUNT_ID,
                STANDARD_CHECK
            ))
        ),
    )
    .await;
    let applied = standard["recommendationResourceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no recommendationResourceSummaries in {standard}"))
        .iter()
        .find(|summary| summary["arn"].as_str() == Some(own_resource.as_str()))
        .map(|summary| summary["exclusionStatus"].as_str().unwrap_or_default())
        .unwrap_or_else(|| panic!("the caller's own resource is not reported: {standard}"));
    assert_eq!(
        applied, "excluded",
        "case the own exclusion batch: expected the entry to have been applied, got {standard}"
    );

    // And the account scope does not shadow the support-level gate: the same own
    // identifier from an account whose plan excludes the API is refused with the
    // gate's own sentence, not with a resource one.
    let (unentitled, _engine) = emulator_on_plan(SupportPlan::Basic);
    let (status, _errortype, response) = dispatch(&unentitled, "GET", &get_path, None).await;
    assert_eq!(
        status, DENIED.0,
        "case unentitled: expected HTTP {}, got {status} with {response}",
        DENIED.0
    );
    assert_eq!(
        response["message"].as_str(),
        Some("Access denied due to support level"),
        "case unentitled: expected the support-level sentence, got {response}"
    );
}
