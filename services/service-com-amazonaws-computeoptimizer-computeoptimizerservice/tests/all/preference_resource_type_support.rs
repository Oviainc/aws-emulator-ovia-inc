// SPDX-License-Identifier: CC0-1.0

//! Which `ResourceType` each recommendation preference may be SET for.
//!
//! One rule, asserted over the whole cross product rather than the one resource type a
//! recording happened to use: `PutRecommendationPreferences` stores a preference member
//! only for the resource types the service supports that preference for, and refuses it for
//! every other `ResourceType` the model's enum admits with `InvalidParameterValueException`
//! (HTTP 400), storing nothing — so a refused request leaves `GetRecommendationPreferences`
//! reporting no record.
//!
//! The supported set of each member is stated once, in [`members`], with the source of
//! each list on its row.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses.

use serde_json::{json, Value};

use crate::harness::{
    call, emulator, error_code, get_preferences, opt_in, scope_for, RESOURCE_TYPES,
};

/// The resource types whose recommendations carry an instance-level analysis feature:
/// `EffectiveRecommendationPreferences` — the shape reported for EC2 instance and Auto
/// Scaling group recommendations — is the only one in the model carrying
/// `enhancedInfrastructureMetrics` together with `inferredWorkloadTypes`, and the service
/// refuses either preference for any other resource type.
const INSTANCE_FEATURE: &[&str] = &["Ec2Instance", "AutoScalingGroup"];

/// The resource types a preference the service analyses per resource may be set for, from
/// `PutRecommendationPreferences$lookBackPeriod`: "You can only set this preference for the
/// Amazon EC2 instance, Auto Scaling group, Amazon EBS volume, Amazon ECS service on
/// Fargate, Amazon RDS DB instance, and Aurora DB cluster storage resource types" —
/// `preferredResources` documents the same six.
const ANALYSED_PER_RESOURCE: &[&str] = &[
    "Ec2Instance",
    "AutoScalingGroup",
    "EbsVolume",
    "EcsService",
    "RdsDBInstance",
    "AuroraDBClusterStorage",
];

/// One row per preference member of this operation: the member, a value from its modelled
/// shape, and the `ResourceType` values it may be set for.
///
/// * `externalMetricsPreference` — "This preference can only be set for the Ec2Instance
///   resource type."
/// * `utilizationPreferences` — "You can only set CPU and memory utilization preferences
///   for the Amazon EC2 instance resource type."
/// * `lookBackPeriod` / `preferredResources` — the six analysed-per-resource types.
/// * `enhancedInfrastructureMetrics` / `inferredWorkloadTypes` — the instance-feature pair.
///
/// `savingsEstimationMode`, the one member no resource type is refused for, is asserted
/// over the whole enum in `tests/all/unrestricted_preference_resource_types.rs`.
fn members() -> Vec<(&'static str, Value, &'static [&'static str])> {
    vec![
        (
            "enhancedInfrastructureMetrics",
            json!("Active"),
            INSTANCE_FEATURE,
        ),
        ("inferredWorkloadTypes", json!("Inactive"), INSTANCE_FEATURE),
        (
            "externalMetricsPreference",
            json!({"source": "Datadog"}),
            &["Ec2Instance"],
        ),
        (
            "utilizationPreferences",
            json!([{
                "metricName": "CpuUtilization",
                "metricParameters": {"threshold": "P95", "headroom": "PERCENT_20"},
            }]),
            &["Ec2Instance"],
        ),
        ("lookBackPeriod", json!("DAYS_93"), ANALYSED_PER_RESOURCE),
        (
            "preferredResources",
            json!([{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}]),
            ANALYSED_PER_RESOURCE,
        ),
    ]
}

/// Every preference member, on every `ResourceType` the enum admits: stored and read back
/// where the service supports it, refused with `InvalidParameterValueException` and stored
/// nowhere where it does not.
#[tokio::test]
async fn every_preference_member_is_settable_exactly_where_its_resource_types_allow() {
    for (member, value, supported) in members() {
        for resource_type in RESOURCE_TYPES {
            let scope = scope_for(resource_type);
            let case = format!("{member}={value} on {resource_type}");
            let accepted = supported.contains(resource_type);

            // A fresh emulator per row: what the read reports is this row's write alone,
            // which is what proves a refused request wrote nothing.
            let router = emulator();
            opt_in(&router).await;

            let mut request = json!({"resourceType": resource_type, "scope": scope});
            request[member] = value.clone();
            let (status, body) =
                call(&router, "PutRecommendationPreferences", request.clone()).await;

            let read = get_preferences(&router, resource_type, Some(&scope)).await;
            let details = read["recommendationPreferencesDetails"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a list of details, got {read}"))
                .clone();

            if !accepted {
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
                    message.contains(member) && message.contains(resource_type),
                    "case {case:?}: the refusal must name the preference and the resource \
                     type, got {body}"
                );
                assert!(
                    details.is_empty(),
                    "case {case:?}: a refused request must store nothing, got {read}"
                );
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: expected {request} to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {case:?}: PutRecommendationPreferences has an empty output, got {body}"
            );
            assert_eq!(
                details.len(),
                1,
                "case {case:?}: expected exactly the stored preference, got {read}"
            );
            let detail = &details[0];
            assert_eq!(
                detail["resourceType"],
                json!(resource_type),
                "case {case:?}: expected the detail to carry its resourceType, got {read}"
            );
            assert_eq!(
                detail["scope"], scope,
                "case {case:?}: expected the detail to carry the scope written, got {read}"
            );
            if member == "preferredResources" {
                // Reported as `EffectivePreferredResource`: the stored lists plus the
                // `effectiveIncludeList` expansion, so the stored lists are what the
                // read-back must carry.
                let got = &detail[member][0];
                let written = &value[0];
                for field in ["name", "includeList"] {
                    assert_eq!(
                        got[field], written[field],
                        "case {case:?}: expected the stored {field} {written}, got {got}"
                    );
                }
                continue;
            }
            assert_eq!(
                detail[member], value,
                "case {case:?}: expected the stored {member} to be read back, got {read}"
            );
        }
    }
}

/// Each `EnhancedInfrastructureMetrics` / `InferredWorkloadTypesPreference` value (the
/// model's enums: Active | Inactive) is refused for a resource type outside the
/// instance-feature pair, and accepted inside it: the refusal turns on the resource type
/// alone, never on which status was asked for.
#[tokio::test]
async fn an_instance_feature_preference_is_refused_outside_its_pair_for_either_status() {
    for member in ["enhancedInfrastructureMetrics", "inferredWorkloadTypes"] {
        for status_value in ["Active", "Inactive"] {
            for resource_type in RESOURCE_TYPES {
                let scope = scope_for(resource_type);
                let case = format!("{member}={status_value} on {resource_type}");
                let accepted = INSTANCE_FEATURE.contains(resource_type);

                let router = emulator();
                opt_in(&router).await;

                let mut request = json!({"resourceType": resource_type, "scope": scope});
                request[member] = json!(status_value);
                let (status, body) =
                    call(&router, "PutRecommendationPreferences", request.clone()).await;
                assert_eq!(
                    status,
                    if accepted { 200 } else { 400 },
                    "case {case:?}: expected {request} to be {}, got {status}: {body}",
                    if accepted { "accepted" } else { "refused" }
                );

                let read = get_preferences(&router, resource_type, Some(&scope)).await;
                let stored = &read["recommendationPreferencesDetails"][0][member];
                if accepted {
                    assert_eq!(
                        stored,
                        &json!(status_value),
                        "case {case:?}: expected the stored {member} to be read back, got {read}"
                    );
                } else {
                    assert!(
                        stored.is_null(),
                        "case {case:?}: a refused request must store nothing, got {read}"
                    );
                }
            }
        }
    }
}
