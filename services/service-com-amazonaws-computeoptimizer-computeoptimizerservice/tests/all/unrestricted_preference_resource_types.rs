// SPDX-License-Identifier: CC0-1.0

//! A recommendation preference whose own documentation names no resource-type restriction
//! is settable for EVERY `ResourceType` the model's enum admits, and round-trips through
//! `GetRecommendationPreferences` for the `(resourceType, scope)` key it was written at.
//!
//! `PutRecommendationPreferences` may refuse a preference for a resource type only where
//! that member's documentation restricts it ("can only be set for …"):
//! `externalMetricsPreference` and `utilizationPreferences` (Ec2Instance only),
//! `lookBackPeriod` and `preferredResources` (the six analysed-per-resource types).
//! `enhancedInfrastructureMetrics`, `inferredWorkloadTypes` and `savingsEstimationMode`
//! carry no such sentence, so no resource type may be refused for them — the tables below
//! walk the whole enum rather than the one resource type a recording happened to use.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RESOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    RESOURCE_TYPE_AUTO_SCALING_GROUP, SCOPE_ACCOUNT_ID, SCOPE_RESOURCE_ARN,
};

use crate::harness::{
    auto_scaling_group_arn, call, emulator, error_code, get_preferences, opt_in, put_preference,
    ACCOUNT,
};

/// Every value of the model's `ResourceType` enum — the type this operation's
/// `resourceType` member carries — taken from the crate's generated value set rather than
/// copied, so a resource type the model adds is covered here the day it appears
/// (`RESOURCE_TYPE_VALUES`: AuroraDBClusterStorage | AutoScalingGroup | EbsVolume |
/// Ec2Instance | EcsService | Idle | LambdaFunction | License | NotApplicable |
/// RdsDBInstance).
const RESOURCE_TYPES: &[&str] = RESOURCE_TYPE_VALUES;

/// The scope a preference for `resource_type` may be written at.
///
/// An Auto Scaling group preference "can only be set at the resource level by specifying a
/// scope name of `ResourceArn`" (`PutRecommendationPreferences$scope`); every other
/// resource type takes the account-level scope, whose "value must be a 12-digit Amazon Web
/// Services account ID" (`Scope$value`).
fn scope_for(resource_type: &str) -> Value {
    if resource_type == RESOURCE_TYPE_AUTO_SCALING_GROUP {
        json!({"name": SCOPE_RESOURCE_ARN, "value": auto_scaling_group_arn("batch-tier")})
    } else {
        json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})
    }
}

/// `enhancedInfrastructureMetrics` is accepted — HTTP 200 with an empty body — and stored
/// for EVERY `ResourceType` the enum admits, and the stored record is what the matching
/// `GetRecommendationPreferences` read reports back.
#[tokio::test]
async fn enhanced_metrics_is_settable_for_every_resource_type() {
    // The two `EnhancedInfrastructureMetrics` values (model: Active | Inactive), alternated
    // so neither status is the only one exercised.
    for (index, resource_type) in RESOURCE_TYPES.iter().enumerate() {
        let status = if index % 2 == 0 { "Active" } else { "Inactive" };
        let scope = scope_for(resource_type);
        let case = format!("{resource_type} enhancedInfrastructureMetrics={status}");

        // A fresh emulator per row: what the read reports is this row's write, not the last.
        let router = emulator();
        opt_in(&router).await;

        let request = json!({
            "resourceType": resource_type,
            "scope": scope,
            "enhancedInfrastructureMetrics": status,
        });
        // `put_preference` asserts HTTP 200 and the modelled empty output body.
        put_preference(&router, &request).await;

        let read = get_preferences(&router, resource_type, Some(&scope)).await;
        let details = read["recommendationPreferencesDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list of details, got {read}"));
        assert_eq!(
            details.len(),
            1,
            "case {case:?}: expected exactly the stored preference, got {read}"
        );
        assert_eq!(
            details[0],
            json!({
                "scope": scope,
                "resourceType": resource_type,
                "enhancedInfrastructureMetrics": status,
                // Not written by this request, so not reported: the detail carries the
                // members that were set.
            }),
            "case {case:?}: expected the written preference read back, got {read}"
        );
    }
}

/// Each preference member whose documentation names NO resource-type restriction is
/// accepted and read back for a resource type beyond the ones a recording covers.
#[tokio::test]
async fn every_unrestricted_preference_member_is_settable_for_every_resource_type() {
    // (the member, a value from its modelled enum)
    //   enhancedInfrastructureMetrics -> EnhancedInfrastructureMetrics (Active | Inactive)
    //   inferredWorkloadTypes         -> InferredWorkloadTypesPreference (Active | Inactive)
    //   savingsEstimationMode         -> SavingsEstimationMode (AfterDiscounts |
    //                                    BeforeDiscounts)
    // None of the three documents a resource-type restriction, so each is asserted over the
    // whole `ResourceType` enum.
    let members = [
        ("enhancedInfrastructureMetrics", "Inactive"),
        ("inferredWorkloadTypes", "Inactive"),
        ("savingsEstimationMode", "AfterDiscounts"),
    ];

    for (member, value) in members {
        for resource_type in RESOURCE_TYPES {
            let scope = scope_for(resource_type);
            let case = format!("{member}={value} on {resource_type}");

            let router = emulator();
            opt_in(&router).await;

            let mut request = json!({"resourceType": resource_type, "scope": scope});
            request[member] = json!(value);
            put_preference(&router, &request).await;

            let read = get_preferences(&router, resource_type, Some(&scope)).await;
            let detail = &read["recommendationPreferencesDetails"][0];
            assert_eq!(
                detail[member], json!(value),
                "case {case:?}: expected the stored {member} to be read back, got {read}"
            );
            assert_eq!(
                detail["resourceType"],
                json!(resource_type),
                "case {case:?}: expected the detail to carry its resourceType, got {read}"
            );
            assert_eq!(
                detail["scope"], scope,
                "case {case:?}: expected the detail to carry the scope written, got {read}"
            );
        }
    }
}

/// The preference store is keyed by `(resourceType, scope)`: a preference written for one
/// resource type is invisible to a read of another, a scope-less read sees the records of
/// every scope of that type, and a second write to the same key MERGES into it.
#[tokio::test]
async fn preferences_are_keyed_by_resource_type_and_scope() {
    let router = emulator();
    opt_in(&router).await;

    let account_scope = json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT});
    let organization_scope = json!({"name": "Organization", "value": "ALL_ACCOUNTS"});

    put_preference(
        &router,
        &json!({
            "resourceType": "RdsDBInstance",
            "scope": account_scope,
            "enhancedInfrastructureMetrics": "Inactive",
            "lookBackPeriod": "DAYS_14",
        }),
    )
    .await;
    put_preference(
        &router,
        &json!({
            "resourceType": "RdsDBInstance",
            "scope": organization_scope,
            "enhancedInfrastructureMetrics": "Active",
        }),
    )
    .await;
    put_preference(
        &router,
        &json!({
            "resourceType": "AuroraDBClusterStorage",
            "scope": account_scope,
            "enhancedInfrastructureMetrics": "Active",
        }),
    )
    .await;

    // Keyed by scope: the account-scoped read sees only the account-scoped record.
    let read = get_preferences(&router, "RdsDBInstance", Some(&account_scope)).await;
    assert_eq!(
        read["recommendationPreferencesDetails"],
        json!([{
            "scope": account_scope,
            "resourceType": "RdsDBInstance",
            "enhancedInfrastructureMetrics": "Inactive",
            "lookBackPeriod": "DAYS_14",
        }]),
        "case \"account-scoped read\": got {read}"
    );

    // Keyed by resource type: a read of one type never reports another's record.
    let read = get_preferences(&router, "RdsDBInstance", None).await;
    let details = read["recommendationPreferencesDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"scope-less read\": expected a list, got {read}"));
    assert_eq!(
        details.len(),
        2,
        "case \"scope-less read\": expected both RdsDBInstance records, got {read}"
    );
    assert!(
        details
            .iter()
            .all(|detail| detail["resourceType"] == json!("RdsDBInstance")),
        "case \"scope-less read\": every detail must be an RdsDBInstance record, got {read}"
    );

    // "Creates a new recommendation preference or updates an existing recommendation
    // preference": a second write to the same key replaces what it sets and leaves the rest.
    put_preference(
        &router,
        &json!({
            "resourceType": "RdsDBInstance",
            "scope": account_scope,
            "enhancedInfrastructureMetrics": "Active",
        }),
    )
    .await;
    let read = get_preferences(&router, "RdsDBInstance", Some(&account_scope)).await;
    assert_eq!(
        read["recommendationPreferencesDetails"],
        json!([{
            "scope": account_scope,
            "resourceType": "RdsDBInstance",
            "enhancedInfrastructureMetrics": "Active",
            "lookBackPeriod": "DAYS_14",
        }]),
        "case \"upsert of one key\": got {read}"
    );
}

/// The boundary of the rule: a member whose documentation DOES restrict its resource types
/// is still refused outside them, with `InvalidParameterValueException` (HTTP 400), and
/// still stores nothing — so widening the unrestricted members has not widened these.
#[tokio::test]
async fn a_documented_resource_type_restriction_still_refuses() {
    // (case, the request, the member it sets)
    //   externalMetricsPreference: "can only be set for the Ec2Instance resource type"
    //   utilizationPreferences: "only ... for the Amazon EC2 instance resource type"
    //   lookBackPeriod: the six analysed-per-resource types, so not License
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "external metrics on RdsDBInstance",
            json!({
                "resourceType": "RdsDBInstance",
                "externalMetricsPreference": {"source": "Datadog"},
            }),
            "externalMetricsPreference",
        ),
        (
            "utilization preferences on AuroraDBClusterStorage",
            json!({
                "resourceType": "AuroraDBClusterStorage",
                "utilizationPreferences": [{
                    "metricName": "CpuUtilization",
                    "metricParameters": {"headroom": "PERCENT_20"},
                }],
            }),
            "utilizationPreferences",
        ),
        (
            "look-back period on License",
            json!({"resourceType": "License", "lookBackPeriod": "DAYS_93"}),
            "lookBackPeriod",
        ),
    ];

    for (case, request, member) in rows {
        let router = emulator();
        opt_in(&router).await;

        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name the preference, got {body}"
        );

        let resource_type = request["resourceType"].as_str().expect("a resourceType");
        let read = get_preferences(&router, resource_type, None).await;
        let details = read["recommendationPreferencesDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list of details, got {read}"));
        assert!(
            details.is_empty(),
            "case {case:?}: a refused request must store nothing, got {read}"
        );
    }
}
