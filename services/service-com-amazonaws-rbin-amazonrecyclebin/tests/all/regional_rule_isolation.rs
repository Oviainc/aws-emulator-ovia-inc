// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Region a Recycle Bin retention rule exists in.
//!
//! The rule asserted here: a retention rule exists only in the Region its create was
//! signed for, so every operation sees the rules of the Region the REQUEST is signed
//! for and no other's — `ListRules` ("Lists the Recycle Bin retention rules in the
//! Region", smithy_query(query="doc", shape="ListRules")) reports only them, every
//! identifier- or ARN-addressed operation resolves only them and answers the modelled
//! 404 `ResourceNotFoundException` for any other Region's rule, and the per-Region
//! rule quota counts only them.
//!
//! The tables below span the Region the request is signed for (the dimension the rule
//! generalises over), every addressed operation of the service, and both per-Region
//! counts. Every row drives the service through the real router
//! (`MultiServiceRouter::dispatch`), exactly as a signed SDK request would, and states
//! its own case so a failure names the input.

use serde_json::{json, Value};

use crate::error_response_bodies::{error_code, message};
use crate::resource_tags::percent_encode;
use crate::retention_rule_lifecycle::{
    call_signed_for, lock_configuration, list_of, retention, router, supported_retention,
    RESOURCE_TYPES, UNLOCK_DELAY_MIN,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    MAX_RULES, MAX_RULES_PER_RESOURCE_TAG, RULE_STORE_LIMIT,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Regions the tables span: published AWS Region codes from four different
/// geographies, each one admitted by `RuleArn`'s Region segment
/// (`^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}$`, from
/// smithy_query(query="shape", shape="RuleArn")). The rule generalises over the Region
/// a request is signed for, so the rows are Regions.
const REGIONS: [&str; 4] = ["us-east-1", "eu-west-1", "ap-southeast-2", "sa-east-1"];

/// The index of the Region segment of an ARN: `arn:<partition>:<service>:<region>:…`.
const ARN_REGION_SEGMENT: usize = 3;

/// `POST /rules` signed for `region` — `CreateRule`.
async fn create_rule_in(
    router: &MultiServiceRouter,
    region: &str,
    body: Value,
) -> (u16, Value) {
    call_signed_for(router, region, "POST", "/rules", Some(body)).await
}

/// `GET /rules/{Identifier}` signed for `region` — `GetRule`.
async fn get_rule_in(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_signed_for(router, region, "GET", &format!("/rules/{identifier}"), None).await
}

/// `POST /list-rules` signed for `region` — `ListRules`.
async fn list_rules_in(router: &MultiServiceRouter, region: &str, body: Value) -> (u16, Value) {
    call_signed_for(router, region, "POST", "/list-rules", Some(body)).await
}

/// Create one Region-level rule in `region` and hand back `(identifier, rule_arn)`.
async fn created_rule_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: Value,
) -> (String, String) {
    let (status, created) = create_rule_in(router, region, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string();
    let rule_arn = created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no RuleArn: {created:?}"))
        .to_string();
    (identifier, rule_arn)
}

/// The Region segment of an ARN.
fn arn_region(rule_arn: &str) -> &str {
    rule_arn
        .split(':')
        .nth(ARN_REGION_SEGMENT)
        .unwrap_or_else(|| panic!("an rbin rule ARN carries a Region segment, got {rule_arn:?}"))
}

/// Every identifier `ListRules` reports for `resource_type` through `region`'s endpoint.
async fn listed_in(
    router: &MultiServiceRouter,
    region: &str,
    resource_type: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) =
        list_rules_in(router, region, json!({ "ResourceType": resource_type })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListRules, got {status} / {body:?}"
    );
    list_of(&body, "Rules")
        .iter()
        .map(|summary| {
            summary["Identifier"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every request a rule can be ADDRESSED by, as a request signed for one Region
/// against a rule of another: the operation's name, and the call built from the
/// identifier and the ARN the rule was minted with.
///
/// `Identifier` names the rule for five of them and `ResourceArn` for the three tag
/// operations, so each row also carries the rule IDENTIFIER the refusal must be worded
/// from — which for an ARN-addressed row is the identifier that ARN names.
async fn addressed_calls(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    rule_arn: &str,
) -> Vec<(&'static str, String, (u16, Value))> {
    let arn_path = format!("/tags/{}", percent_encode(rule_arn));
    let rule_path = format!("/rules/{identifier}");
    vec![
        (
            "GetRule",
            identifier.to_string(),
            call_signed_for(router, region, "GET", &rule_path, None).await,
        ),
        (
            "UpdateRule",
            identifier.to_string(),
            call_signed_for(
                router,
                region,
                "PATCH",
                &rule_path,
                Some(json!({ "Description": "reached from another Region" })),
            )
            .await,
        ),
        (
            "LockRule",
            identifier.to_string(),
            call_signed_for(
                router,
                region,
                "PATCH",
                &format!("/rules/{identifier}/lock"),
                Some(json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) })),
            )
            .await,
        ),
        (
            "UnlockRule",
            identifier.to_string(),
            call_signed_for(
                router,
                region,
                "PATCH",
                &format!("/rules/{identifier}/unlock"),
                Some(json!({})),
            )
            .await,
        ),
        (
            "ListTagsForResource",
            identifier.to_string(),
            call_signed_for(router, region, "GET", &arn_path, None).await,
        ),
        (
            "TagResource",
            identifier.to_string(),
            call_signed_for(
                router,
                region,
                "POST",
                &arn_path,
                Some(json!({ "Tags": [{ "Key": "reached", "Value": "elsewhere" }] })),
            )
            .await,
        ),
        (
            "UntagResource",
            identifier.to_string(),
            call_signed_for(
                router,
                region,
                "DELETE",
                &format!("{arn_path}?tagKeys={}", percent_encode("owner")),
                None,
            )
            .await,
        ),
        // DELETE last: every row above must find the rule still absent, and this one
        // must not remove it from the Region that does hold it.
        (
            "DeleteRule",
            identifier.to_string(),
            call_signed_for(router, region, "DELETE", &rule_path, None).await,
        ),
    ]
}

#[tokio::test]
async fn a_rule_is_reported_only_through_the_region_it_was_created_in() {
    let router = router();
    // One rule per Region, of a cycled resource type so a Region's listing has to skip
    // another Region's rule of the same type AND of a different one.
    let mut created: Vec<(&str, &str, String)> = Vec::new();
    for (row, region) in REGIONS.iter().enumerate() {
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let case = format!("a rule created in {region} retaining {resource_type}");
        let (identifier, rule_arn) = created_rule_in(
            &router,
            region,
            &case,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(supported_retention(resource_type, row)),
            }),
        )
        .await;
        assert_eq!(
            arn_region(&rule_arn),
            *region,
            "case {case:?}: the rule's ARN must name the Region it was created in, \
             got {rule_arn:?}"
        );
        created.push((region, resource_type, identifier));
    }

    for region in REGIONS {
        for resource_type in RESOURCE_TYPES {
            let case = format!("ListRules in {region} for {resource_type}");
            let expected: Vec<String> = created
                .iter()
                .filter(|(held_region, held_type, _)| {
                    *held_region == region && *held_type == resource_type
                })
                .map(|(_, _, identifier)| identifier.clone())
                .collect();
            let reported = listed_in(&router, region, resource_type, &case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: ListRules must report this Region's rules of that type \
                 alone, got {reported:?}"
            );
        }
        // And every summary this Region reports carries this Region in its ARN.
        for resource_type in RESOURCE_TYPES {
            let case = format!("the ARNs ListRules reports in {region} for {resource_type}");
            let (status, body) =
                list_rules_in(&router, region, json!({ "ResourceType": resource_type })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListRules, got {status} / {body:?}"
            );
            for summary in list_of(&body, "Rules") {
                let rule_arn = summary["RuleArn"].as_str().unwrap_or_default().to_string();
                assert_eq!(
                    arn_region(&rule_arn),
                    region,
                    "case {case:?}: a reported rule must belong to this Region, \
                     got {rule_arn:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_addressed_rule_another_region_holds_does_not_exist_here() {
    for (row, owner) in REGIONS.iter().enumerate() {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let setup = format!("a rule of {owner} retaining {resource_type}");
        let (identifier, rule_arn) = created_rule_in(
            &router,
            owner,
            &setup,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(7),
                "Tags": [{ "Key": "owner", "Value": owner }],
            }),
        )
        .await;

        for other in REGIONS.iter().filter(|region| *region != owner) {
            for (operation, addressed, (status, body)) in
                addressed_calls(&router, other, &identifier, &rule_arn).await
            {
                let case = format!("{operation} in {other} on a rule of {owner}");
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected HTTP 404, got {status} / {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected the modelled ResourceNotFoundException, \
                     got {body:?}"
                );
                let expected = format!("Recycle Bin Rule '{addressed}' does not exist.");
                assert_eq!(
                    message(&body),
                    expected,
                    "case {case:?}: expected Message {expected:?}, got {:?}",
                    message(&body)
                );
            }
            // Nothing the other Region's endpoint did reached the rule.
            let case = format!("the rule of {owner} after {other} addressed it");
            let (status, fetched) = get_rule_in(&router, owner, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the owning Region must still hold the rule, \
                 got {status} / {fetched:?}"
            );
            assert_eq!(
                fetched["Description"],
                Value::Null,
                "case {case:?}: a refused UpdateRule must leave the rule unchanged, got {:?}",
                fetched["Description"]
            );
            assert_eq!(
                fetched["LockState"],
                Value::Null,
                "case {case:?}: a refused LockRule must leave the rule unlocked, got {:?}",
                fetched["LockState"]
            );
            let (status, tags) = call_signed_for(
                &router,
                owner,
                "GET",
                &format!("/tags/{}", percent_encode(&rule_arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListTagsForResource, \
                 got {status} / {tags:?}"
            );
            assert_eq!(
                tags["Tags"],
                json!([{ "Key": "owner", "Value": owner }]),
                "case {case:?}: a refused TagResource/UntagResource must leave the rule's \
                 tags alone, got {:?}",
                tags["Tags"]
            );
        }

        // The owning Region serves what every other Region refused.
        let case = format!("DeleteRule in {owner} on its own rule");
        let (status, body) = call_signed_for(
            &router,
            owner,
            "DELETE",
            &format!("/rules/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected HTTP 204, got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn the_rules_per_region_quota_counts_only_the_signed_region() {
    let router = router();
    let saturated = REGIONS[0];
    let resource_type = RESOURCE_TYPES[0];
    // Fill one Region to its published quota of retention rules.
    for nth in 0..MAX_RULES {
        let (status, body) = create_rule_in(
            &router,
            saturated,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(supported_retention(resource_type, nth)),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {saturated:?}: expected HTTP 201 from CreateRule #{nth}, \
             got {status} / {body:?}"
        );
    }
    // One past the quota, in the Region that holds them, is refused.
    let (status, body) = create_rule_in(
        &router,
        saturated,
        json!({ "ResourceType": resource_type, "RetentionPeriod": retention(7) }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case {saturated:?}: a rule past the per-Region quota must be refused, \
         got {status} / {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("SERVICE_QUOTA_EXCEEDED"),
        "case {saturated:?}: expected Reason SERVICE_QUOTA_EXCEEDED, got {:?}",
        body["Reason"]
    );

    // Every OTHER Region still holds nothing, so its own quota is untouched.
    for region in REGIONS.iter().filter(|region| **region != saturated) {
        let case = format!("CreateRule in {region} while {saturated} is saturated");
        let (identifier, rule_arn) = created_rule_in(
            &router,
            region,
            &case,
            json!({ "ResourceType": resource_type, "RetentionPeriod": retention(7) }),
        )
        .await;
        assert_eq!(
            arn_region(&rule_arn),
            *region,
            "case {case:?}: the rule must belong to the Region that created it, \
             got {rule_arn:?}"
        );
        let reported = listed_in(&router, region, resource_type, &case).await;
        assert_eq!(
            reported,
            vec![identifier],
            "case {case:?}: the Region must report its one rule alone, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_resource_tag_pair_quota_counts_only_the_signed_region() {
    let router = router();
    let saturated = REGIONS[1];
    let resource_type = RESOURCE_TYPES[1];
    let pair = json!({ "ResourceTagKey": "retain", "ResourceTagValue": "yes" });
    // One resource tag key and value pair may select the quota of rules in a Region.
    for nth in 0..MAX_RULES_PER_RESOURCE_TAG {
        let case = format!("holder {nth} of the pair in {saturated}");
        created_rule_in(
            &router,
            saturated,
            &case,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(7),
                "ResourceTags": [pair],
            }),
        )
        .await;
    }
    let (status, body) = create_rule_in(
        &router,
        saturated,
        json!({
            "ResourceType": resource_type,
            "RetentionPeriod": retention(7),
            "ResourceTags": [pair],
        }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case {saturated:?}: a rule past the pair's quota must be refused, \
         got {status} / {body:?}"
    );

    // The same pair selects nothing in any other Region, so it admits the quota there
    // too.
    for region in REGIONS.iter().filter(|region| **region != saturated) {
        for nth in 0..MAX_RULES_PER_RESOURCE_TAG {
            let case = format!("holder {nth} of the same pair in {region}");
            created_rule_in(
                &router,
                region,
                &case,
                json!({
                    "ResourceType": resource_type,
                    "RetentionPeriod": retention(7),
                    "ResourceTags": [pair],
                }),
            )
            .await;
        }
        let case = format!("one past the pair's quota in {region}");
        let (status, body) = create_rule_in(
            &router,
            region,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(7),
                "ResourceTags": [pair],
            }),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: the pair's quota must bound this Region too, \
             got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn the_store_ceiling_bounds_the_whole_store_however_the_region_varies() {
    let router = router();
    let resource_type = RESOURCE_TYPES[2];
    // Fill the store to the ceiling this emulator keeps on the WHOLE store, a Region at
    // a time: each Region admits its own published quota, so the ceiling is reached
    // across Regions rather than within one.
    let mut held = 0usize;
    for region in REGIONS {
        let room = (RULE_STORE_LIMIT - held).min(MAX_RULES);
        for nth in 0..room {
            let (status, body) = create_rule_in(
                &router,
                region,
                json!({
                    "ResourceType": resource_type,
                    "RetentionPeriod": retention(supported_retention(resource_type, nth)),
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {region:?}: expected HTTP 201 from CreateRule #{nth}, \
                 got {status} / {body:?}"
            );
        }
        held += room;
        if held == RULE_STORE_LIMIT {
            break;
        }
    }
    assert_eq!(
        held, RULE_STORE_LIMIT,
        "setup: the Regions under test must be able to fill the store to its ceiling"
    );

    // Past the ceiling every Region is refused — including one holding nothing, which
    // is what makes this a bound on the store rather than on a Region.
    for region in REGIONS {
        let case = format!("CreateRule in {region} with the store at its ceiling");
        let (status, body) = create_rule_in(
            &router,
            region,
            json!({ "ResourceType": resource_type, "RetentionPeriod": retention(7) }),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: a rule past the store's ceiling must be refused, \
             got {status} / {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("SERVICE_QUOTA_EXCEEDED"),
            "case {case:?}: expected Reason SERVICE_QUOTA_EXCEEDED, got {:?}",
            body["Reason"]
        );
        assert!(
            body["Message"].as_str().is_some_and(|stated| !stated.is_empty()),
            "case {case:?}: expected a non-empty Message, got {:?}",
            body["Message"]
        );
    }
}
