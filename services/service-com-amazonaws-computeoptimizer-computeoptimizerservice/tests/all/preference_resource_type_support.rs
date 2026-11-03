// SPDX-License-Identifier: CC0-1.0

//! Which `ResourceType` each recommendation preference may be SET for, and how a request for
//! any other one is refused.
//!
//! One rule, asserted over the whole cross product rather than the one resource type a
//! recording happened to use: `PutRecommendationPreferences` stores a preference only for the
//! resource types that preference applies to, and refuses it for every other `ResourceType`
//! the model's enum admits with `InvalidParameterValueException` (HTTP 400) and the service's
//! own sentence for that preference, storing nothing — so a refused request leaves
//! `GetRecommendationPreferences` reporting no record.
//!
//! The supported set of each preference, and the sentence its refusal carries, are stated
//! once, in [`members`], with the source of each list on its row. A preference that applies to
//! every resource type is asserted in
//! `tests/all/unrestricted_preference_resource_types.rs`; that the refusal below is worded the
//! same whatever ELSE the request carries is asserted in
//! `tests/all/preference_support_refusal_wording.rs`, and the instance-ARN refusal of the
//! recommendation readers in `tests/all/instance_arn_refusal_wording.rs`.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses.

use serde_json::{json, Value};

use crate::harness::{
    call, emulator, error_code, get_preferences, opt_in, scope_for, ANALYSED_PER_RESOURCE_TYPES,
    RESOURCE_TYPES,
};

/// The generic wording this service must never answer: a refusal built from a member name and
/// the value the request carried. Asserted absent on every refused row, so a refusal that
/// falls back to a parameter template fails whatever its remaining text says.
const GENERIC_PARAMETER_PREFIX: &str = "The value supplied for parameter";

/// One row per preference member of this operation whose resource types are RESTRICTED: the
/// member, a value from its modelled shape, the `ResourceType` values it may be set for, and
/// the sentence a request that sets it for any OTHER resource type is refused with.
///
/// * `externalMetricsPreference` — "This preference can only be set for the Ec2Instance
///   resource type."
/// * `utilizationPreferences` — "You can only set CPU and memory utilization preferences
///   for the Amazon EC2 instance resource type."
/// * `lookBackPeriod` / `preferredResources` — the six analysed-per-resource types.
/// * `enhancedInfrastructureMetrics` — the member's own documentation enumerates none, so the
///   model's effective-preferences shapes do: `EffectiveRecommendationPreferences` (an
///   instance or Auto Scaling group recommendation) and `RDSEffectiveRecommendationPreferences`
///   (an Amazon Aurora and RDS database recommendation, DB instance and storage together) are
///   the shapes that carry it. Its sentence advises only the two compute types, which is
///   narrower than what it accepts — so the rows assert the sentence as the service words it,
///   not as the supported set reads.
///
/// Each sentence names the preference by its `RecommendationPreferenceName` — the model's own
/// name for a recommendation preference — and the resource types it advises, and nothing else:
/// no member name and no value from the request.
///
/// The preferences that apply to every resource type — `savingsEstimationMode` and the status
/// switch `inferredWorkloadTypes` — are asserted over the whole enum in
/// `tests/all/unrestricted_preference_resource_types.rs`.
pub(crate) fn members() -> Vec<(&'static str, Value, &'static [&'static str], &'static str)> {
    vec![
        (
            "enhancedInfrastructureMetrics",
            json!("Active"),
            &[
                "Ec2Instance",
                "AutoScalingGroup",
                "RdsDBInstance",
                "AuroraDBClusterStorage",
            ],
            "The specified resource type is not valid for \"EnhancedInfrastructureMetrics\" \
             preference. Specify \"Ec2Instance\" or \"AutoScalingGroup\" in your request and \
             try again.",
        ),
        (
            "externalMetricsPreference",
            json!({"source": "Datadog"}),
            &["Ec2Instance"],
            "The specified resource type is not valid for \"ExternalMetricsPreference\" \
             preference. Specify \"Ec2Instance\" in your request and try again.",
        ),
        (
            "utilizationPreferences",
            json!([{
                "metricName": "CpuUtilization",
                "metricParameters": {"threshold": "P95", "headroom": "PERCENT_20"},
            }]),
            &["Ec2Instance"],
            "The specified resource type is not valid for \"UtilizationPreferences\" \
             preference. Specify \"Ec2Instance\" in your request and try again.",
        ),
        (
            "lookBackPeriod",
            json!("DAYS_93"),
            ANALYSED_PER_RESOURCE_TYPES,
            "The specified resource type is not valid for \"LookBackPeriodPreference\" \
             preference. Specify \"Ec2Instance\", \"AutoScalingGroup\", \"EbsVolume\", \
             \"EcsService\", \"RdsDBInstance\", or \"AuroraDBClusterStorage\" in your request \
             and try again.",
        ),
        (
            "preferredResources",
            json!([{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}]),
            ANALYSED_PER_RESOURCE_TYPES,
            "The specified resource type is not valid for \"PreferredResources\" preference. \
             Specify \"Ec2Instance\", \"AutoScalingGroup\", \"EbsVolume\", \"EcsService\", \
             \"RdsDBInstance\", or \"AuroraDBClusterStorage\" in your request and try again.",
        ),
    ]
}

/// Every preference member, on every `ResourceType` the enum admits: stored and read back
/// where the service supports it, refused with `InvalidParameterValueException` and stored
/// nowhere where it does not.
#[tokio::test]
async fn every_preference_member_is_settable_exactly_where_its_resource_types_allow() {
    for (member, value, supported, expected) in members() {
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
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the service's sentence {expected:?}, got \
                     {message:?}"
                );
                assert!(
                    !message.contains(GENERIC_PARAMETER_PREFIX),
                    "case {case:?}: the refusal must not be built from a parameter template, \
                     got {message:?}"
                );
                assert!(
                    !message.contains(member) && !message.contains(resource_type),
                    "case {case:?}: that sentence names neither the member the preference \
                     arrived on nor the rejected resource type, got {body}"
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

