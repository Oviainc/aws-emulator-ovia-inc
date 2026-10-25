// SPDX-License-Identifier: CC0-1.0

//! The RECOMMENDED-ACTION INVENTORY of an enrolled account, end to end.
//!
//! The rule under test: an account this service holds as enrolled has the
//! recommended actions the analysis of its resources produces, so a client that
//! has done nothing but opt in can list them, summarise them, preview a rule
//! against them, start an automation for one and read that automation back —
//! without anything having handed the service an action.
//!
//! Every call below goes over the wire: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts, from the UNENROLLED
//! harness, so nothing in this suite reaches into the store. A response built
//! from anything but persisted state fails here.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    RECOMMENDED_ACTION_FILTER_NAME_VALUES, RECOMMENDED_ACTION_TYPE_VALUES,
    SAVINGS_ESTIMATION_MODE_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    caller_account_id, recommended_action_inventory, RESOURCE_TYPE_EBS_VOLUME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    call, call_ok, member, string_member, unenrolled_emulator_with_engine,
};

/// A member account id to associate, in the model's `AccountId` form
/// (`^[0-9]{12}$`). Associating it is what makes this service hold it as
/// enrolled, so its own inventory is analysis output about ANOTHER account.
const MEMBER_ACCOUNT: &str = "444455556666";

/// How many recommended actions the analysis reports for one enrolled account.
fn inventory_size() -> usize {
    recommended_action_inventory(&caller_account_id()).len()
}

/// A router with this service mounted, with the calling account opted in the way
/// a client opts in: `UpdateEnrollmentConfiguration` over the wire, read back
/// through `GetEnrollmentConfiguration`.
async fn opted_in_emulator(case: &str) -> MultiServiceRouter {
    let router = unenrolled_emulator_with_engine().0;
    let updated = call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        case,
    )
    .await;
    assert_eq!(
        string_member(&updated, "status"),
        "Active",
        "case {case:?}: the opt-in must report the status it stored, got {updated}"
    );
    let read_back = call_ok(&router, "GetEnrollmentConfiguration", &json!({}), case).await;
    assert_eq!(
        string_member(&read_back, "status"),
        "Active",
        "case {case:?}: the account must read back as enrolled, got {read_back}"
    );
    router
}

/// Every recommended action `ListRecommendedActions` reports for `request`,
/// walked page by page so the whole set is seen however it is paged.
async fn listed_actions(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &token {
            page_request["nextToken"] = json!(token);
        }
        let page = call_ok(router, "ListRecommendedActions", &page_request, case).await;
        walked.extend(
            member(&page, "recommendedActions")
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: recommendedActions is a list, got {page}"))
                .clone(),
        );
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return walked;
        }
        assert!(
            walked.len() <= 1_000,
            "case {case:?}: the walk must terminate, walked {} actions",
            walked.len()
        );
    }
}

/// The `recommendedActionId`s of a list of reported actions, in report order.
fn ids(actions: &[Value]) -> Vec<String> {
    actions
        .iter()
        .map(|action| string_member(action, "recommendedActionId"))
        .collect()
}

/// The current `EbsVolumeConfiguration` an action reports, as a JSON object.
fn current_configuration(action: &Value) -> &Value {
    action
        .pointer("/currentResourceDetails/ebsVolume/configuration")
        .unwrap_or(&Value::Null)
}

/// `RecommendedActionId` and `EventId` are both modelled `^[0-9A-Za-z]{16}$`.
fn is_sixteen_alphanumerics(value: &str) -> bool {
    value.len() == 16 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

// ---------------------------------------------------------------------------
// The inventory an enrolled account has
// ---------------------------------------------------------------------------

/// Opting in is enough: the account then has recommended actions, every one of
/// them a well-formed analysis result whose members agree with each other, and
/// the set spans the dimensions this service's filters and criteria select on.
#[tokio::test]
async fn an_account_that_opts_in_can_list_the_actions_analysis_produced() {
    let router = opted_in_emulator("opt-in").await;

    let reported = listed_actions(&router, &json!({}), "inventory").await;
    assert_eq!(
        reported.len(),
        inventory_size(),
        "case \"inventory\": an enrolled account must have its analysed actions listed, got {:?}",
        ids(&reported)
    );

    let action_types: BTreeSet<String> = RECOMMENDED_ACTION_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let modes: BTreeSet<String> = SAVINGS_ESTIMATION_MODE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_types: BTreeSet<String> = BTreeSet::new();
    let mut seen_regions: BTreeSet<String> = BTreeSet::new();
    let mut seen_volume_types: BTreeSet<String> = BTreeSet::new();
    let mut seen_look_backs: BTreeSet<i64> = BTreeSet::new();
    let mut seen_tag_keys: BTreeSet<String> = BTreeSet::new();

    for action in &reported {
        let case = string_member(action, "recommendedActionId");
        assert!(
            is_sixteen_alphanumerics(&case),
            "case {case:?}: recommendedActionId must match ^[0-9A-Za-z]{{16}}$, got {action}"
        );
        assert!(
            seen_ids.insert(case.clone()),
            "case {case:?}: no two analysed actions may share an id, got {action}"
        );

        let account_id = string_member(action, "accountId");
        assert_eq!(
            account_id,
            caller_account_id(),
            "case {case:?}: an AccountRule caller's inventory is about its own account, got \
             {action}"
        );
        assert_eq!(
            string_member(action, "resourceType"),
            RESOURCE_TYPE_EBS_VOLUME,
            "case {case:?}: the only modelled ResourceType is the one reported, got {action}"
        );

        let action_type = string_member(action, "recommendedActionType");
        assert!(
            action_types.contains(&action_type),
            "case {case:?}: expected a modelled RecommendedActionType of {action_types:?}, got \
             {action}"
        );

        // The resource the action is about: its ARN carries the action's own
        // Region, account and resource id, so the three agree with each other.
        let region = string_member(action, "region");
        let resource_id = string_member(action, "resourceId");
        assert!(
            !region.is_empty() && !resource_id.is_empty(),
            "case {case:?}: an analysed action names the resource it is about, got {action}"
        );
        assert!(
            resource_id.len() <= 100
                && resource_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'),
            "case {case:?}: resourceId must match ^[a-zA-Z0-9_.-]+$ within 100 characters, got \
             {resource_id:?}"
        );
        assert_eq!(
            string_member(action, "resourceArn"),
            format!("arn:aws:ec2:{region}:{account_id}:volume/{resource_id}"),
            "case {case:?}: the ARN must name the same Region, account and resource, got {action}"
        );

        let look_back = member(action, "lookBackPeriodInDays")
            .as_i64()
            .unwrap_or_default();
        assert!(
            look_back > 0,
            "case {case:?}: a recommendation is drawn from some history, got {action}"
        );

        // The current configuration, and the one-line summary of it: the
        // summary describes the very configuration the action reports.
        let configuration = current_configuration(action);
        let volume_type = string_member(configuration, "type");
        let size_in_gib = member(configuration, "sizeInGib").as_i64().unwrap_or_default();
        assert!(
            !volume_type.is_empty() && size_in_gib > 0,
            "case {case:?}: the analysed volume carries its type and size, got {action}"
        );
        assert!(
            member(configuration, "iops").as_i64().unwrap_or_default() > 0,
            "case {case:?}: a provisioned volume carries its IOPS, got {action}"
        );
        assert_eq!(
            string_member(action, "currentResourceSummary"),
            format!("{volume_type} {size_in_gib} GiB"),
            "case {case:?}: the summary must describe the reported configuration, got {action}"
        );

        // What the action would leave behind: an upgrade leaves a volume of a
        // DIFFERENT type and the same size; a snapshot-and-delete leaves no
        // volume, so it describes no recommended resource.
        let recommended = action
            .pointer("/recommendedResourceDetails/ebsVolume/configuration")
            .unwrap_or(&Value::Null);
        if action_type == "UpgradeEbsVolumeType" {
            let recommended_type = string_member(recommended, "type");
            assert!(
                !recommended_type.is_empty() && recommended_type != volume_type,
                "case {case:?}: an upgrade must recommend another volume type, got {action}"
            );
            assert_eq!(
                member(recommended, "sizeInGib").as_i64().unwrap_or_default(),
                size_in_gib,
                "case {case:?}: a type change keeps the provisioned size, got {action}"
            );
            assert_eq!(
                string_member(action, "recommendedResourceSummary"),
                format!("{recommended_type} {size_in_gib} GiB"),
                "case {case:?}: the recommended summary must describe the recommended \
                 configuration, got {action}"
            );
        } else {
            assert!(
                recommended.is_null()
                    && member(action, "recommendedResourceSummary").is_null(),
                "case {case:?}: an action that deletes the volume leaves no recommended \
                 resource, got {action}"
            );
        }

        assert!(
            member(action, "restartNeeded").is_boolean(),
            "case {case:?}: restartNeeded must be answered either way, got {action}"
        );

        // The estimate: a figure computed for this volume, in one currency, with
        // the after-discount saving below the on-demand one.
        let savings = member(action, "estimatedMonthlySavings");
        let before = member(savings, "beforeDiscountSavings")
            .as_f64()
            .unwrap_or_default();
        let after = member(savings, "afterDiscountSavings")
            .as_f64()
            .unwrap_or_default();
        assert!(
            before > 0.0 && after > 0.0 && after < before,
            "case {case:?}: expected 0 < afterDiscountSavings < beforeDiscountSavings, got \
             {savings}"
        );
        assert!(
            !string_member(savings, "currency").is_empty(),
            "case {case:?}: an amount names its currency, got {savings}"
        );
        assert!(
            modes.contains(&string_member(savings, "savingsEstimationMode")),
            "case {case:?}: expected a modelled SavingsEstimationMode of {modes:?}, got {savings}"
        );

        seen_types.insert(action_type);
        seen_regions.insert(region);
        seen_volume_types.insert(volume_type);
        seen_look_backs.insert(look_back);
        for tag in member(action, "resourceTags").as_array().unwrap_or(&Vec::new()) {
            seen_tag_keys.insert(string_member(tag, "key"));
        }
    }

    // The inventory spans what this service selects on, so every filter and
    // criteria condition has something to tell apart.
    assert_eq!(
        seen_types, action_types,
        "the inventory must hold an action of every modelled RecommendedActionType, got \
         {seen_types:?}"
    );
    for (dimension, distinct) in [
        ("region", seen_regions.len()),
        ("current volume type", seen_volume_types.len()),
        ("lookBackPeriodInDays", seen_look_backs.len()),
    ] {
        assert!(
            distinct > 1,
            "the inventory must vary along {dimension}, got {distinct} distinct value(s)"
        );
    }
    assert!(
        !seen_tag_keys.is_empty(),
        "the inventory must carry resource tags for the tag filters to select on"
    );
}

/// The inventory is a property of the enrolled account, not of the call that
/// read it: listing it twice reports exactly the same actions, two separately
/// built emulators report the same ones, and opting out and back in neither
/// duplicates nor rewrites an action.
#[tokio::test]
async fn the_inventory_is_deterministic_and_unchanged_by_reading_it() {
    let router = opted_in_emulator("first").await;
    let first = listed_actions(&router, &json!({}), "first").await;
    let again = listed_actions(&router, &json!({}), "again").await;
    assert_eq!(
        first, again,
        "case \"again\": listing is a read, so it must report the same actions"
    );

    // A second, independently built emulator: same account, same analysis.
    let other = opted_in_emulator("other-emulator").await;
    assert_eq!(
        listed_actions(&other, &json!({}), "other-emulator").await,
        first,
        "case \"other-emulator\": one account's analysis output must not depend on the process"
    );

    // Opting out and back in: the account's inventory is the same set, in the
    // same order, with no second copy of anything.
    for (case, status) in [("opt-out", "Inactive"), ("opt-in-again", "Active")] {
        call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": status}),
            case,
        )
        .await;
    }
    assert_eq!(
        listed_actions(&router, &json!({}), "opt-in-again").await,
        first,
        "case \"opt-in-again\": re-enrolling must not duplicate or rewrite an action"
    );
}

/// Every `RecommendedActionFilterName` selects over the inventory the account
/// got by enrolling: for each distinct value the inventory holds for that field,
/// the filter reports exactly the actions carrying it.
#[tokio::test]
async fn every_filter_selects_over_the_analysed_inventory() {
    let router = opted_in_emulator("filters").await;
    let all = listed_actions(&router, &json!({}), "filters").await;
    assert!(
        !all.is_empty(),
        "the filter table needs an inventory to select over"
    );

    // (filter name, the value of that field for one action) — every row is
    // derived from what the service itself reported, so the expectation is the
    // selection, never a transcribed constant.
    type Selector = fn(&Value) -> Vec<String>;
    let selectors: Vec<(&str, Selector)> = vec![
        ("ResourceType", |action| {
            vec![string_member(action, "resourceType")]
        }),
        ("RecommendedActionType", |action| {
            vec![string_member(action, "recommendedActionType")]
        }),
        ("ResourceId", |action| {
            vec![string_member(action, "resourceId")]
        }),
        ("AccountId", |action| {
            vec![string_member(action, "accountId")]
        }),
        ("LookBackPeriodInDays", |action| {
            vec![member(action, "lookBackPeriodInDays")
                .as_i64()
                .unwrap_or_default()
                .to_string()]
        }),
        ("CurrentResourceDetailsEbsVolumeType", |action| {
            vec![string_member(current_configuration(action), "type")]
        }),
        ("RestartNeeded", |action| {
            vec![member(action, "restartNeeded")
                .as_bool()
                .unwrap_or_default()
                .to_string()]
        }),
        ("ResourceTagsKey", |action| {
            member(action, "resourceTags")
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|tag| string_member(tag, "key"))
                .collect()
        }),
        ("ResourceTagsValue", |action| {
            member(action, "resourceTags")
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|tag| string_member(tag, "value"))
                .collect()
        }),
    ];

    // The table spans the whole modelled value set, so a name added to the
    // model cannot be left untested.
    let covered: BTreeSet<String> = selectors
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    let modelled: BTreeSet<String> = RECOMMENDED_ACTION_FILTER_NAME_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        covered, modelled,
        "every modelled RecommendedActionFilterName must appear in the table"
    );

    for (name, selector) in &selectors {
        // Every value the inventory holds for this field, plus one it holds for
        // nothing, so a filter that selects everything would be caught.
        let mut values: BTreeSet<String> = all.iter().flat_map(selector).collect();
        values.insert("no-action-carries-this".to_string());
        for value in &values {
            let case = format!("{name}={value}");
            let expected: Vec<String> = all
                .iter()
                .filter(|action| selector(action).contains(value))
                .map(|action| string_member(action, "recommendedActionId"))
                .collect();
            let selected = listed_actions(
                &router,
                &json!({"filters": [{"name": name, "values": [value]}]}),
                &case,
            )
            .await;
            assert_eq!(
                ids(&selected),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                ids(&selected)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The workflow the inventory makes reachable
// ---------------------------------------------------------------------------

/// With nothing but an opt-in, a client can summarise the inventory, preview a
/// rule against it, start an automation for each action it holds and read every
/// one of those automations back.
#[tokio::test]
async fn the_whole_automation_workflow_runs_from_an_opt_in_alone() {
    let router = opted_in_emulator("workflow").await;
    let actions = listed_actions(&router, &json!({}), "workflow").await;
    assert!(
        !actions.is_empty(),
        "case \"workflow\": the workflow starts from the account's analysed actions"
    );

    // What the listed actions say the summaries must report: one group per
    // action type, in the order the inventory first holds that type, each with
    // its count and the sums of its estimates.
    let mut group_order: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut before_totals: BTreeMap<String, f64> = BTreeMap::new();
    let mut after_totals: BTreeMap<String, f64> = BTreeMap::new();
    for action in &actions {
        let key = string_member(action, "recommendedActionType");
        if !group_order.contains(&key) {
            group_order.push(key.clone());
        }
        *counts.entry(key.clone()).or_default() += 1;
        let savings = member(action, "estimatedMonthlySavings");
        *before_totals.entry(key.clone()).or_default() += member(savings, "beforeDiscountSavings")
            .as_f64()
            .unwrap_or_default();
        *after_totals.entry(key).or_default() += member(savings, "afterDiscountSavings")
            .as_f64()
            .unwrap_or_default();
    }

    let summarised = call_ok(
        &router,
        "ListRecommendedActionSummaries",
        &json!({}),
        "summaries",
    )
    .await;
    let summaries = member(&summarised, "recommendedActionSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("recommendedActionSummaries is a list, got {summarised}"))
        .clone();
    assert_eq!(
        summaries
            .iter()
            .map(|summary| string_member(summary, "key"))
            .collect::<Vec<String>>(),
        group_order,
        "case \"summaries\": one group per action type the inventory holds, got {summarised}"
    );
    for summary in &summaries {
        let key = string_member(summary, "key");
        let total = member(summary, "total");
        let case = format!("summary-{key}");
        assert_eq!(
            member(total, "recommendedActionCount").as_i64(),
            counts.get(&key).copied(),
            "case {case:?}: the group counts the listed actions of its type, got {summary}"
        );
        let savings = member(total, "estimatedMonthlySavings");
        for (field, expected) in [
            ("beforeDiscountSavings", before_totals.get(&key)),
            ("afterDiscountSavings", after_totals.get(&key)),
        ] {
            let expected = expected.copied().unwrap_or_default();
            let actual = member(savings, field).as_f64().unwrap_or_default();
            assert!(
                (actual - expected).abs() < 1e-9,
                "case {case:?}: expected {field} {expected}, got {actual} in {summary}"
            );
        }
    }

    // A rule previewed against the inventory selects the very actions the list
    // reports for the same action types.
    let previewed = call_ok(
        &router,
        "ListAutomationRulePreview",
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": RECOMMENDED_ACTION_TYPE_VALUES,
            "maxResults": 100,
        }),
        "preview",
    )
    .await;
    let preview_results = member(&previewed, "previewResults")
        .as_array()
        .unwrap_or_else(|| panic!("previewResults is a list, got {previewed}"))
        .clone();
    assert_eq!(
        ids(&preview_results),
        ids(&actions),
        "case \"preview\": a preview over every action type selects the whole inventory, got \
         {previewed}"
    );

    let preview_summaries = call_ok(
        &router,
        "ListAutomationRulePreviewSummaries",
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": RECOMMENDED_ACTION_TYPE_VALUES,
        }),
        "preview-summaries",
    )
    .await;
    let summarised_preview: Vec<(String, Option<i64>)> =
        member(&preview_summaries, "previewResultSummaries")
            .as_array()
            .unwrap_or_else(|| panic!("previewResultSummaries is a list, got {preview_summaries}"))
            .iter()
            .map(|summary| {
                (
                    string_member(summary, "key"),
                    member(member(summary, "total"), "recommendedActionCount").as_i64(),
                )
            })
            .collect();
    assert_eq!(
        summarised_preview,
        group_order
            .iter()
            .map(|key| (key.clone(), counts.get(key).copied()))
            .collect::<Vec<(String, Option<i64>)>>(),
        "case \"preview-summaries\": the preview summarises the same selection, got \
         {preview_summaries}"
    );

    // Each action can be automated, and each automation reads back.
    for action in &actions {
        let action_id = string_member(action, "recommendedActionId");
        let case = format!("start-{action_id}");
        let started = call_ok(
            &router,
            "StartAutomationEvent",
            &json!({"recommendedActionId": &action_id}),
            &case,
        )
        .await;
        let event_id = string_member(&started, "eventId");
        assert!(
            is_sixteen_alphanumerics(&event_id),
            "case {case:?}: expected an eventId matching ^[0-9A-Za-z]{{16}}$, got {started}"
        );
        assert_eq!(
            string_member(&started, "recommendedActionId"),
            action_id,
            "case {case:?}: the automation names the action it runs, got {started}"
        );

        let event = call_ok(
            &router,
            "GetAutomationEvent",
            &json!({"eventId": &event_id}),
            &case,
        )
        .await;
        for field in [
            "recommendedActionId",
            "resourceArn",
            "resourceId",
            "resourceType",
            "region",
            "accountId",
        ] {
            let expected = string_member(action, field);
            assert_eq!(
                string_member(&event, field),
                expected,
                "case {case:?}: the event must carry the action's {field} ({expected:?}), got \
                 {event}"
            );
        }
        assert_eq!(
            string_member(&event, "eventType"),
            string_member(action, "recommendedActionType"),
            "case {case:?}: the automation's type is the action's, got {event}"
        );
    }

    // And every automation that was started is listed.
    let listed_events = call_ok(&router, "ListAutomationEvents", &json!({}), "events").await;
    let event_action_ids: Vec<String> = member(&listed_events, "automationEvents")
        .as_array()
        .unwrap_or_else(|| panic!("automationEvents is a list, got {listed_events}"))
        .iter()
        .map(|event| string_member(event, "recommendedActionId"))
        .collect();
    assert_eq!(
        event_action_ids,
        ids(&actions),
        "case \"events\": one automation per started action, got {listed_events}"
    );
}

/// Associating a member account brings that account's own analysed inventory,
/// which the management account can list, tell apart by `accountId`, preview
/// under an organization scope and automate.
#[tokio::test]
async fn an_associated_member_account_brings_its_own_inventory() {
    let router = opted_in_emulator("association").await;
    let own = listed_actions(&router, &json!({}), "association").await;

    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [MEMBER_ACCOUNT]}),
        "associate",
    )
    .await;

    let after = listed_actions(&router, &json!({}), "associate").await;
    assert_eq!(
        after.len(),
        own.len() + inventory_size(),
        "case \"associate\": the member account's analysis output joins the listing, got {:?}",
        ids(&after)
    );
    assert_eq!(
        ids(&after)[..own.len()],
        ids(&own)[..],
        "case \"associate\": the actions already held keep their place in the listing"
    );

    // The member account's actions are about the member account, and no action
    // is shared between the two accounts.
    let member_actions = listed_actions(
        &router,
        &json!({"filters": [{"name": "AccountId", "values": [MEMBER_ACCOUNT]}]}),
        "member-only",
    )
    .await;
    assert_eq!(
        member_actions.len(),
        inventory_size(),
        "case \"member-only\": the member account has its own analysed actions, got {:?}",
        ids(&member_actions)
    );
    for action in &member_actions {
        let case = string_member(action, "recommendedActionId");
        assert_eq!(
            string_member(action, "accountId"),
            MEMBER_ACCOUNT,
            "case {case:?}: the filter selects on the account the action is about, got {action}"
        );
        assert!(
            string_member(action, "resourceArn").contains(MEMBER_ACCOUNT),
            "case {case:?}: a member account's resource is named in that account, got {action}"
        );
        assert!(
            !ids(&own).contains(&case),
            "case {case:?}: two accounts must not share a recommended action"
        );
    }

    // Re-associating the same account changes nothing.
    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [MEMBER_ACCOUNT, MEMBER_ACCOUNT]}),
        "re-associate",
    )
    .await;
    assert_eq!(
        ids(&listed_actions(&router, &json!({}), "re-associate").await),
        ids(&after),
        "case \"re-associate\": an association already recorded adds no second inventory"
    );

    // An organization-scoped preview reaches the member account's actions, and
    // one of them can be automated.
    let previewed = call_ok(
        &router,
        "ListAutomationRulePreview",
        &json!({
            "ruleType": "OrganizationRule",
            "organizationScope": {"accountIds": [MEMBER_ACCOUNT]},
            "recommendedActionTypes": RECOMMENDED_ACTION_TYPE_VALUES,
            "maxResults": 100,
        }),
        "org-preview",
    )
    .await;
    let previewed_ids = ids(member(&previewed, "previewResults")
        .as_array()
        .unwrap_or_else(|| panic!("previewResults is a list, got {previewed}")));
    assert_eq!(
        previewed_ids,
        ids(&member_actions),
        "case \"org-preview\": the scope selects exactly the member account's actions, got \
         {previewed}"
    );

    let member_action_id = previewed_ids
        .first()
        .cloned()
        .expect("the member account has at least one analysed action");
    let started = call_ok(
        &router,
        "StartAutomationEvent",
        &json!({"recommendedActionId": &member_action_id}),
        "member-start",
    )
    .await;
    let event = call_ok(
        &router,
        "GetAutomationEvent",
        &json!({"eventId": string_member(&started, "eventId")}),
        "member-start",
    )
    .await;
    assert_eq!(
        string_member(&event, "accountId"),
        MEMBER_ACCOUNT,
        "case \"member-start\": the automation runs in the account the action is about, got \
         {event}"
    );

    // An account that was never associated has no inventory, so an id of its
    // own is not found.
    let (status, body) = call(
        &router,
        "StartAutomationEvent",
        &json!({"recommendedActionId": recommended_action_inventory("999988887777")
            .first()
            .map(|(id, _)| id.clone())
            .expect("the analysis describes at least one volume")}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"unassociated-account\": an account this service does not hold has no analysed \
         actions, got {status} and {body}"
    );
}
