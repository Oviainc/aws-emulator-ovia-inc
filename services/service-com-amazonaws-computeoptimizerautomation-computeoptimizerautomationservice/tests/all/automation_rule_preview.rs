// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAutomationRulePreview`.
//!
//! The preview reports over the recommended actions Compute Optimizer has
//! DERIVED from observed resources. No operation of this API mints one, so each
//! case seeds that analysis inventory in the shared engine and then drives the
//! service the way a client does — an awsJson1_0 `POST /` through the same
//! router the emulator binary mounts. Every expectation below is therefore about
//! what the handler computes from persisted state.
//!
//! The rule under test: a preview answers the actions in the scope the rule type
//! and `organizationScope` select, keeping only those whose type is one of the
//! requested `recommendedActionTypes` and that satisfy every `criteria`
//! condition (conditions AND, the values inside one condition OR), in the order
//! the store holds them, cut into `maxResults`-sized pages.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EbsVolume, EbsVolumeConfiguration, EstimatedMonthlySavings, RecommendedAction, ResourceDetails,
    Tag,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, is_next_token,
};

/// The one account every emulated caller presents.
fn caller_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A second, member account id, for the organization-scope cases. Analysis
/// output about another account of the organization is seeded, never minted, so
/// a literal 12-digit id (the `AccountId` pattern) is what stands in for it.
const MEMBER_ACCOUNT: &str = "210987654321";

/// One row of the seeded inventory.
struct Seed<'a> {
    id: &'a str,
    account: String,
    region: &'a str,
    action_type: &'a str,
    /// The CURRENT EBS volume configuration, or `None` for an action that
    /// carries no `currentResourceDetails` — the absent-attribute case.
    volume: Option<(&'a str, i32)>,
    look_back_days: i32,
    /// `(beforeDiscountSavings, afterDiscountSavings, savingsEstimationMode)`, or
    /// `None` for an action with no estimate. The two figures DIFFER in every
    /// row, so a criterion that read the wrong one would select differently.
    savings: Option<(f64, f64, &'a str)>,
    restart_needed: Option<bool>,
    tags: &'a [(&'a str, &'a str)],
}

impl Seed<'_> {
    fn stored(&self) -> RecommendedAction {
        let resource_id = format!("vol-{}", self.id.to_ascii_lowercase());
        RecommendedAction {
            recommended_action_id: Some(self.id.to_string()),
            resource_arn: Some(format!(
                "arn:aws:ec2:{}:{}:volume/{resource_id}",
                self.region, self.account
            )),
            resource_id: Some(resource_id),
            account_id: Some(self.account.clone()),
            region: Some(self.region.to_string()),
            resource_type: Some("EbsVolume".to_string()),
            look_back_period_in_days: Some(self.look_back_days),
            recommended_action_type: Some(self.action_type.to_string()),
            current_resource_summary: self
                .volume
                .map(|(volume_type, size)| format!("{volume_type} | {size} GiB")),
            current_resource_details: self.volume.map(|(volume_type, size)| {
                ResourceDetails::ebsVolume(EbsVolume {
                    configuration: Some(EbsVolumeConfiguration {
                        r#type: Some(volume_type.to_string()),
                        size_in_gib: Some(size),
                        iops: Some(3000),
                        throughput: Some(125),
                    }),
                })
            }),
            recommended_resource_summary: self.volume.map(|(_, size)| format!("gp3 | {size} GiB")),
            recommended_resource_details: self.volume.map(|(_, size)| {
                ResourceDetails::ebsVolume(EbsVolume {
                    configuration: Some(EbsVolumeConfiguration {
                        r#type: Some("gp3".to_string()),
                        size_in_gib: Some(size),
                        iops: Some(3000),
                        throughput: Some(125),
                    }),
                })
            }),
            restart_needed: self.restart_needed,
            estimated_monthly_savings: self.savings.map(|(before, after, mode)| {
                EstimatedMonthlySavings {
                    currency: "USD".to_string(),
                    before_discount_savings: before,
                    after_discount_savings: after,
                    savings_estimation_mode: mode.to_string(),
                }
            }),
            resource_tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: (*key).to_string(),
                        value: (*value).to_string(),
                    })
                    .collect(),
            ),
        }
    }
}

const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// The inventory every case below reports over: five actions in the calling
/// account spanning both action types, four regions, three volume types, an
/// action with no current configuration, estimates in both savings modes and
/// three tag shapes — plus one action in a member account.
///
/// `recommendedActionId` is the model's `^[0-9A-Za-z]{16}$`.
fn inventory() -> Vec<RecommendedAction> {
    [
        Seed {
            id: "RA00000000000001",
            account: caller_account(),
            region: "us-east-1",
            action_type: UPGRADE,
            volume: Some(("gp2", 500)),
            look_back_days: 14,
            savings: Some((12.5, 0.1, "BeforeDiscount")),
            restart_needed: Some(false),
            tags: &[("Environment", "Production"), ("Owner", "Platform")],
        },
        Seed {
            id: "RA00000000000002",
            account: caller_account(),
            region: "us-west-2",
            action_type: UPGRADE,
            volume: Some(("gp2", 80)),
            look_back_days: 32,
            savings: Some((50.0, 3.0, "AfterDiscount")),
            restart_needed: Some(true),
            tags: &[("Environment", "Staging")],
        },
        Seed {
            id: "RA00000000000003",
            account: caller_account(),
            region: "eu-west-1",
            action_type: SNAPSHOT_AND_DELETE,
            volume: Some(("gp3", 1000)),
            look_back_days: 93,
            savings: Some((100.0, 90.0, "BeforeDiscount")),
            restart_needed: Some(false),
            tags: &[],
        },
        Seed {
            id: "RA00000000000004",
            account: caller_account(),
            region: "us-east-1",
            action_type: SNAPSHOT_AND_DELETE,
            volume: Some(("io1", 16)),
            look_back_days: 14,
            savings: Some((9.0, 0.5, "AfterDiscount")),
            restart_needed: Some(true),
            tags: &[("Environment", "Production")],
        },
        Seed {
            id: "RA00000000000005",
            account: caller_account(),
            region: "ap-southeast-2",
            action_type: UPGRADE,
            volume: None,
            look_back_days: 14,
            savings: None,
            restart_needed: None,
            tags: &[],
        },
        Seed {
            id: "RA00000000000006",
            account: MEMBER_ACCOUNT.to_string(),
            region: "us-east-1",
            action_type: UPGRADE,
            volume: Some(("gp2", 250)),
            look_back_days: 14,
            savings: Some((20.0, 18.0, "BeforeDiscount")),
            restart_needed: Some(false),
            tags: &[("Environment", "Production")],
        },
    ]
    .iter()
    .map(Seed::stored)
    .collect()
}

/// The crate's awsJson1_0 harness, with the caller enrolled (every operation but
/// the two enrollment ones refuses an account that is not opted in) and the
/// analysis inventory holding `actions`. The engine comes back too, so a case can
/// check the store after a call.
fn emulator_with(
    actions: &[RecommendedAction],
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    {
        let state =
            service_state(&*engine).expect("the in-memory engine carries this service's state");
        let mut guard = state.lock();
        for action in actions {
            let id = action
                .recommended_action_id
                .clone()
                .expect("a seeded action carries its id");
            guard.recommended_actions.insert(id, action.clone());
        }
    }
    (router, engine)
}

/// Every stored recommended action, rendered: a read that changed any field of
/// any action in place would change this string.
fn inventory_snapshot(engine: &engine::InMemoryEngine) -> String {
    let state = service_state(engine).expect("the engine carries this service's state");
    let snapshot = format!(
        "{:?}",
        state
            .lock()
            .recommended_actions
            .iter_in_order()
            .collect::<Vec<_>>()
    );
    snapshot
}

/// How many automation rules the store holds.
fn rule_count(engine: &engine::InMemoryEngine) -> usize {
    let state = service_state(engine).expect("the engine carries this service's state");
    let count = state.lock().rules.iter_in_order().count();
    count
}

/// The `recommendedActionId`s a preview response lists, in response order.
fn previewed_ids(body: &Value) -> Vec<String> {
    body.get("previewResults")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|result| {
            result
                .get("recommendedActionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A request for every action type, with `criteria` spliced in.
fn preview_request(criteria: Value) -> Value {
    json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        "criteria": criteria,
    })
}

// ---------------------------------------------------------------------------
// Selection: scope, action types and criteria
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preview_selects_the_actions_the_request_describes() {
    let (router, _engine) = emulator_with(&inventory());

    // (case, request, the ids the rule says come back)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "both action types, no criteria: every action of the calling account",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000003",
                "RA00000000000004",
                "RA00000000000005",
            ],
        ),
        (
            "one action type",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [UPGRADE] }),
            vec!["RA00000000000001", "RA00000000000002", "RA00000000000005"],
        ),
        (
            "the other action type",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [SNAPSHOT_AND_DELETE] }),
            vec!["RA00000000000003", "RA00000000000004"],
        ),
        (
            "region StringEquals",
            preview_request(
                json!({ "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }] }),
            ),
            vec!["RA00000000000001", "RA00000000000004"],
        ),
        (
            "region StringEquals with two values: the values of one condition are alternatives",
            preview_request(json!({
                "region": [{ "comparison": "StringEquals", "values": ["us-east-1", "eu-west-1"] }]
            })),
            vec![
                "RA00000000000001",
                "RA00000000000003",
                "RA00000000000004",
            ],
        ),
        (
            "region StringNotEquals",
            preview_request(
                json!({ "region": [{ "comparison": "StringNotEquals", "values": ["us-east-1"] }] }),
            ),
            vec![
                "RA00000000000002",
                "RA00000000000003",
                "RA00000000000005",
            ],
        ),
        (
            "region StringEqualsIgnoreCase",
            preview_request(json!({
                "region": [{ "comparison": "StringEqualsIgnoreCase", "values": ["US-EAST-1"] }]
            })),
            vec!["RA00000000000001", "RA00000000000004"],
        ),
        (
            "region StringLike with a wildcard",
            preview_request(
                json!({ "region": [{ "comparison": "StringLike", "values": ["us-*"] }] }),
            ),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000004",
            ],
        ),
        (
            "region StringNotLike with a wildcard",
            preview_request(
                json!({ "region": [{ "comparison": "StringNotLike", "values": ["us-*"] }] }),
            ),
            vec!["RA00000000000003", "RA00000000000005"],
        ),
        (
            "region StringLike with a single-character wildcard",
            preview_request(
                json!({ "region": [{ "comparison": "StringLike", "values": ["us-?ast-1"] }] }),
            ),
            vec!["RA00000000000001", "RA00000000000004"],
        ),
        (
            "region StringLike with a leading wildcard: the pattern is anchored at both ends, \
             so only a region that ENDS in the literal matches",
            preview_request(
                json!({ "region": [{ "comparison": "StringLike", "values": ["*-east-1"] }] }),
            ),
            vec!["RA00000000000001", "RA00000000000004"],
        ),
        (
            "region StringLike with a wildcard on both sides: the literal may sit anywhere",
            preview_request(
                json!({ "region": [{ "comparison": "StringLike", "values": ["*east*"] }] }),
            ),
            vec![
                "RA00000000000001",
                "RA00000000000004",
                "RA00000000000005",
            ],
        ),
        (
            "region StringLike with two adjacent wildcards: still every region",
            preview_request(
                json!({ "region": [{ "comparison": "StringLike", "values": ["**"] }] }),
            ),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000003",
                "RA00000000000004",
                "RA00000000000005",
            ],
        ),
        (
            "region StringLike where a single-character wildcard has nothing to consume: `?` \
             matches EXACTLY one character, never none",
            preview_request(
                json!({ "region": [{ "comparison": "StringLike", "values": ["us-east-1?"] }] }),
            ),
            vec![],
        ),
        (
            "region StringLike mixing both wildcards, which makes the walk backtrack: a \
             mismatch after `*` resumes one character later",
            preview_request(
                json!({ "region": [{ "comparison": "StringLike", "values": ["?s-*-?"] }] }),
            ),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000004",
            ],
        ),
        (
            "resourceArn StringLike over the account and region segments",
            preview_request(json!({
                "resourceArn": [{
                    "comparison": "StringLike",
                    "values": [format!("arn:aws:ec2:us-east-1:{}:volume/*", caller_account())],
                }]
            })),
            vec!["RA00000000000001", "RA00000000000004"],
        ),
        (
            "ebsVolumeType StringEquals: an action with no current configuration has no \
             attribute to compare",
            preview_request(
                json!({ "ebsVolumeType": [{ "comparison": "StringEquals", "values": ["gp2"] }] }),
            ),
            vec!["RA00000000000001", "RA00000000000002"],
        ),
        (
            "ebsVolumeType StringEqualsIfExists: the same, plus the action that has no \
             configuration",
            preview_request(json!({
                "ebsVolumeType": [{ "comparison": "StringEqualsIfExists", "values": ["gp2"] }]
            })),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000005",
            ],
        ),
        (
            "ebsVolumeSizeInGib NumericGreaterThan",
            preview_request(json!({
                "ebsVolumeSizeInGib": [{ "comparison": "NumericGreaterThan", "values": [100] }]
            })),
            vec!["RA00000000000001", "RA00000000000003"],
        ),
        (
            "ebsVolumeSizeInGib NumericLessThanEquals",
            preview_request(json!({
                "ebsVolumeSizeInGib": [{ "comparison": "NumericLessThanEquals", "values": [80] }]
            })),
            vec!["RA00000000000002", "RA00000000000004"],
        ),
        (
            "ebsVolumeSizeInGib NumericEquals",
            preview_request(json!({
                "ebsVolumeSizeInGib": [{ "comparison": "NumericEquals", "values": [500] }]
            })),
            vec!["RA00000000000001"],
        ),
        (
            "ebsVolumeSizeInGib NumericNotEquals",
            preview_request(json!({
                "ebsVolumeSizeInGib": [{ "comparison": "NumericNotEquals", "values": [500] }]
            })),
            vec![
                "RA00000000000002",
                "RA00000000000003",
                "RA00000000000004",
            ],
        ),
        (
            "two conditions on one criterion are ANDed",
            preview_request(json!({
                "ebsVolumeSizeInGib": [
                    { "comparison": "NumericGreaterThan", "values": [50] },
                    { "comparison": "NumericLessThanEquals", "values": [500] },
                ]
            })),
            vec!["RA00000000000001", "RA00000000000002"],
        ),
        (
            "lookBackPeriodInDays NumericEquals",
            preview_request(json!({
                "lookBackPeriodInDays": [{ "comparison": "NumericEquals", "values": [14] }]
            })),
            vec![
                "RA00000000000001",
                "RA00000000000004",
                "RA00000000000005",
            ],
        ),
        (
            "estimatedMonthlySavings NumericGreaterThanEquals includes the boundary",
            preview_request(json!({
                "estimatedMonthlySavings": [{
                    "comparison": "NumericGreaterThanEquals", "values": [12.5]
                }]
            })),
            vec!["RA00000000000001", "RA00000000000003"],
        ),
        (
            "estimatedMonthlySavings NumericGreaterThan excludes the boundary",
            preview_request(json!({
                "estimatedMonthlySavings": [{
                    "comparison": "NumericGreaterThan", "values": [12.5]
                }]
            })),
            vec!["RA00000000000003"],
        ),
        (
            "estimatedMonthlySavings compares the figure for the estimate's OWN \
             savingsEstimationMode",
            preview_request(json!({
                "estimatedMonthlySavings": [{ "comparison": "NumericLessThan", "values": [10.0] }]
            })),
            // The two AfterDiscount estimates (after 3.0 and 0.5); reading the
            // before-discount figure (50.0 and 9.0) would select differently.
            vec!["RA00000000000002", "RA00000000000004"],
        ),
        (
            "estimatedMonthlySavings NumericLessThanIfExists keeps the action with no estimate",
            preview_request(json!({
                "estimatedMonthlySavings": [{
                    "comparison": "NumericLessThanIfExists", "values": [1.0]
                }]
            })),
            vec!["RA00000000000004", "RA00000000000005"],
        ),
        (
            "restartNeeded StringEquals true",
            preview_request(
                json!({ "restartNeeded": [{ "comparison": "StringEquals", "values": ["true"] }] }),
            ),
            vec!["RA00000000000002", "RA00000000000004"],
        ),
        (
            "restartNeeded StringEquals false",
            preview_request(
                json!({ "restartNeeded": [{ "comparison": "StringEquals", "values": ["false"] }] }),
            ),
            vec!["RA00000000000001", "RA00000000000003"],
        ),
        (
            "resourceTag StringEquals on one tag key",
            preview_request(json!({
                "resourceTag": [{
                    "comparison": "StringEquals", "key": "Environment", "values": ["Production"]
                }]
            })),
            vec!["RA00000000000001", "RA00000000000004"],
        ),
        (
            "resourceTag StringLike on one tag key",
            preview_request(json!({
                "resourceTag": [{
                    "comparison": "StringLike", "key": "Environment", "values": ["Prod*"]
                }]
            })),
            vec!["RA00000000000001", "RA00000000000004"],
        ),
        (
            "resourceTag StringEqualsIfExists keeps every resource without that tag",
            preview_request(json!({
                "resourceTag": [{
                    "comparison": "StringEqualsIfExists", "key": "Owner", "values": ["Platform"]
                }]
            })),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000003",
                "RA00000000000004",
                "RA00000000000005",
            ],
        ),
        (
            "conditions on different criterions are ANDed",
            preview_request(json!({
                "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }],
                "restartNeeded": [{ "comparison": "StringEquals", "values": ["false"] }],
            })),
            vec!["RA00000000000001"],
        ),
        (
            "criteria that nothing satisfies",
            preview_request(
                json!({ "region": [{ "comparison": "StringEquals", "values": ["me-south-1"] }] }),
            ),
            vec![],
        ),
        (
            "an OrganizationRule preview with no scope spans every account",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000003",
                "RA00000000000004",
                "RA00000000000005",
                "RA00000000000006",
            ],
        ),
        (
            "an OrganizationRule preview whose scope names no accountIds spans every account, \
             like a scope that is absent",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "organizationScope": {},
            }),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000003",
                "RA00000000000004",
                "RA00000000000005",
                "RA00000000000006",
            ],
        ),
        (
            "an OrganizationRule preview scoped to one member account",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
            vec!["RA00000000000006"],
        ),
        (
            "an OrganizationRule preview scoped to two accounts, one action type",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE],
                "organizationScope": {
                    "accountIds": [caller_account(), MEMBER_ACCOUNT],
                },
            }),
            vec![
                "RA00000000000001",
                "RA00000000000002",
                "RA00000000000005",
                "RA00000000000006",
            ],
        ),
    ];

    for (case, request, expected) in cases {
        let body = call_ok(&router, "ListAutomationRulePreview", &request, case).await;
        let actual = previewed_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected previewed ids {expected:?}, got {actual:?} from {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: a single page that holds every match carries no nextToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The shape of a preview result
// ---------------------------------------------------------------------------

/// Every member of a `PreviewResult` comes off the stored action, so a preview
/// of one action reports exactly what the inventory holds for it.
#[tokio::test]
async fn a_preview_result_reports_the_stored_action() {
    let (router, _engine) = emulator_with(&inventory());
    let case = "one action selected by its resource id";
    let body = call_ok(
        &router,
        "ListAutomationRulePreview",
        &preview_request(json!({
            "ebsVolumeSizeInGib": [{ "comparison": "NumericEquals", "values": [500] }]
        })),
        case,
    )
    .await;

    let results = body
        .get("previewResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        results.len(),
        1,
        "case {case:?}: expected exactly one result, got {body}"
    );
    let result = &results[0];
    let expected = json!({
        "recommendedActionId": "RA00000000000001",
        "resourceArn": format!(
            "arn:aws:ec2:us-east-1:{}:volume/vol-ra00000000000001",
            caller_account()
        ),
        "resourceId": "vol-ra00000000000001",
        "accountId": caller_account(),
        "region": "us-east-1",
        "resourceType": "EbsVolume",
        "lookBackPeriodInDays": 14,
        "recommendedActionType": UPGRADE,
        "currentResourceSummary": "gp2 | 500 GiB",
        "currentResourceDetails": {
            "ebsVolume": {
                "configuration": {
                    "type": "gp2", "sizeInGib": 500, "iops": 3000, "throughput": 125
                }
            }
        },
        "recommendedResourceSummary": "gp3 | 500 GiB",
        "recommendedResourceDetails": {
            "ebsVolume": {
                "configuration": {
                    "type": "gp3", "sizeInGib": 500, "iops": 3000, "throughput": 125
                }
            }
        },
        "restartNeeded": false,
        "estimatedMonthlySavings": {
            "currency": "USD",
            "beforeDiscountSavings": 12.5,
            "afterDiscountSavings": 0.1,
            "savingsEstimationMode": "BeforeDiscount"
        },
        "resourceTags": [
            { "key": "Environment", "value": "Production" },
            { "key": "Owner", "value": "Platform" }
        ]
    });
    assert_eq!(
        result, &expected,
        "case {case:?}: expected the stored action, got {result}"
    );
}

/// A service that has observed no resources has no recommended actions, so the
/// preview is an empty list.
#[tokio::test]
async fn an_empty_inventory_previews_nothing() {
    let (router, _engine) = emulator_with(&[]);
    let case = "no analysis output yet";
    let body = call_ok(
        &router,
        "ListAutomationRulePreview",
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        }),
        case,
    )
    .await;
    assert_eq!(
        body,
        json!({ "previewResults": [] }),
        "case {case:?}: expected an empty preview, got {body}"
    );
}

/// The preview is a REPORT: the call leaves the inventory it reads exactly as it
/// was and creates nothing.
#[tokio::test]
async fn previewing_writes_nothing() {
    let (router, engine) = emulator_with(&inventory());
    let case = "a paged preview over a seeded inventory";
    let before = inventory_snapshot(&engine);
    let _ = call_ok(
        &router,
        "ListAutomationRulePreview",
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": [UPGRADE],
            "maxResults": 1,
        }),
        case,
    )
    .await;
    let after = inventory_snapshot(&engine);
    assert_eq!(
        after, before,
        "case {case:?}: a preview must not change any stored action"
    );
    let rules_after = rule_count(&engine);
    assert_eq!(
        rules_after, 0,
        "case {case:?}: a preview must not create an automation rule, found {rules_after}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Walking with any page size visits every match exactly once, in the same
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_walks_the_whole_selection_once_for_every_page_size() {
    let (router, _engine) = emulator_with(&inventory());
    let request = json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    });
    let all: Vec<String> = previewed_ids(
        &call_ok(
            &router,
            "ListAutomationRulePreview",
            &request,
            "one unpaged page",
        )
        .await,
    );
    assert_eq!(all.len(), 6, "the inventory holds six actions, got {all:?}");

    for page_size in [1_u64, 2, 5, 6, 7, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = request.clone();
            body["maxResults"] = json!(page_size);
            if let Some(token) = next.as_deref() {
                body["nextToken"] = json!(token);
            }
            let response = call_ok(&router, "ListAutomationRulePreview", &body, &case).await;
            let page = previewed_ids(&response);
            assert!(
                page.len() as u64 <= page_size,
                "case {case:?}: a page holds at most maxResults results, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: the walk must terminate, got {walked:?}"
            );
            match response.get("nextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        is_next_token(token),
                        "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
                    );
                    next = Some(token.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: the walk must visit every match once, in order"
        );
        let expected_pages = (all.len() as u64).div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s), walked {pages}"
        );
    }
}

/// A token is bound to the selection it was taken with: resuming with a
/// different one would answer a page of another result set, so it is refused.
#[tokio::test]
async fn a_token_resumes_only_the_selection_it_was_taken_with() {
    let (router, _engine) = emulator_with(&inventory());
    let first = call_ok(
        &router,
        "ListAutomationRulePreview",
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            "maxResults": 1,
        }),
        "first page",
    )
    .await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a five-result selection paged at one result has a next page")
        .to_string();

    let cases: Vec<(&str, Value)> = vec![
        (
            "a token resumed with different action types",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "nextToken": token.clone(),
            }),
        ),
        (
            "a token resumed with different criteria",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }]
                },
                "nextToken": token.clone(),
            }),
        ),
        (
            "a token resumed with a different rule type",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": token.clone(),
            }),
        ),
        (
            "a token this service never issued",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": "notAtokenThisServiceIssued",
            }),
        ),
    ];
    for (case, request) in cases {
        assert_refused(
            call(&router, "ListAutomationRulePreview", &request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    // The same token with the same selection still resumes.
    let resumed = call_ok(
        &router,
        "ListAutomationRulePreview",
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            "maxResults": 1,
            "nextToken": token,
        }),
        "the token resumed with its own selection",
    )
    .await;
    assert_eq!(
        previewed_ids(&resumed),
        vec!["RA00000000000002".to_string()],
        "the token must resume exactly where the first page stopped, got {resumed}"
    );
}

// ---------------------------------------------------------------------------
// Requests the model admits but the service cannot evaluate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_the_service_cannot_evaluate_is_refused() {
    let (router, _engine) = emulator_with(&inventory());

    let cases: Vec<(&str, Value)> = vec![
        (
            "no action type to include",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [] }),
        ),
        (
            "an organization scope on an account-scoped preview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
        ),
        (
            "a numeric comparison on a string criterion",
            preview_request(json!({
                "region": [{ "comparison": "NumericGreaterThan", "values": ["us-east-1"] }]
            })),
        ),
        (
            "a string comparison on an integer criterion",
            preview_request(json!({
                "ebsVolumeSizeInGib": [{ "comparison": "StringEquals", "values": [100] }]
            })),
        ),
        (
            "a string comparison on a double criterion",
            preview_request(json!({
                "estimatedMonthlySavings": [{ "comparison": "StringLike", "values": [1.0] }]
            })),
        ),
        (
            "a condition with no comparison operator",
            preview_request(json!({ "region": [{ "values": ["us-east-1"] }] })),
        ),
        (
            "a condition with no values",
            preview_request(json!({ "region": [{ "comparison": "StringEquals", "values": [] }] })),
        ),
        (
            "a numeric condition with no values",
            preview_request(json!({
                "lookBackPeriodInDays": [{ "comparison": "NumericEquals", "values": [] }]
            })),
        ),
        (
            "a resourceTag condition that names no tag key",
            preview_request(json!({
                "resourceTag": [{ "comparison": "StringEquals", "values": ["Production"] }]
            })),
        ),
    ];
    for (case, request) in cases {
        assert_refused(
            call(&router, "ListAutomationRulePreview", &request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }
}

/// `maxResults` is modelled `range(min: 1, max: 100)`, so a value outside those
/// published bounds is refused at the protocol boundary, while each bound itself
/// is accepted.
#[tokio::test]
async fn max_results_honours_its_modelled_bounds() {
    let (router, _engine) = emulator_with(&inventory());
    for max_results in [1, 100] {
        let case = format!("maxResults={max_results} is a published bound");
        let body = call_ok(
            &router,
            "ListAutomationRulePreview",
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "maxResults": max_results,
            }),
            &case,
        )
        .await;
        let ids = previewed_ids(&body);
        assert!(
            !ids.is_empty() && ids.len() <= max_results as usize,
            "case {case:?}: expected between one and {max_results} results, got {ids:?}"
        );
    }
    for max_results in [0, 101] {
        let case = format!("maxResults={max_results} is outside the modelled range");
        let (status, body) = call(
            &router,
            "ListAutomationRulePreview",
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "maxResults": max_results,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused with HTTP 400, got {status} and \
             {body}"
        );
    }
}
