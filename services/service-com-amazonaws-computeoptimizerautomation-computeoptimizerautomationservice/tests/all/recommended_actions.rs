// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecommendedActions`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0
//! `POST /` through the same [`MultiServiceRouter`] the emulator binary mounts —
//! so a response that is not computed from persisted state fails here.
//!
//! A recommended action is produced by Compute Optimizer's own analysis of a
//! resource, and no operation of THIS API creates one, so the fixtures below
//! record actions into the store the way that analysis would and then read them
//! back only over the wire.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EbsVolume, EbsVolumeConfiguration, EstimatedMonthlySavings, RecommendedAction, ResourceDetails,
    Tag, RECOMMENDED_ACTION_FILTER_NAME_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    service_state, DEFAULT_MAX_RESULTS,
};

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, is_next_token, member,
};

/// Record `actions` into the service's store — the one store
/// `StartAutomationEvent` resolves an id against — the way Compute Optimizer's
/// analysis would.
pub(crate) fn record(engine: &Arc<engine::InMemoryEngine>, actions: Vec<RecommendedAction>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for action in actions {
        let id = action
            .recommended_action_id
            .clone()
            .expect("a recorded action carries its id");
        guard.recommended_actions.insert(id, action);
    }
}

/// How many actions the store holds.
pub(crate) fn stored_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the engine bears this service's state")
        .lock()
        .recommended_actions
        .len()
}

/// A `RecommendedActionId`, modelled as `^[0-9A-Za-z]{16}$`.
pub(crate) fn action_id(index: usize) -> String {
    format!("acoaction{index:07}")
}

/// An EBS volume's configuration, as the `ResourceDetails` union carries it.
fn ebs_details(volume_type: &str, size_in_gib: i32) -> ResourceDetails {
    ResourceDetails::ebsVolume(EbsVolume {
        configuration: Some(EbsVolumeConfiguration {
            r#type: Some(volume_type.to_string()),
            size_in_gib: Some(size_in_gib),
            iops: Some(3000),
            throughput: Some(125),
        }),
    })
}

/// One recorded recommended action, varying along every dimension
/// `RecommendedActionFilterName` selects on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stored(
    index: usize,
    resource_id: &str,
    account_id: &str,
    region: &str,
    look_back_period_in_days: i32,
    recommended_action_type: &str,
    current_volume_type: Option<&str>,
    restart_needed: bool,
    tags: &[(&str, &str)],
) -> RecommendedAction {
    RecommendedAction {
        recommended_action_id: Some(action_id(index)),
        resource_arn: Some(format!(
            "arn:aws:ec2:{region}:{account_id}:volume/{resource_id}"
        )),
        resource_id: Some(resource_id.to_string()),
        account_id: Some(account_id.to_string()),
        region: Some(region.to_string()),
        // `ResourceType` models exactly one value.
        resource_type: Some("EbsVolume".to_string()),
        look_back_period_in_days: Some(look_back_period_in_days),
        recommended_action_type: Some(recommended_action_type.to_string()),
        current_resource_summary: current_volume_type.map(|kind| format!("{kind} 100 GiB")),
        current_resource_details: current_volume_type.map(|kind| ebs_details(kind, 100)),
        recommended_resource_summary: Some("gp3 100 GiB".to_string()),
        recommended_resource_details: Some(ebs_details("gp3", 100)),
        restart_needed: Some(restart_needed),
        estimated_monthly_savings: Some(EstimatedMonthlySavings {
            currency: "USD".to_string(),
            before_discount_savings: 12.5,
            after_discount_savings: 9.75,
            savings_estimation_mode: "AfterDiscount".to_string(),
        }),
        resource_tags: Some(
            tags.iter()
                .map(|(key, value)| Tag {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
        ),
    }
}

/// The second account these fixtures use, beside the emulator's own.
pub(crate) const MEMBER_ACCOUNT_ID: &str = "210987654321";

/// The four actions the filter and paging cases are taken over, in record order.
fn fixtures() -> Vec<RecommendedAction> {
    vec![
        stored(
            1,
            "vol-aaa1",
            engine::EMULATOR_ACCOUNT_ID,
            "us-east-1",
            14,
            "UpgradeEbsVolumeType",
            Some("gp2"),
            false,
            &[("env", "prod")],
        ),
        stored(
            2,
            "vol-bbb2",
            MEMBER_ACCOUNT_ID,
            "us-west-2",
            32,
            "SnapshotAndDeleteUnattachedEbsVolume",
            Some("gp2"),
            true,
            &[("env", "test"), ("team", "infra")],
        ),
        stored(
            3,
            "vol-ccc3",
            engine::EMULATOR_ACCOUNT_ID,
            "eu-west-1",
            93,
            "UpgradeEbsVolumeType",
            Some("io1"),
            true,
            &[],
        ),
        stored(
            4,
            "vol-ddd4",
            engine::EMULATOR_ACCOUNT_ID,
            "us-east-1",
            14,
            "UpgradeEbsVolumeType",
            None,
            false,
            &[("env", "prod")],
        ),
    ]
}

/// The `recommendedActionId`s one response reports, in the order it reported them.
fn reported_ids(body: &Value) -> Vec<String> {
    member(body, "recommendedActions")
        .as_array()
        .expect("recommendedActions is a list")
        .iter()
        .map(|action| {
            action
                .get("recommendedActionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The empty store
// ---------------------------------------------------------------------------

/// With nothing recorded the list is empty and carries no further page — and
/// reading it records nothing, so a later caller still sees an empty store.
#[tokio::test]
async fn an_empty_store_reports_an_empty_list() {
    let (router, engine) = emulator_with_engine();

    for case in ["no-filters", "with-filters", "with-max-results"] {
        let request = match case {
            "with-filters" => json!({"filters": [{"name": "AccountId", "values": ["210987654321"]}]}),
            "with-max-results" => json!({"maxResults": 1}),
            _ => json!({}),
        };
        let listed = call_ok(&router, "ListRecommendedActions", &request, case).await;
        assert_eq!(
            member(&listed, "recommendedActions"),
            &json!([]),
            "case {case:?}: an empty store yields an empty list, got {listed}"
        );
        assert!(
            listed.get("nextToken").is_none(),
            "case {case:?}: there is no further page, got {listed}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: listing is a read and must record nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

/// Every member of a recorded action is reported from the store, and a member
/// the store does not hold is absent rather than invented.
#[tokio::test]
async fn a_recorded_action_is_reported_member_for_member() {
    let (router, engine) = emulator_with_engine();

    let full = stored(
        1,
        "vol-aaa1",
        engine::EMULATOR_ACCOUNT_ID,
        "us-east-1",
        14,
        "UpgradeEbsVolumeType",
        Some("gp2"),
        true,
        &[("env", "prod"), ("team", "infra")],
    );
    // Everything the model leaves optional left unset.
    let sparse = RecommendedAction {
        current_resource_summary: None,
        current_resource_details: None,
        recommended_resource_summary: None,
        recommended_resource_details: None,
        estimated_monthly_savings: None,
        resource_tags: None,
        ..stored(
            2,
            "vol_bbb.2",
            MEMBER_ACCOUNT_ID,
            "ap-south-1",
            180,
            "SnapshotAndDeleteUnattachedEbsVolume",
            None,
            false,
            &[],
        )
    };
    record(&engine, vec![full, sparse]);

    let expected = [
        (
            "fully-populated",
            json!({
                "recommendedActionId": action_id(1),
                "resourceArn": format!(
                    "arn:aws:ec2:us-east-1:{}:volume/vol-aaa1",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "resourceId": "vol-aaa1",
                "accountId": engine::EMULATOR_ACCOUNT_ID,
                "region": "us-east-1",
                "resourceType": "EbsVolume",
                "lookBackPeriodInDays": 14,
                "recommendedActionType": "UpgradeEbsVolumeType",
                "currentResourceSummary": "gp2 100 GiB",
                "currentResourceDetails": {"ebsVolume": {"configuration": {
                    "type": "gp2", "sizeInGib": 100, "iops": 3000, "throughput": 125
                }}},
                "recommendedResourceSummary": "gp3 100 GiB",
                "recommendedResourceDetails": {"ebsVolume": {"configuration": {
                    "type": "gp3", "sizeInGib": 100, "iops": 3000, "throughput": 125
                }}},
                "restartNeeded": true,
                "estimatedMonthlySavings": {
                    "currency": "USD",
                    "beforeDiscountSavings": 12.5,
                    "afterDiscountSavings": 9.75,
                    "savingsEstimationMode": "AfterDiscount"
                },
                "resourceTags": [
                    {"key": "env", "value": "prod"},
                    {"key": "team", "value": "infra"}
                ]
            }),
        ),
        (
            "sparse",
            json!({
                "recommendedActionId": action_id(2),
                "resourceArn": format!("arn:aws:ec2:ap-south-1:{MEMBER_ACCOUNT_ID}:volume/vol_bbb.2"),
                "resourceId": "vol_bbb.2",
                "accountId": MEMBER_ACCOUNT_ID,
                "region": "ap-south-1",
                "resourceType": "EbsVolume",
                "lookBackPeriodInDays": 180,
                "recommendedActionType": "SnapshotAndDeleteUnattachedEbsVolume",
                "restartNeeded": false
            }),
        ),
    ];

    let listed = call_ok(&router, "ListRecommendedActions", &json!({}), "projection").await;
    let reported = member(&listed, "recommendedActions")
        .as_array()
        .expect("recommendedActions is a list")
        .clone();
    assert_eq!(
        reported.len(),
        expected.len(),
        "case \"projection\": every recorded action is reported once, got {listed}"
    );
    for ((case, want), got) in expected.iter().zip(reported.iter()) {
        assert_eq!(
            got, want,
            "case {case:?}: expected {want}, got {got}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each `RecommendedActionFilterName` selects on the field it names, a filter
/// matches when the field equals ANY of its values, and several filters are
/// ANDed.
#[tokio::test]
async fn filters_select_on_the_field_they_name() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

    let all = vec![action_id(1), action_id(2), action_id(3), action_id(4)];
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "ResourceType-the-only-modelled-value",
            json!([{"name": "ResourceType", "values": ["EbsVolume"]}]),
            all.clone(),
        ),
        (
            "ResourceType-no-match",
            json!([{"name": "ResourceType", "values": ["Ec2Instance"]}]),
            vec![],
        ),
        (
            "RecommendedActionType-upgrade",
            json!([{"name": "RecommendedActionType", "values": ["UpgradeEbsVolumeType"]}]),
            vec![action_id(1), action_id(3), action_id(4)],
        ),
        (
            "RecommendedActionType-both-values",
            json!([{"name": "RecommendedActionType", "values": [
                "UpgradeEbsVolumeType", "SnapshotAndDeleteUnattachedEbsVolume"
            ]}]),
            all.clone(),
        ),
        (
            "ResourceId-one",
            json!([{"name": "ResourceId", "values": ["vol-bbb2"]}]),
            vec![action_id(2)],
        ),
        (
            "ResourceId-two",
            json!([{"name": "ResourceId", "values": ["vol-aaa1", "vol-ccc3"]}]),
            vec![action_id(1), action_id(3)],
        ),
        (
            "AccountId-the-emulator-account",
            json!([{"name": "AccountId", "values": [engine::EMULATOR_ACCOUNT_ID]}]),
            vec![action_id(1), action_id(3), action_id(4)],
        ),
        (
            "AccountId-a-member-account",
            json!([{"name": "AccountId", "values": [MEMBER_ACCOUNT_ID]}]),
            vec![action_id(2)],
        ),
        (
            "LookBackPeriodInDays-fourteen",
            json!([{"name": "LookBackPeriodInDays", "values": ["14"]}]),
            vec![action_id(1), action_id(4)],
        ),
        (
            "LookBackPeriodInDays-two-values",
            json!([{"name": "LookBackPeriodInDays", "values": ["32", "93"]}]),
            vec![action_id(2), action_id(3)],
        ),
        (
            "LookBackPeriodInDays-not-a-number",
            json!([{"name": "LookBackPeriodInDays", "values": ["fourteen"]}]),
            vec![],
        ),
        (
            "CurrentResourceDetailsEbsVolumeType-gp2",
            json!([{"name": "CurrentResourceDetailsEbsVolumeType", "values": ["gp2"]}]),
            vec![action_id(1), action_id(2)],
        ),
        (
            "CurrentResourceDetailsEbsVolumeType-io1",
            json!([{"name": "CurrentResourceDetailsEbsVolumeType", "values": ["io1"]}]),
            vec![action_id(3)],
        ),
        (
            "ResourceTagsKey-env",
            json!([{"name": "ResourceTagsKey", "values": ["env"]}]),
            vec![action_id(1), action_id(2), action_id(4)],
        ),
        (
            "ResourceTagsKey-team",
            json!([{"name": "ResourceTagsKey", "values": ["team"]}]),
            vec![action_id(2)],
        ),
        (
            "ResourceTagsValue-prod",
            json!([{"name": "ResourceTagsValue", "values": ["prod"]}]),
            vec![action_id(1), action_id(4)],
        ),
        (
            "RestartNeeded-true",
            json!([{"name": "RestartNeeded", "values": ["true"]}]),
            vec![action_id(2), action_id(3)],
        ),
        (
            "RestartNeeded-false",
            json!([{"name": "RestartNeeded", "values": ["false"]}]),
            vec![action_id(1), action_id(4)],
        ),
        (
            "RestartNeeded-upper-case",
            json!([{"name": "RestartNeeded", "values": ["TRUE"]}]),
            vec![action_id(2), action_id(3)],
        ),
        (
            "two-filters-are-anded",
            json!([
                {"name": "RecommendedActionType", "values": ["UpgradeEbsVolumeType"]},
                {"name": "RestartNeeded", "values": ["false"]}
            ]),
            vec![action_id(1), action_id(4)],
        ),
        (
            "two-filters-with-no-common-match",
            json!([
                {"name": "AccountId", "values": [engine::EMULATOR_ACCOUNT_ID]},
                {"name": "ResourceTagsKey", "values": ["team"]}
            ]),
            vec![],
        ),
    ];

    // The table spans the whole modelled value set, so a name added to the model
    // cannot be left untested.
    let covered: BTreeSet<String> = cases
        .iter()
        .flat_map(|(_, filters, _)| {
            filters
                .as_array()
                .expect("a filter list")
                .iter()
                .map(|filter| filter["name"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .collect();
    let modelled: BTreeSet<String> = RECOMMENDED_ACTION_FILTER_NAME_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        covered, modelled,
        "every modelled RecommendedActionFilterName must appear in the table"
    );

    for (case, filters, expected) in &cases {
        let listed = call_ok(
            &router,
            "ListRecommendedActions",
            &json!({"filters": filters}),
            case,
        )
        .await;
        assert_eq!(
            &reported_ids(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {listed}"
        );
    }

    // A filter name outside the modelled value set is refused, not ignored, and
    // the refusal reports the member and the accepted set without echoing the
    // rejected value back to the caller.
    let (status, body) = call(
        &router,
        "ListRecommendedActions",
        &json!({"filters": [{"name": "ResourceArn", "values": ["vol-aaa1"]}]}),
    )
    .await;
    assert_refused(
        (status, body.clone()),
        400,
        "InvalidParameterValueException",
        "unknown-filter-name",
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        message.starts_with("1 validation error detected: Value at 'filters")
            && message.contains("member.name")
            && !message.contains("ResourceArn")
            && RECOMMENDED_ACTION_FILTER_NAME_VALUES
                .iter()
                .all(|name| message.contains(name)),
        "case \"unknown-filter-name\": the refusal must name the member and the accepted set \
         without echoing the rejected value, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `ListRecommendedActions` pages at `maxResults` and resumes exactly where the
/// previous page stopped — for every page size from one to one past the number
/// of recorded actions.
#[tokio::test]
async fn pages_over_the_recorded_actions() {
    let (router, engine) = emulator_with_engine();
    let recorded: Vec<String> = fixtures()
        .iter()
        .map(|action| {
            action
                .recommended_action_id
                .clone()
                .expect("a fixture carries its id")
        })
        .collect();
    record(&engine, fixtures());

    for page_size in 1..=recorded.len() + 1 {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = call_ok(&router, "ListRecommendedActions", &request, &case).await;
            let ids = reported_ids(&page);
            let remaining = recorded.len() - walked.len();
            assert_eq!(
                ids.len(),
                remaining.min(page_size),
                "case {case:?}: a page holds min(maxResults, remaining) actions, got {page}"
            );
            walked.extend(ids);

            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if let Some(token) = &token {
                assert!(
                    is_next_token(token),
                    "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
                );
            }
            pages += 1;
            assert!(
                pages <= recorded.len() + 1,
                "case {case:?}: the walk must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, recorded,
            "case {case:?}: the walk must report every action once, in record order"
        );
        assert_eq!(
            pages,
            recorded.len().div_ceil(page_size),
            "case {case:?}: no page beyond the last, and no nextToken on it"
        );
    }

    // A token this operation did not issue, and one issued for other filters,
    // are both refused rather than answered with a page of something else.
    let first = call_ok(
        &router,
        "ListRecommendedActions",
        &json!({"maxResults": 1}),
        "token-reuse",
    )
    .await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of four leaves a further page")
        .to_string();
    assert_refused(
        call(
            &router,
            "ListRecommendedActions",
            &json!({"maxResults": 1, "nextToken": token, "filters": [
                {"name": "RestartNeeded", "values": ["true"]}
            ]}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "token-with-different-filters",
    );
    assert_refused(
        call(
            &router,
            "ListRecommendedActions",
            &json!({"nextToken": "notatokenweissued"}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "foreign-token",
    );
    // A structurally valid cursor whose fingerprint is not the one this request
    // would have issued — what a token from another list operation looks like —
    // is refused too.
    assert_refused(
        call(
            &router,
            "ListRecommendedActions",
            &json!({"nextToken": hex::encode("0:")}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "token-with-a-foreign-fingerprint",
    );

    // `maxResults` carries a member `@range` of min 1 / max 100, which the
    // generated request walk enforces before the handler runs: each bound of that
    // range is served, and a value past either end is refused — including 1000,
    // the ceiling the member's PROSE names, which the range trait overrides.
    for (case, max_results, expected_status) in [
        ("maxResults-at-the-range-minimum", 1, 200),
        ("maxResults-at-the-range-maximum", DEFAULT_MAX_RESULTS, 200),
        ("maxResults-below-the-range-minimum", 0, 400),
        ("maxResults-above-the-range-maximum", DEFAULT_MAX_RESULTS + 1, 400),
        ("maxResults-at-the-documented-ceiling", 1000, 400),
        ("maxResults-negative", -1, 400),
    ] {
        let answered = call(
            &router,
            "ListRecommendedActions",
            &json!({"maxResults": max_results}),
        )
        .await;
        if expected_status == 400 {
            assert_refused(answered, 400, "InvalidParameterValueException", case);
        } else {
            let (status, body) = answered;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
            );
            let page_size = usize::try_from(max_results).expect("a positive page size");
            let expected: Vec<String> = recorded.iter().take(page_size).cloned().collect();
            assert_eq!(
                reported_ids(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

